//! Wire form of Conversations, Runs, and their Commands.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Whether Jet keeps a Conversation after its final Run.
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RetentionPolicy {
	/// Keep the Conversation and its history. The default.
	#[default]
	Retain,
	/// Forget the Conversation once it has no live Run and no other
	/// protected state.
	ForgetAfterFinalRun,
}

/// Mutually exclusive lifecycle state of one Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunLifecycle {
	/// Recorded but not yet launching.
	Created,
	/// Launching its Harness.
	Starting,
	/// Executing.
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

/// One Conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
	/// Durable identity.
	pub conversation_id: Uuid,
	/// Retention choice.
	pub retention: RetentionPolicy,
	/// When it was created, in signed Unix milliseconds.
	pub created_at_unix_ms: i64,
}

/// One Run of a Conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
	/// Durable identity.
	pub run_id: Uuid,
	/// The Conversation it executes.
	pub conversation_id: Uuid,
	/// Monotonic version used by conflict-sensitive Commands, carried as a
	/// decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub revision: u64,
	/// Current lifecycle state.
	pub lifecycle: RunLifecycle,
	/// When it was created, in signed Unix milliseconds.
	pub created_at_unix_ms: i64,
	/// When it reached a terminal state, if it has.
	pub ended_at_unix_ms: Option<i64>,
}

/// Every Conversation on the Plane, fenced by a journal cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationList {
	/// Newest Event sequence visible when the list was read, carried as a
	/// decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// Conversations in creation order.
	pub conversations: Vec<Conversation>,
}

/// One Conversation with all of its Runs, fenced by a journal cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSnapshot {
	/// Newest Event sequence visible when the snapshot was read, carried as
	/// a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// The Conversation itself.
	pub conversation: Conversation,
	/// Its Runs in creation order, terminal ones included.
	pub runs: Vec<Run>,
}

/// Commands a client may execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandRequest {
	/// Create a Conversation with no Runs. Retained unless told otherwise.
	CreateConversation {
		/// Retention choice.
		#[serde(default)]
		retention: RetentionPolicy,
	},
	/// Record a new Run of a Conversation that has no live Run.
	CreateRun {
		/// The Conversation to execute.
		conversation_id: Uuid,
	},
	/// Move a Run forward through its lifecycle.
	TransitionRun {
		/// The Run to move.
		run_id: Uuid,
		/// Revision observed when the Command was prepared, carried as a
		/// decimal string (ADR-0089).
		#[serde(with = "crate::decimal")]
		expected_revision: u64,
		/// The state to enter.
		lifecycle: RunLifecycle,
	},
}

/// Durable Command outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandResponse {
	/// The Conversation as created.
	ConversationCreated(Conversation),
	/// The Run as created.
	RunCreated(Run),
	/// The Run after its transition.
	RunTransitioned(Run),
}

/// Structured state returned when a Revision precondition is stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionConflict {
	/// Revision that is authoritative now, carried as a decimal string
	/// (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub current_revision: u64,
	/// Safe current state with which the caller can refresh.
	pub safe_state: ConflictState,
}

/// Safe resource state attached to a Revision conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConflictState {
	/// The current Run.
	Run {
		/// Complete safe Run state.
		run: Run,
	},
}
