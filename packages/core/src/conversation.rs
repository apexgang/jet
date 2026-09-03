//! Conversations and their Runs as the core sees them (ADR-0001,
//! ADR-0065).

use std::time::SystemTime;

use jet_store::{ConversationRecord, Retention, RunLifecycle, RunRecord};
use uuid::Uuid;

use crate::event::EventSequence;
use crate::system_time;

/// Durable identity of one Conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConversationId(pub Uuid);

/// Durable identity of one Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RunId(pub Uuid);

/// A logical interaction between a user and a Harness. It exists before
/// its first Run and outlives every Run it has had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conversation {
	/// Durable identity.
	pub conversation_id: ConversationId,
	/// Whether Jet keeps the Conversation after its final Run.
	pub retention: Retention,
	/// When the Conversation was created.
	pub created_at: SystemTime,
}

/// One bounded execution of a Conversation on this Plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
	/// Durable identity.
	pub run_id: RunId,
	/// The Conversation this Run executes.
	pub conversation_id: ConversationId,
	/// Current lifecycle state.
	pub lifecycle: RunLifecycle,
	/// When the Run was created.
	pub created_at: SystemTime,
	/// When the Run reached a terminal state, if it has.
	pub ended_at: Option<SystemTime>,
}

/// Every Conversation on the Plane, fenced by the journal position it was
/// read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationList {
	/// Newest Event sequence visible when the list was read.
	pub cursor: EventSequence,
	/// Conversations in creation order.
	pub conversations: Vec<Conversation>,
}

/// One Conversation with all of its Runs, fenced by the journal position
/// it was read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSnapshot {
	/// Newest Event sequence visible when the snapshot was read.
	pub cursor: EventSequence,
	/// The Conversation itself.
	pub conversation: Conversation,
	/// Its Runs in creation order, terminal ones included.
	pub runs: Vec<Run>,
}

impl From<ConversationRecord> for Conversation {
	fn from(record: ConversationRecord) -> Self {
		Self {
			conversation_id: ConversationId(record.conversation_id),
			retention: record.retention,
			created_at: system_time(record.created_at_unix_ms),
		}
	}
}

impl From<RunRecord> for Run {
	fn from(record: RunRecord) -> Self {
		Self {
			run_id: RunId(record.run_id),
			conversation_id: ConversationId(record.conversation_id),
			lifecycle: record.lifecycle,
			created_at: system_time(record.created_at_unix_ms),
			ended_at: record.ended_at_unix_ms.map(system_time),
		}
	}
}
