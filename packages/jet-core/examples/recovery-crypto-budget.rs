//! Links every permitted Recovery cipher for the incremental size gate.
#[path = "support/recovery_error.rs"]
mod error;
use error::CoreError;
#[path = "../src/store_recovery/bundle/crypto.rs"]
mod crypto;
fn main() -> Result<(), CoreError> {
	use age::secrecy::ExposeSecret;
	let value = std::env::args()
		.nth(1)
		.unwrap_or_else(|| "size probe".into());
	let (protection, key) = if value.starts_with("passphrase") {
		(
			crypto::RecoveryProtection::passphrase(value.clone())?,
			crypto::RecoveryKey::passphrase(value.clone())?,
		)
	} else if value == "plaintext" {
		let protection = crypto::RecoveryProtection::unencrypted(
			crypto::RECOVERY_PLAINTEXT_WARNING.into(),
		)?;
		assert!(protection.is_unencrypted());
		(protection, crypto::RecoveryKey::unencrypted())
	} else {
		let identity = age::x25519::Identity::generate();
		(
			crypto::RecoveryProtection::recipients(vec![
				identity.to_public().to_string(),
			])?,
			crypto::RecoveryKey::identity(
				identity.to_string().expose_secret().into(),
			)?,
		)
	};
	let ciphertext = crypto::encrypt(value.as_bytes(), protection)?;
	assert_eq!(crypto::decrypt(&ciphertext, key)?, value.as_bytes());
	Ok(())
}
