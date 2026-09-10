//! Event journal records and verified snapshot coverage.

use super::ActorRecord;
use uuid::Uuid;

/// Whether an Event is durable Conversation history or compactable
/// operational noise (ADR-0078).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventClass {
	/// Semantic history follows its Conversation's retention policy.
	Semantic,
	/// Superseded operational noise may be removed after snapshot coverage.
	Operational,
}

/// Opaque evidence that a durable snapshot covers Events through a sequence.
///
/// Only a write transaction over the durable normalized projection can mint
/// this value; compaction callers cannot substitute an asserted integer
/// (ADR-0078).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedSnapshotCoverage {
	pub(crate) plane_id: Uuid,
	pub(crate) sequence: u64,
}

impl VerifiedSnapshotCoverage {
	pub(crate) fn parts(self) -> (Uuid, u64) {
		(self.plane_id, self.sequence)
	}
}

impl EventClass {
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Semantic => "semantic",
			Self::Operational => "operational",
		}
	}
}

/// An Event to append to the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
	/// Globally unique identity chosen by the caller.
	pub event_id: Uuid,
	/// Who caused the Event.
	pub actor: ActorRecord,
	/// When the caller recorded the Event; display metadata only
	/// (ADR-0069).
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
	/// Retention class governing whether compaction may remove this Event.
	pub class: EventClass,
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
