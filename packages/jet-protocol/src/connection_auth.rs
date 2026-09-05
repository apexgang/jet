//! Ed25519 proof of one remote connection's offered negotiation (ADR-0090).

use crate::{ClientHello, ControlError, encode_control};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The two enrollment operations available before remote authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "pairing", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemotePairingRequest {
	/// Present the out-of-band one-time secret and this installation's key.
	Claim {
		/// Stable Command identity, retained across retries.
		command_id: Uuid,
		/// The manual code or QR payload, never retained in diagnostics.
		secret: String,
		/// The installation's public key.
		key: crate::ClientPublicKey,
	},
	/// Prove the Pairing transcript after confirmation on the target.
	Complete {
		/// Stable Command identity.
		command_id: Uuid,
		/// The previously claimed offer.
		offer_id: Uuid,
		/// Signature of the claim's signing bytes.
		#[serde(with = "crate::hex")]
		signature: [u8; 64],
	},
}

/// Enrollment-only reply; it never authorizes application streams.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemotePairingResponse {
	/// The token was accepted; compare the string on both screens.
	Claimed {
		/// The claim and its confirmation string.
		pending: crate::PendingPairing,
		/// The bound Pairing transcript, avoiding any protected status Query.
		signing_bytes: Vec<u8>,
	},
	/// Pairing completed; reconnect with a fresh connection signature.
	Completed {
		/// The newly Paired client.
		client: crate::PairedClient,
	},
	/// Enrollment was refused.
	Rejected {
		/// Stable explanation.
		error: crate::WireError,
	},
}

/// The only message accepted in response to a remote challenge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionProof {
	/// Signature of [`connection_signing_bytes`] by the Paired client's key.
	#[serde(with = "crate::hex")]
	pub signature: [u8; 64],
}

/// Domain-separated proof bytes, binding the identity, offered negotiation,
/// and fresh nonce. Pairing signatures cannot be reused as login signatures.
///
/// # Errors
/// Returns a codec error if the offered hello cannot be encoded.
pub fn connection_signing_bytes(
	hello: &ClientHello,
	nonce: &[u8; 32],
) -> Result<Vec<u8>, ControlError> {
	let mut bytes = b"jet.connection.v1\0ed25519\0".to_vec();
	bytes.extend(encode_control(hello)?);
	bytes.extend(nonce);
	Ok(bytes)
}
