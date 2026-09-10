//! Durable Run lifecycle records.

use super::NameRecord;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

	/// The durable spelling, also used in messages and JSON.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Created => "created",
			Self::Starting => "starting",
			Self::Active => "active",
			Self::Stopping => "stopping",
			Self::Completed => "completed",
			Self::Failed => "failed",
			Self::Canceled => "canceled",
			Self::Lost => "lost",
		}
	}

	pub(crate) fn parse(text: &str) -> Option<Self> {
		[
			Self::Created,
			Self::Starting,
			Self::Active,
			Self::Stopping,
			Self::Completed,
			Self::Failed,
			Self::Canceled,
			Self::Lost,
		]
		.into_iter()
		.find(|lifecycle| lifecycle.as_str() == text)
	}
}

/// A Run to insert in the `created` lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewRun {
	/// Globally unique identity chosen by the caller.
	pub run_id: Uuid,
	/// The Conversation this Run executes.
	pub conversation_id: Uuid,
	/// When the caller recorded the Run.
	pub created_at_unix_ms: i64,
}

/// Current state of one Run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
	/// Globally unique identity.
	pub run_id: Uuid,
	/// The Conversation this Run executes.
	pub conversation_id: Uuid,
	/// Monotonic version for conflict-sensitive Run Commands.
	pub revision: u64,
	/// Current lifecycle state.
	pub lifecycle: RunLifecycle,
	/// Resolved user-facing name and its authority.
	pub name: NameRecord,
	/// When the Run was recorded.
	pub created_at_unix_ms: i64,
	/// When the Run reached a terminal state, if it has.
	pub ended_at_unix_ms: Option<i64>,
}
