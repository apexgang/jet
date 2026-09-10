//! The one-time secret a Pairing offer is claimed with, and the strings
//! both sides derive from it (ADR-0017).
//!
//! The secret proves fresh intent and nothing else: it is disclosed once,
//! lives for two minutes, and is spent by the first claim. What the Plane
//! keeps of it is a salted digest, so the store it is written to cannot be
//! read to claim the offer it describes, and every comparison against it is
//! constant-time.

use crate::{
	error::CoreError,
	pairing::{
		AuthenticationString, ClientPublicKey, PairingChallenge,
		PairingDisclosure, PairingSecret,
	},
};
use jet_store::PairingMethod;
use sha2::{Digest, Sha256};
use std::mem::MaybeUninit;
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// Digits in a manual pairing code, which ADR-0017 fixes at eight and
/// numeric so it can be read out loud and typed on any keyboard.
const MANUAL_CODE_DIGITS: usize = 8;

/// Where the manual code is grouped, as `xxxx-yyyy`.
const MANUAL_CODE_GROUP: usize = 4;

/// Digits in the authentication string the people at both ends compare.
/// Six is one chance in a million of a substituted key showing the same
/// string, against a single attempt that ends the offer either way.
const AUTHENTICATION_DIGITS: u32 = 6;

/// Bytes of the one-time token a QR payload carries (ADR-0017: 128 bits).
const TOKEN_BYTES: usize = 16;

/// The scheme and version a QR payload is read back under. The payload is
/// `jet-pair:1:<token>:<endpoint>`: the version is second so a reader knows
/// the grammar before it parses anything, and the endpoint is last so its
/// own colons need no escaping.
const QR_SCHEME: &str = "jet-pair";
const QR_VERSION: u32 = 1;

/// Separates the domain from what is hashed under it, so no value can be
/// arranged to hash the same way under two of them.
const SEPARATOR: u8 = 0;
const SECRET_DOMAIN: &[u8] = b"jet.pairing.secret.v1";
const TRANSCRIPT_DOMAIN: &[u8] = b"jet.pairing.transcript.v1";
const AUTHENTICATION_DOMAIN: &[u8] = b"jet.pairing.authentication.v1";

/// Issues the one-time secret for an offer made through `method`, and the
/// form it is disclosed in.
///
/// # Errors
///
/// Returns an `unavailable` [`CoreError`] when the Plane has no entropy to
/// draw the secret from.
pub(crate) fn issue(
	method: &PairingMethod,
) -> Result<(PairingSecret, PairingDisclosure), CoreError> {
	match method {
		PairingMethod::ManualCode => {
			let code = manual_code()?;
			Ok((
				PairingSecret(code.clone()),
				PairingDisclosure::ManualCode { code },
			))
		}
		PairingMethod::QrPayload { endpoint } => {
			let token = hex(&random_bytes::<TOKEN_BYTES>()?);
			let payload =
				format!("{QR_SCHEME}:{QR_VERSION}:{token}:{endpoint}");
			Ok((
				PairingSecret(token),
				PairingDisclosure::QrPayload { payload },
			))
		}
	}
}

/// A fresh salt for one offer's digest.
///
/// # Errors
///
/// Returns an `unavailable` [`CoreError`] when the Plane has no entropy.
pub(crate) fn salt() -> Result<[u8; 16], CoreError> {
	random_bytes()
}

/// A fresh challenge for the claiming client's key to sign.
///
/// # Errors
///
/// Returns an `unavailable` [`CoreError`] when the Plane has no entropy.
pub(crate) fn challenge() -> Result<PairingChallenge, CoreError> {
	random_bytes().map(PairingChallenge)
}

/// The digest an offer keeps of its secret.
pub(crate) fn digest(
	salt: &[u8; 16],
	method: &PairingMethod,
	secret: &PairingSecret,
) -> [u8; 32] {
	let mut hash = Sha256::new();
	hash.update(SECRET_DOMAIN);
	hash.update([SEPARATOR]);
	hash.update(salt);
	hash.update(normalize(method, secret).as_bytes());
	hash.finalize().into()
}

/// Whether `presented` is the secret `digested` was taken over.
///
/// The comparison is over digests of the same width and runs in constant
/// time, so a wrong secret tells the caller nothing but that it was wrong.
pub(crate) fn matches(
	salt: &[u8; 16],
	digested: &[u8; 32],
	method: &PairingMethod,
	presented: &PairingSecret,
) -> bool {
	digest(salt, method, presented).ct_eq(digested).into()
}

/// What the people at both ends compare, and what the claiming client's
/// key signs to complete the Pairing.
///
/// Every value that decides who is being paired with whom is in here: the
/// Plane, the offer, the client, the key it presented, and the challenge
/// this claim was answered with. A substituted key is therefore a different
/// string on the two screens.
pub(crate) fn transcript(
	plane_id: Uuid,
	offer_id: Uuid,
	client_id: Uuid,
	key: &ClientPublicKey,
	challenge: &PairingChallenge,
) -> Vec<u8> {
	let mut transcript = Vec::new();
	transcript.extend_from_slice(TRANSCRIPT_DOMAIN);
	transcript.push(SEPARATOR);
	transcript.extend_from_slice(plane_id.as_bytes());
	transcript.extend_from_slice(offer_id.as_bytes());
	transcript.extend_from_slice(client_id.as_bytes());
	transcript.extend_from_slice(key.algorithm.as_str().as_bytes());
	transcript.push(SEPARATOR);
	transcript.extend_from_slice(&key.key);
	transcript.extend_from_slice(&challenge.0);
	transcript
}

/// The string both sides display for the people to compare.
pub(crate) fn authentication_string(transcript: &[u8]) -> AuthenticationString {
	let mut hash = Sha256::new();
	hash.update(AUTHENTICATION_DOMAIN);
	hash.update([SEPARATOR]);
	hash.update(transcript);
	let digest: [u8; 32] = hash.finalize().into();
	let (drawn, _) = digest.split_at(size_of::<u64>());
	let drawn = u64::from_be_bytes(drawn.try_into().unwrap_or([0; 8]));
	let modulus = 10u64.pow(AUTHENTICATION_DIGITS);
	let value = drawn % modulus;
	let digits =
		format!("{value:0width$}", width = AUTHENTICATION_DIGITS as usize);
	let (first, second) = digits.split_at(digits.len() / 2);
	AuthenticationString(format!("{first}-{second}"))
}

/// Whether the string presented back is the one this Pairing is showing.
///
/// It is not a secret — both screens display it — but it is what a client
/// racing a substituted key would have to guess, so it is compared the same
/// way the secret is.
pub(crate) fn same_authentication_string(
	displayed: &AuthenticationString,
	presented: &AuthenticationString,
) -> bool {
	let (displayed, presented) =
		(displayed.0.as_bytes(), presented.0.as_bytes());
	displayed.len() == presented.len() && bool::from(displayed.ct_eq(presented))
}

/// The secret as it is hashed and compared, whatever spacing the person
/// typing it used. A manual code is its digits; a token is the lowercase
/// hexadecimal the QR payload carried.
fn normalize(method: &PairingMethod, secret: &PairingSecret) -> String {
	match method {
		PairingMethod::ManualCode => {
			secret.0.chars().filter(char::is_ascii_digit).collect()
		}
		PairingMethod::QrPayload { .. } => secret.0.trim().to_ascii_lowercase(),
	}
}

/// An eight-digit numeric code, grouped as `xxxx-yyyy`.
fn manual_code() -> Result<String, CoreError> {
	let mut digits = String::with_capacity(MANUAL_CODE_DIGITS);
	for _ in 0..MANUAL_CODE_DIGITS {
		digits.push(char::from(b'0' + digit()?));
	}
	let (first, second) = digits.split_at(MANUAL_CODE_GROUP);
	Ok(format!("{first}-{second}"))
}

/// One uniform decimal digit. A byte at or above the largest multiple of
/// ten it can hold is drawn again, so no digit is likelier than another.
fn digit() -> Result<u8, CoreError> {
	const LARGEST_MULTIPLE: u8 = 250;
	loop {
		let [byte] = random_bytes()?;
		if byte < LARGEST_MULTIPLE {
			return Ok(byte % 10);
		}
	}
}

/// Bytes as the lowercase hexadecimal this protocol spells them in.
pub(crate) fn hex(bytes: &[u8]) -> String {
	bytes.iter().fold(String::new(), |mut text, byte| {
		use std::fmt::Write as _;
		let _ = write!(text, "{byte:02x}");
		text
	})
}

/// Draws a complete array from the operating system.
///
/// A Plane that cannot answer is not asked to carry on with a weaker
/// secret: Pairing is refused until it can (ADR-0017).
fn random_bytes<const N: usize>() -> Result<[u8; N], CoreError> {
	// ASVS 11.5.1: only the OS CSPRNG initializes the returned bytes.
	// No zero-filled salt or token can survive an entropy-source failure.
	let mut buffer = [MaybeUninit::uninit(); N];
	let bytes = getrandom::fill_uninit(&mut buffer).map_err(|error| {
		CoreError::unavailable(
			"pairing.entropy_unavailable",
			"this Plane cannot draw a Pairing secret right now",
			error.to_string(),
		)
	})?;
	Ok(std::array::from_fn(|index| bytes[index]))
}

#[cfg(test)]
mod tests {
	use std::collections::HashSet;

	use jet_store::PairingMethod;

	use super::{digest, matches, salt};
	use crate::pairing::PairingSecret;

	#[test]
	fn fresh_salts_separate_digests_of_the_same_secret() {
		let method = PairingMethod::ManualCode;
		let secret = PairingSecret("1234-5678".into());
		let mut salts = HashSet::new();
		let mut digests = HashSet::new();
		for _ in 0..32 {
			let salt = salt().expect("the OS supplies entropy");
			let hashed = digest(&salt, &method, &secret);
			// Report only the failed invariant, never salts, digests, or secrets.
			assert!(salt != [0; 16], "salt must be initialized with entropy");
			assert!(salts.insert(salt), "each offer needs a fresh salt");
			assert!(
				digests.insert(hashed),
				"salt must affect the secret digest"
			);
			assert!(matches(&salt, &hashed, &method, &secret));
		}
	}

	#[test]
	fn a_digest_is_bound_to_its_salt_and_normalized_secret() {
		let method = PairingMethod::ManualCode;
		let secret = PairingSecret("1234-5678".into());
		let original_salt = salt().expect("the OS supplies entropy");
		let other_salt = salt().expect("the OS supplies entropy");
		let hashed = digest(&original_salt, &method, &secret);

		assert!(matches(
			&original_salt,
			&hashed,
			&method,
			&PairingSecret("1234 5678".into()),
		));
		assert!(!matches(&other_salt, &hashed, &method, &secret));
		assert!(!matches(
			&original_salt,
			&hashed,
			&method,
			&PairingSecret("8765-4321".into()),
		));
	}
}
