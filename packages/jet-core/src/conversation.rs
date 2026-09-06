//! Conversations and their Runs as the core sees them (ADR-0001,
//! ADR-0065).

use std::time::SystemTime;

use jet_store::{ConversationRecord, RetentionPolicy, RunLifecycle, RunRecord};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::EventSequence;
use crate::system_time;
use crate::workspace::{WorkingTree, Workspace};

/// Durable identity of one Conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationId(pub Uuid);

/// Opaque token for continuing one fenced keyset snapshot page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageCursor(pub Uuid);

/// Durable identity of one Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub Uuid);

/// Monotonic version of a conflict-sensitive resource.
#[derive(
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
	Serialize,
	Deserialize,
)]
pub struct Revision(pub u64);

/// A logical interaction between a user and a Harness. It exists before
/// its first Run and outlives every Run it has had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
	/// Durable identity.
	pub conversation_id: ConversationId,
	/// Whether Jet keeps the Conversation after its final Run.
	pub retention: RetentionPolicy,
	/// Where it does its work (ADR-0025).
	pub working_tree: WorkingTree,
	/// When the Conversation was created.
	pub created_at: SystemTime,
}

/// One bounded execution of a Conversation on this Plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
	/// Durable identity.
	pub run_id: RunId,
	/// The Conversation this Run executes.
	pub conversation_id: ConversationId,
	/// Current version for conflict-sensitive Run Commands.
	pub revision: Revision,
	/// Current lifecycle state.
	pub lifecycle: RunLifecycle,
	/// When the Run was created.
	pub created_at: SystemTime,
	/// When the Run reached a terminal state, if it has.
	pub ended_at: Option<SystemTime>,
}

/// One bounded page of Conversations, fenced by the journal position at
/// which its first page was read (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationList {
	/// Newest Event sequence visible when the list was read.
	pub cursor: EventSequence,
	/// Conversations in creation order.
	pub conversations: Vec<Conversation>,
	/// Opaque continuation token when another page belongs to this snapshot.
	pub next_page: Option<PageCursor>,
}

/// One Conversation with all of its Runs, fenced by the journal position
/// it was read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSnapshot {
	/// Newest Event sequence visible when the snapshot was read.
	pub cursor: EventSequence,
	/// The Conversation itself.
	pub conversation: Conversation,
	/// The Workspace it owns, when it works in one.
	pub workspace: Option<Workspace>,
	/// Its Runs in creation order, terminal ones included.
	pub runs: Vec<Run>,
}

impl From<ConversationRecord> for Conversation {
	fn from(record: ConversationRecord) -> Self {
		Self {
			conversation_id: ConversationId(record.conversation_id),
			retention: record.retention,
			working_tree: record.working_tree.into(),
			created_at: system_time(record.created_at_unix_ms),
		}
	}
}

impl From<RunRecord> for Run {
	fn from(record: RunRecord) -> Self {
		Self {
			run_id: RunId(record.run_id),
			conversation_id: ConversationId(record.conversation_id),
			revision: Revision(record.revision),
			lifecycle: record.lifecycle,
			created_at: system_time(record.created_at_unix_ms),
			ended_at: record.ended_at_unix_ms.map(system_time),
		}
	}
}
