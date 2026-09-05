//! Wire form of Pairing: how a GUI client comes to control a Plane
//! (ADR-0017).
//!
//! The Pairing gate is Plane-level and concerns new clients only. A Plane
//! reports where its owner left it; the clients that are already Paired are
//! unaffected by it in either direction.

use serde::{Deserialize, Serialize};

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

/// One Plane's Pairing as it stands, fenced by a journal cursor
/// (ADR-0092).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingSnapshot {
	/// Newest Event sequence visible when the snapshot was read, carried as
	/// a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// Whether a new GUI client may begin Pairing.
	pub gate: PairingGate,
}
