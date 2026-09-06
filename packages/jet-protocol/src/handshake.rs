//! Connection preface and restricted handshake messages (ADR-0090). The
//! handshake negotiates protocol major and minor, codec, frame limits, and
//! capabilities before any Plane state is exposed (ADR-0019).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::message::WireError;

/// Fixed bytes every client sends before its first control frame.
pub const PREFACE: &[u8] = b"jet-protocol\n";
/// The only protocol major this crate speaks.
pub const PROTOCOL_VERSION: u32 = 1;
/// The newest minor of [`PROTOCOL_VERSION`] this crate speaks. Minors are
/// additive: a peer negotiated to a lower minor never sees fields it does
/// not know (ADR-0019).
pub const PROTOCOL_MINOR: u32 = 14;
/// Minor that introduced managed Run admission and execution snapshots.
pub const MANAGED_RUNS_MINOR: u32 = 13;
/// Minor that introduced fresh signed remote connection challenges.
pub const REMOTE_AUTH_MINOR: u32 = 7;
/// Minor that introduced fenced status and Conversation pagination.
pub const FENCED_READS_MINOR: u32 = 1;
/// Minor that switches post-handshake frames to numbered stream envelopes.
pub const MULTIPLEXED_STREAMS_MINOR: u32 = 2;
/// Minor that introduced Setting Queries and Commands and the Capability
/// Query.
pub const SETTINGS_AND_CAPABILITIES_MINOR: u32 = 3;
/// Minor that introduced Account binding Queries and Commands.
pub const ACCOUNT_BINDINGS_MINOR: u32 = 4;
/// Minor that introduced the owner-only Security audit Query.
pub const SECURITY_AUDIT_MINOR: u32 = 5;
/// Minor that introduced the Pairing Query and Commands.
pub const PAIRING_MINOR: u32 = 6;
/// Minor that introduced Project registration, the Project Queries, and
/// the Git LFS external tool in Capability snapshots.
pub const PROJECTS_MINOR: u32 = 8;
/// Minor that introduced Workspace and Local-checkout Conversations, the
/// working tree on every Conversation, and the Workspace in a Conversation
/// snapshot.
pub const WORKSPACES_MINOR: u32 = 9;
/// Minor that introduced seeding a Workspace from selected Local-checkout
/// changes, and the seed on a Workspace.
pub const SEEDED_WORKSPACES_MINOR: u32 = 10;
/// Minor that introduced previewing and confirming a Workspace promotion,
/// and the promotion on a Workspace.
pub const WORKSPACE_PROMOTION_MINOR: u32 = 11;
/// Minor that introduced the Search Query over Plane-local Conversation
/// content.
pub const SEARCH_MINOR: u32 = 12;
/// Minor that introduced external Conversation discovery, imports, managed
/// Resume, and the origin on every Conversation.
pub const IMPORTED_CONVERSATIONS_MINOR: u32 = 14;
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
	/// Newest minor of the `max` major the client speaks (ADR-0019).
	pub minor: u32,
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
	/// Remote endpoint access is established; Jet authorization is still pending.
	Challenge {
		/// A fresh 256-bit nonce, used once on this connection.
		#[serde(with = "crate::hex")]
		nonce: [u8; 32],
	},
	/// The connection is authenticated and negotiated.
	Welcome {
		/// Selected protocol major.
		protocol: u32,
		/// Selected minor of that major: the smaller of the two peers' newest
		/// minors, so neither side sends fields the other cannot read.
		minor: u32,
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
