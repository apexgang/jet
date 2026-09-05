//! Proving that the client completing a Pairing holds the Client identity
//! it presented (ADR-0017, ADR-0090).
//!
//! The short-lived secret proved fresh intent; this proves who the intent
//! belongs to. The installation signs the transcript of its own claim, so
//! what is verified names the Plane, the offer, the client, the key, and
//! the challenge it was answered with — not a bare nonce a signature could
//! be replayed from somewhere else.

use ed25519_dalek::{Signature, VerifyingKey};
use jet_store::PairingKeyAlgorithm;

use crate::pairing::{ClientPublicKey, PairingSignature};

/// Whether `signature` over `transcript` was made by the private half of
/// `key`.
///
/// Verification is strict: a key of small order and a signature that is not
/// canonically encoded are both refused, so one signed transcript has one
/// signature and a second key cannot be made to verify the same bytes.
pub(crate) fn verifies(
	key: &ClientPublicKey,
	transcript: &[u8],
	signature: &PairingSignature,
) -> bool {
	match key.algorithm {
		PairingKeyAlgorithm::Ed25519 => {
			let Ok(verifying) = VerifyingKey::from_bytes(&key.key) else {
				return false;
			};
			verifying
				.verify_strict(transcript, &Signature::from_bytes(&signature.0))
				.is_ok()
		}
	}
}
