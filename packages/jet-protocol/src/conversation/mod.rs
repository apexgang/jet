//! Wire form of Conversations and Runs, and the Commands clients execute.

mod response;
pub use response::CommandResponse;

mod command;
pub use command::CommandRequest;

pub(crate) mod auto_continue;
pub(crate) mod checkpoint;
pub(crate) mod event;
pub(crate) mod git_delivery;
pub(crate) mod handoff;
pub(crate) mod import;
pub(crate) mod name;
pub(crate) mod promotion;
pub(crate) mod run;
pub(crate) mod schedule;
pub(crate) mod turn;
pub(crate) mod user_input;
pub(crate) mod workspace;

use crate::conversation::{
	import::ConversationOrigin,
	workspace::{WorkingTree, Workspace},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Opaque token for continuing one fenced keyset snapshot page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct PageCursor(pub Uuid);

/// Whether Jet keeps a Conversation after its final Run.
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Conversation {
	/// Durable identity.
	pub conversation_id: Uuid,
	/// Current version for conflict-sensitive Commands. Absent before minor 20.
	#[serde(
		default,
		skip_serializing_if = "Option::is_none",
		with = "crate::transport::decimal::optional"
	)]
	#[cfg_attr(feature = "schema", schemars(with = "crate::OptionalDecimal"))]
	pub revision: Option<u64>,
	/// Retention choice.
	pub retention: RetentionPolicy,
	/// Where it does its work. Absent before protocol minor 9.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub working_tree: Option<WorkingTree>,
	/// Where it came from. Absent before protocol minor 13.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub origin: Option<ConversationOrigin>,
	/// Resolved name and its authority. Absent before protocol minor 20.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub name: Option<crate::Name>,
	/// When it was created, in signed Unix milliseconds.
	pub created_at_unix_ms: i64,
}

/// One Run of a Conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Run {
	/// Durable identity.
	pub run_id: Uuid,
	/// The Conversation it executes.
	pub conversation_id: Uuid,
	/// Monotonic version used by conflict-sensitive Commands, carried as a
	/// decimal string (ADR-0089).
	#[serde(with = "crate::transport::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub revision: u64,
	/// Current lifecycle state.
	pub lifecycle: RunLifecycle,
	/// Resolved name and its authority. Absent before protocol minor 20.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub name: Option<crate::Name>,
	/// When it was created, in signed Unix milliseconds.
	pub created_at_unix_ms: i64,
	/// When it reached a terminal state, if it has.
	pub ended_at_unix_ms: Option<i64>,
}

/// One bounded page of Conversations, fenced by a journal cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConversationList {
	/// Newest Event sequence visible when the list was read, carried as a
	/// decimal string (ADR-0089).
	#[serde(with = "crate::transport::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub cursor: u64,
	/// Conversations in creation order.
	pub conversations: Vec<Conversation>,
	/// Opaque continuation token when another page belongs to this snapshot.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub next_page: Option<PageCursor>,
}

/// One Conversation with all of its Runs, fenced by a journal cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConversationSnapshot {
	/// Newest Event sequence visible when the snapshot was read, carried as
	/// a decimal string (ADR-0089).
	#[serde(with = "crate::transport::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub cursor: u64,
	/// The Conversation itself.
	pub conversation: Conversation,
	/// The Workspace it owns, when it works in one. Absent before protocol
	/// minor 9.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub workspace: Option<Workspace>,
	/// Its Runs in creation order, terminal ones included.
	pub runs: Vec<Run>,
}

/// Structured state returned when a Revision precondition is stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RevisionConflict {
	/// Revision that is authoritative now, carried as a decimal string
	/// (ADR-0089).
	#[serde(with = "crate::transport::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub current_revision: u64,
	/// Safe current state with which the caller can refresh.
	pub safe_state: ConflictState,
}

/// Safe resource state attached to a Revision conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConflictState {
	/// The current Conversation.
	Conversation {
		/// Complete safe Conversation state.
		conversation: Conversation,
	},
	/// The current Run.
	Run {
		/// Complete safe Run state.
		run: Run,
	},
}
