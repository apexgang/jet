//! The only age dependency boundary. No armor, SSH, plugins, or fallback cipher.
use crate::CoreError;
use age::secrecy::{ExposeSecret, SecretString};
use serde::{Serialize, Serializer};
use std::{
	fmt,
	io::{Read, Write},
};
use subtle::ConstantTimeEq;

pub(super) const MAX_BYTES: usize = 256 * 1024 * 1024;
#[derive(Clone)]
struct Secret(SecretString);
impl Secret {
	fn new(value: String) -> Result<Self, CoreError> {
		if value.is_empty() || value.len() > 1024 {
			return Err(invalid());
		}
		Ok(Self(value.into()))
	}
}
impl fmt::Debug for Secret {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("[redacted]")
	}
}
impl PartialEq for Secret {
	fn eq(&self, other: &Self) -> bool {
		self.0
			.expose_secret()
			.as_bytes()
			.ct_eq(other.0.expose_secret().as_bytes())
			.into()
	}
}
impl Eq for Secret {}
impl Serialize for Secret {
	fn serialize<S: Serializer>(
		&self,
		serializer: S,
	) -> Result<S::Ok, S::Error> {
		// Never expose a fast password verifier through Command serialization.
		serializer.serialize_str("[redacted]")
	}
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
enum Protection {
	Passphrase(Secret),
	Recipients(Vec<String>),
	Unencrypted { acknowledged_warning: String },
}
/// Required export encryption. No implicit plaintext default exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryProtection(Protection);
/// Show this complete disclosure before accepting an unencrypted export.
pub const RECOVERY_PLAINTEXT_WARNING: &str = "Export includes retained Conversation and Run history, transcript content, referenced Artifacts, disabled schedules, Craft metadata, and non-secret Settings. Anyone with this unencrypted bundle can read its content. I authorize this unencrypted export.";
impl RecoveryProtection {
	/// Accepts only the exact included-data disclosure and explicit warning.
	/// Returns an input error if the acknowledged disclosure does not match.
	pub fn unencrypted(
		acknowledged_warning: String,
	) -> Result<Self, CoreError> {
		if acknowledged_warning != RECOVERY_PLAINTEXT_WARNING {
			return Err(invalid());
		}
		Ok(Self(Protection::Unencrypted {
			acknowledged_warning,
		}))
	}
	pub(crate) fn is_unencrypted(&self) -> bool {
		matches!(self.0, Protection::Unencrypted { .. })
	}

	/// Uses age v1 scrypt. Losing this passphrase makes recovery impossible.
	/// Returns an input error for an empty or excessive passphrase.
	pub fn passphrase(value: String) -> Result<Self, CoreError> {
		Secret::new(value).map(|secret| Self(Protection::Passphrase(secret)))
	}
	/// Uses one to sixteen age X25519 public recipients.
	/// Returns an input error for unsupported, missing, or malformed recipients.
	pub fn recipients(values: Vec<String>) -> Result<Self, CoreError> {
		if values.is_empty()
			|| values.len() > 16
			|| values.iter().any(|v| {
				v.len() > 128 || v.parse::<age::x25519::Recipient>().is_err()
			}) {
			return Err(invalid());
		}
		Ok(Self(Protection::Recipients(values)))
	}
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
enum Key {
	Passphrase(Secret),
	Identity(Secret),
	Unencrypted,
}
/// An ephemeral import key, never persisted by the core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryKey(Key);
impl RecoveryKey {
	/// Explicitly accepts a checksummed, unauthenticated plaintext bundle.
	/// An encrypted-key attempt never falls back to this mode.
	pub fn unencrypted() -> Self {
		Self(Key::Unencrypted)
	}

	/// Decrypts an age scrypt bundle; empty and excessive passphrases are refused.
	pub fn passphrase(value: String) -> Result<Self, CoreError> {
		Secret::new(value).map(|secret| Self(Key::Passphrase(secret)))
	}
	/// Decrypts with a native age X25519 private identity.
	/// Returns an input error for unsupported or malformed identities.
	pub fn identity(value: String) -> Result<Self, CoreError> {
		let secret = Secret::new(value)?;
		secret
			.0
			.expose_secret()
			.parse::<age::x25519::Identity>()
			.map_err(|_| invalid())?;
		Ok(Self(Key::Identity(secret)))
	}
}
pub(super) fn encrypt(
	bytes: &[u8],
	protection: RecoveryProtection,
) -> Result<Vec<u8>, CoreError> {
	if bytes.len() > MAX_BYTES {
		return Err(invalid());
	}
	// ASVS 11.2.1, 11.2.5: age owns randomness, header MAC, and stream AEAD.
	let recipients: Vec<Box<dyn age::Recipient>> = match protection.0 {
		Protection::Unencrypted { .. } => return Ok(bytes.to_vec()),
		Protection::Passphrase(secret) => {
			let mut recipient = age::scrypt::Recipient::new(secret.0);
			recipient.set_work_factor(18);
			vec![Box::new(recipient)]
		}
		Protection::Recipients(values) => values
			.into_iter()
			.map(|v| {
				v.parse::<age::x25519::Recipient>()
					.map(|r| Box::new(r) as Box<dyn age::Recipient>)
					.map_err(|_| invalid())
			})
			.collect::<Result<_, _>>()?,
	};
	let encryptor =
		age::Encryptor::with_recipients(recipients.iter().map(Box::as_ref))
			.map_err(|_| invalid())?;
	let mut result = Vec::new();
	let mut writer =
		encryptor.wrap_output(&mut result).map_err(|_| invalid())?;
	writer.write_all(bytes).map_err(|_| invalid())?;
	writer.finish().map_err(|_| invalid())?;
	if result.len() > MAX_BYTES {
		return Err(invalid());
	}
	Ok(result)
}
pub(super) fn decrypt(
	bytes: &[u8],
	key: RecoveryKey,
) -> Result<Vec<u8>, CoreError> {
	if bytes.len() > MAX_BYTES {
		return Err(invalid());
	}
	let identity: Box<dyn age::Identity> = match key.0 {
		Key::Unencrypted => return Ok(bytes.to_vec()),
		Key::Passphrase(secret) => {
			let mut identity = age::scrypt::Identity::new(secret.0);
			// Bound hostile KDF headers to 256 MiB. Export uses the same work factor.
			identity.set_max_work_factor(18);
			Box::new(identity)
		}
		Key::Identity(secret) => Box::new(
			secret
				.0
				.expose_secret()
				.parse::<age::x25519::Identity>()
				.map_err(|_| invalid())?,
		),
	};
	let decryptor = age::Decryptor::new(bytes).map_err(|_| invalid())?;
	let reader = decryptor
		.decrypt(std::iter::once(identity.as_ref()))
		.map_err(|_| invalid())?;
	let mut result = Vec::new();
	reader
		.take(MAX_BYTES as u64 + 1)
		.read_to_end(&mut result)
		.map_err(|_| invalid())?;
	if result.len() > MAX_BYTES {
		return Err(invalid());
	}
	Ok(result)
}
pub(super) fn invalid() -> CoreError {
	CoreError::invalid_input(
		"recovery.invalid_bundle",
		"Recovery bundle or encryption parameters are invalid",
	)
}
