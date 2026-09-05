//! Control messages exchanged after the handshake.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use uuid::Uuid;

use crate::account::AccountBindingList;
use crate::audit::{SecurityAudit, SecurityState};
use crate::capability::{CapabilityObservation, CapabilitySnapshot};
use crate::control::{ControlError, decode_control};
use crate::conversation::{
	CommandRequest, CommandResponse, ConversationList, ConversationSnapshot,
	PageCursor, RevisionConflict,
};
use crate::event::Event;
use crate::pairing::PairingSnapshot;
use crate::project::{ProjectList, ProjectPreview};
use crate::setting::{SettingScope, SettingSelection, SettingSnapshot};

/// Correlates a client request with its server reply.
pub type RequestId = u64;

/// Control message sent by a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientMessage {
	/// Run a Query and return its snapshot.
	Query {
		/// Correlation identifier echoed in the reply.
		id: RequestId,
		/// The Query to run.
		query: QueryRequest,
	},
	/// Execute a Command and return its durable outcome.
	Command {
		/// Correlation identifier echoed in the reply.
		id: RequestId,
		/// Actor-scoped identity used to deduplicate retries.
		command_id: Uuid,
		/// The Command to execute.
		command: CommandRequest,
	},
}

/// The exact bytes of the `command` object inside an encoded
/// [`ClientMessage::Command`] frame. `jetd` digests them before interpreting
/// the Command, so only a byte-equivalent retry reuses a durable outcome
/// (ADR-0093). Decode the typed message first; this reads nothing else.
///
/// # Errors
///
/// Returns [`ControlError::Malformed`] when the frame has no `command`
/// object.
pub fn raw_command(frame: &[u8]) -> Result<Box<RawValue>, ControlError> {
	#[derive(Deserialize)]
	struct CommandBytes {
		command: Box<RawValue>,
	}
	let CommandBytes { command } = decode_control(frame)?;
	Ok(command)
}

/// Control message sent by `jetd`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerMessage {
	/// Successful Query snapshot.
	QueryResult {
		/// Identifier of the request being answered.
		id: RequestId,
		/// The snapshot.
		result: QueryResponse,
	},
	/// Durable Command outcome.
	CommandResult {
		/// Identifier of the request being answered.
		id: RequestId,
		/// The outcome.
		result: CommandResponse,
	},
	/// A request failed, or the connection is being refused.
	Error {
		/// Identifier of the request that failed, if any.
		id: Option<RequestId>,
		/// Stable error body.
		error: WireError,
	},
}

/// Queries a client may run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueryRequest {
	/// Snapshot of the Plane's daemon status.
	Status,
	/// First bounded page of Conversations on the Plane.
	Conversations,
	/// Continue a fenced Conversation keyset snapshot.
	NextConversations {
		/// Opaque token returned by the previous page.
		cursor: PageCursor,
	},
	/// One Conversation with all of its Runs.
	Conversation {
		/// The Conversation to read.
		conversation_id: Uuid,
	},
	/// What the Plane can do.
	Capabilities {
		/// Whether to report the last observation or take a new one.
		observation: CapabilityObservation,
	},
	/// Every Account binding on the Plane, with the state of the Credential
	/// each one resolves.
	AccountBindings {
		/// Whether the Credential states follow the last observation of the
		/// Plane or a new one, taken now.
		observation: CapabilityObservation,
	},
	/// Settings resolved for one scope.
	Settings {
		/// The scope to resolve for; its own values win over the Plane's.
		scope: SettingScope,
		/// Which Settings to resolve.
		selection: SettingSelection,
	},
	/// A page of journal Events strictly after a sequence.
	Events {
		/// The sequence to resume after, carried as a decimal string
		/// (ADR-0089); `"0"` for the whole journal.
		#[serde(with = "crate::decimal")]
		after: u64,
	},
	/// The Plane's Pairing: whether it accepts new GUI clients.
	Pairing,
	/// A page of the owner-only Security audit strictly after a position.
	SecurityAudit {
		/// The position to resume after, carried as a decimal string
		/// (ADR-0089); `"0"` for the whole audit.
		#[serde(with = "crate::decimal")]
		after: u64,
	},
	/// Every registered Project on the Plane.
	Projects,
	/// What registering the Git working tree at an absolute path would
	/// record, before the Path grant is made: the directory it resolves to
	/// and what the Plane's Git says about it.
	PreviewProject {
		/// The absolute path the user is about to grant.
		path: String,
		/// Whether Git LFS is reported from the last observation of the
		/// Plane or a new one, taken now.
		observation: CapabilityObservation,
	},
}

/// Query snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueryResponse {
	/// Snapshot of the Plane's daemon status.
	Status(PlaneStatus),
	/// One page of Conversations on the Plane.
	Conversations(ConversationList),
	/// One Conversation with all of its Runs.
	Conversation(ConversationSnapshot),
	/// What the Plane can do.
	Capabilities(CapabilitySnapshot),
	/// Every Account binding on the Plane.
	AccountBindings(AccountBindingList),
	/// Settings resolved for one scope.
	Settings(SettingSnapshot),
	/// One page of journal Events in sequence order.
	Events(EventPage),
	/// The Plane's Pairing as it stands.
	Pairing(PairingSnapshot),
	/// One page of the Security audit, oldest first.
	SecurityAudit(SecurityAudit),
	/// Every registered Project on the Plane.
	Projects(ProjectList),
	/// What a Path grant would register.
	ProjectPreview(ProjectPreview),
}

/// One page of journal Events, fenced by the journal position it was read
/// at (ADR-0092). The page is the last one when its final Event's sequence
/// equals `cursor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventPage {
	/// Newest Event sequence in the journal when the page was read, carried
	/// as a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// The Events strictly after the requested position, in sequence order.
	pub events: Vec<Event>,
}

/// Wire form of the Plane status snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaneStatus {
	/// Newest Event sequence visible when the status was read, carried as a
	/// decimal string (ADR-0089). Absent only on negotiated minor zero.
	#[serde(
		default,
		skip_serializing_if = "Option::is_none",
		with = "crate::decimal::optional"
	)]
	pub cursor: Option<u64>,
	/// Durable identity of the Plane, created when its store was created.
	pub plane_id: Uuid,
	/// How many times an authoritative `jetd` has started on this Plane.
	pub daemon_starts: u64,
	/// When the current `jetd` started, in signed Unix milliseconds.
	pub started_at_unix_ms: i64,
	/// Version of the running core.
	pub core_version: String,
	/// Whether the Plane can vouch for its own Security audit. Absent on a
	/// minor that does not name the Security audit.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub security: Option<SecurityState>,
}

/// Stable error categories exposed to clients (ADR-0068).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
	/// The request was malformed or violated a precondition.
	InvalidInput,
	/// The Actor may not perform the request.
	Unauthorized,
	/// The request conflicts with current state.
	Conflict,
	/// A required resource is temporarily unavailable.
	Unavailable,
	/// The peers cannot agree on a protocol, codec, or version.
	Incompatible,
	/// The request was throttled.
	RateLimited,
	/// The addressed resource does not exist.
	NotFound,
	/// The result of external work could not be established.
	OutcomeUnknown,
	/// An unexpected internal failure.
	Internal,
}

/// Stable error body carried by every failed reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireError {
	/// Category the client may branch on.
	pub category: ErrorCategory,
	/// Domain-specific code such as `protocol.unsupported_version`.
	pub code: String,
	/// Whether repeating the same request may succeed.
	pub retryable: bool,
	/// Safe human-readable description free of native error strings.
	pub message: String,
	/// Current resource state when an expected Revision was stale.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub revision_conflict: Option<RevisionConflict>,
	/// Structured metadata when a stale read must restart.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub restart: Option<RestartMetadata>,
	/// Structured actions that can safely recover from this error.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub recovery_actions: Vec<RecoveryAction>,
}

/// Structured action a client may take to recover from an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecoveryAction {
	/// Refresh current Run state before preparing another Command.
	RefreshRun {
		/// Run whose current state should be queried.
		run_id: Uuid,
	},
	/// Reconnect and resume the semantic Event stream after this cursor.
	ResumeEvents {
		/// Last Event cursor the disconnected client received completely.
		#[serde(with = "crate::decimal")]
		after: u64,
	},
}

/// Stable metadata explaining why a snapshot must be restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum RestartMetadata {
	/// Required Event replay is no longer retained.
	CursorExpired {
		/// Oldest cursor from which continuous replay remains possible.
		#[serde(with = "crate::decimal")]
		minimum_available_cursor: u64,
		/// Current Event high-water cursor for a replacement snapshot.
		#[serde(with = "crate::decimal")]
		current_snapshot_revision: u64,
	},
	/// The supplied Event cursor belongs to a later or different Plane.
	CursorAhead {
		/// Current Event high-water cursor for the replacement snapshot.
		#[serde(with = "crate::decimal")]
		current_snapshot_revision: u64,
	},
	/// A later page no longer belongs to the current projection state.
	PaginationStale {
		/// Current Event high-water cursor for the replacement first page.
		#[serde(with = "crate::decimal")]
		current_snapshot_revision: u64,
	},
}

#[cfg(test)]
#[path = "message_tests.rs"]
mod tests;
