//! Connection preface and restricted handshake messages (ADR-0090).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::message::WireError;

/// Fixed bytes every client sends before its first control frame.
pub const PREFACE: &[u8] = b"jet-protocol\n";
/// The only protocol major this crate speaks.
pub const PROTOCOL_VERSION: u32 = 1;
/// The only v1 codec; other codecs are reserved for later negotiation.
pub const CODEC_JSON_V1: &str = "json-v1";

/// Inclusive range of protocol majors a client can speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRange {
	/// Lowest supported major.
	pub min: u32,
	/// Highest supported major.
	pub max: u32,
}

impl VersionRange {
	/// Whether `version` falls within the range.
	#[must_use]
	pub fn contains(self, version: u32) -> bool {
		self.min <= version && version <= self.max
	}
}

/// First control frame from a client, sent right after the preface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
	/// Protocol majors the client can speak.
	pub protocol: VersionRange,
	/// Requested codec name.
	pub codec: String,
	/// Durable Client identity of the connecting installation.
	pub client_id: Uuid,
	/// Largest control frame the client is willing to receive.
	pub max_control_frame: u32,
	/// Largest data frame the client is willing to receive.
	pub max_data_frame: u32,
	/// Protocol capability flags the client supports. No flags are defined
	/// in v1; compatible minors may introduce them.
	#[serde(default)]
	pub capabilities: Vec<String>,
}

/// Server reply to a [`ClientHello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerHello {
	/// The connection is authenticated and negotiated.
	Welcome {
		/// Selected protocol major.
		protocol: u32,
		/// Selected codec.
		codec: String,
		/// Negotiated control frame limit both peers honor when sending.
		max_control_frame: u32,
		/// Negotiated data frame limit both peers honor when sending.
		max_data_frame: u32,
		/// Protocol capability flags the server supports.
		#[serde(default)]
		capabilities: Vec<String>,
	},
	/// The server refuses the connection; it closes the stream afterwards.
	Rejected {
		/// Why the handshake failed.
		error: WireError,
	},
}
