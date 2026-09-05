//! Wire form of Pairing: how a GUI client comes to control a Plane
//! (ADR-0017).
//!
//! The Pairing gate is Plane-level and concerns new clients only. Behind an
//! open gate the Plane issues one offer at a time, whose one-time secret it
//! discloses once, to the owner who asked for it. Nothing else on this side
//! of the seam carries that secret: an offer a client reads back names
//! itself, how far it has got, and how long it has left.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Whether a Plane accepts new Pairings right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingGate {
	/// A new GUI client may begin Pairing.
	Open,
	/// It may not. A Plane starts here, so an owner opens the gate for as
	/// long as a pairing takes rather than leaving it open.
	Closed,
}

/// How the Plane hands one offer's one-time secret to the person pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum PairingMethod {
	/// An eight-digit numeric code, read off the target and typed into the
	/// GUI client.
	ManualCode,
	/// A versioned payload the GUI client scans.
	QrPayload {
		/// The reachable endpoint the payload advertises.
		endpoint: String,
	},
}

/// The signature algorithm one Client identity signs with, retained beside
/// the key so a Plane that later speaks a second one can tell which key is
/// which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingKeyAlgorithm {
	/// Ed25519, the only algorithm a v1 Client identity uses.
	Ed25519,
}

/// The public half of one Client identity: the durable credential a
/// completed Pairing leaves behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientPublicKey {
	/// The algorithm it signs with.
	pub algorithm: PairingKeyAlgorithm,
	/// The key itself, as 64 lowercase hexadecimal characters.
	#[serde(with = "crate::hex")]
	pub key: [u8; 32],
}

/// The one-time secret as the Plane hands it to the owner who opened the
/// offer. It is disclosed once: a retry of the Command that opened the
/// offer is answered without it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disclosure", rename_all = "snake_case")]
pub enum PairingDisclosure {
	/// The eight-digit code, grouped as `xxxx-yyyy`.
	ManualCode {
		/// The code to read out.
		code: String,
	},
	/// The versioned payload to render as a QR code.
	QrPayload {
		/// The payload to render.
		payload: String,
	},
	/// The Plane already disclosed this offer's secret. Open another offer
	/// to be given one.
	AlreadyDisclosed,
}

/// Why a Pairing offer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingEnd {
	/// Its window passed.
	Expired,
	/// Too many wrong secrets were presented against it.
	TooManyAttempts,
	/// The owner closed the Plane's Pairing gate while it was open.
	GateClosed,
}

/// How far the Plane's one Pairing offer has got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "progress", rename_all = "snake_case")]
pub enum PairingProgress {
	/// Issued, and waiting for a client to present its secret.
	Offered,
	/// A client presented the secret. Both sides now display the
	/// authentication string for the people at each end to compare.
	AwaitingConfirmation {
		/// The Client identity that claimed the offer.
		client_id: Uuid,
		/// What both sides display.
		authentication_string: String,
	},
	/// Over. It can only be replaced by a new offer.
	Ended {
		/// Why it is over.
		reason: PairingEnd,
	},
}

/// The Plane's one Pairing offer, without the secret it was issued with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingPairing {
	/// Durable identity.
	pub offer_id: Uuid,
	/// How its secret reached the person pairing.
	pub method: PairingMethod,
	/// How far it has got.
	pub progress: PairingProgress,
	/// How many wrong secrets it still survives.
	pub attempts_remaining: u32,
	/// When it was opened, in signed Unix milliseconds.
	pub opened_at_unix_ms: i64,
	/// When its current step stops being accepted, in signed Unix
	/// milliseconds.
	pub expires_at_unix_ms: i64,
}

/// One Plane's Pairing as it stands, fenced by a journal cursor
/// (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingSnapshot {
	/// Newest Event sequence visible when the snapshot was read, carried as
	/// a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// Whether a new GUI client may begin Pairing.
	pub gate: PairingGate,
	/// The offer the Plane has open, if any.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub pending: Option<PendingPairing>,
}
