//! Typed rows exchanged with the store. Enum variants carry their durable
//! column spelling, which also serves as their JSON spelling.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Whether Jet keeps a Conversation after its final Run (ADR-0001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retention {
	/// Keep the Conversation and its history indefinitely. The default.
	Retain,
	/// Forget the Conversation once it has no active Run and no other
	/// protected state.
	ForgetAfterFinalRun,
}

/// Mutually exclusive lifecycle of one Run (ADR-0065).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunLifecycle {
	/// Recorded but not yet launching.
	Created,
	/// Launching its Harness.
	Starting,
	/// Executing; activity is reported separately.
	Active,
	/// Ending gracefully.
	Stopping,
	/// Terminal: finished its work.
	Completed,
	/// Terminal: ended with an error.
	Failed,
	/// Terminal: ended on request.
	Canceled,
	/// Terminal: its execution can no longer be observed.
	Lost,
}

impl RunLifecycle {
	/// Whether this lifecycle state is one of the four terminal results.
	#[must_use]
	pub fn is_terminal(self) -> bool {
		match self {
			Self::Created | Self::Starting | Self::Active | Self::Stopping => {
				false
			}
			Self::Completed | Self::Failed | Self::Canceled | Self::Lost => {
				true
			}
		}
	}
}

/// Who caused an Event (ADR-0063).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorRecord {
	/// An interactive GUI client identified by its durable Client identity.
	InteractiveClient {
		/// The client's durable identity.
		client_id: Uuid,
	},
}

/// A Conversation to insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewConversation {
	/// Globally unique identity chosen by the caller.
	pub conversation_id: Uuid,
	/// Retention choice.
	pub retention: Retention,
}

/// Current state of one Conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationRecord {
	/// Globally unique identity.
	pub conversation_id: Uuid,
	/// Retention choice.
	pub retention: Retention,
	/// When the Conversation was recorded.
	pub created_at_unix_ms: i64,
}

/// A Run to insert in the `created` lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewRun {
	/// Globally unique identity chosen by the caller.
	pub run_id: Uuid,
	/// The Conversation this Run executes.
	pub conversation_id: Uuid,
}

/// Current state of one Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunRecord {
	/// Globally unique identity.
	pub run_id: Uuid,
	/// The Conversation this Run executes.
	pub conversation_id: Uuid,
	/// Current lifecycle state.
	pub lifecycle: RunLifecycle,
	/// When the Run was recorded.
	pub created_at_unix_ms: i64,
	/// When the Run reached a terminal state, if it has.
	pub ended_at_unix_ms: Option<i64>,
}

/// An Event to append to the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
	/// Globally unique identity chosen by the caller.
	pub event_id: Uuid,
	/// Who caused the Event.
	pub actor: ActorRecord,
	/// The Conversation the Event concerns, if any.
	pub conversation_id: Option<Uuid>,
	/// The Run the Event concerns, if any.
	pub run_id: Option<Uuid>,
	/// Indexed kind such as `run.lifecycle_changed`.
	pub kind: String,
	/// Version of the payload schema for `kind`.
	pub payload_version: u32,
	/// Bounded JSON payload.
	pub payload: String,
}

/// One journal row (ADR-0096).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
	/// Plane-local monotonic position; never reused (ADR-0069).
	pub sequence: u64,
	/// Globally unique identity.
	pub event_id: Uuid,
	/// Who caused the Event.
	pub actor: ActorRecord,
	/// When the Event was recorded.
	pub recorded_at_unix_ms: i64,
	/// The Conversation the Event concerns, if any.
	pub conversation_id: Option<Uuid>,
	/// The Run the Event concerns, if any.
	pub run_id: Option<Uuid>,
	/// Indexed kind such as `run.lifecycle_changed`.
	pub kind: String,
	/// Version of the payload schema for `kind`.
	pub payload_version: u32,
	/// Bounded JSON payload.
	pub payload: String,
}

/// Reports a column whose stored text no longer parses. The store maps
/// it to [`crate::StoreError::Integrity`] like any other conversion
/// failure.
pub(crate) fn column_error(index: usize, message: String) -> rusqlite::Error {
	rusqlite::Error::FromSqlConversionFailure(
		index,
		rusqlite::types::Type::Text,
		message.into(),
	)
}

pub(crate) fn parse_uuid(index: usize, text: &str) -> rusqlite::Result<Uuid> {
	Uuid::parse_str(text).map_err(|error| {
		column_error(index, format!("column {index} is not a UUID: {error}"))
	})
}

pub(crate) fn parse_optional_uuid(
	index: usize,
	text: Option<&str>,
) -> rusqlite::Result<Option<Uuid>> {
	text.map(|text| parse_uuid(index, text)).transpose()
}
