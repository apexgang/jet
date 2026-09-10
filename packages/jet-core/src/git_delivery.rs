//! Non-destructive Git delivery vocabulary (ADR-0029, ADR-0067).
use crate::{ConversationId, RunId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The only Git mutations exposed by delivery. Each is also a manual Command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitOperation {
	/// Create and check out a new branch; never replace an existing branch.
	Branch {
		/// Editable proposed name.
		name: String,
	},
	/// Commit the exact retained checkpoint tree.
	Commit,
	/// Push one branch without force, tags, deletion, or configured refspecs.
	Push {
		/// One configured remote name.
		remote: String,
	},
	/// Create or update this Conversation's GitHub draft.
	DraftPullRequest {
		/// One configured GitHub remote name.
		remote: String,
		/// Explicit base branch, or the GitHub repository default.
		base: Option<String>,
	},
}
/// Immutable source of generated Git text and committed content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCheckpoint {
	/// Owning Run.
	pub run_id: RunId,
	/// Turn number, starting at one.
	pub turn: u32,
}
/// Policy resolved at admission and revalidated before each Effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitDeliveryPolicy {
	/// Whether a successful checkpoint admitted this operation.
	pub automatic: bool,
	/// Automatic branch creation.
	pub branch: bool,
	/// Automatic commits.
	pub commit: bool,
	/// Automatic pushes.
	pub push: bool,
	/// Automatic draft creation and updates.
	pub draft_pull_request: bool,
	/// Editable branch prefix.
	pub branch_prefix: String,
}
/// Individually durable outcome; a later failure never rolls it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GitDeliveryOutcome {
	/// Committed request awaiting execution.
	Pending,
	/// Observed Git state after completion.
	Completed {
		/// Resulting commit identity.
		head: String,
		/// Resulting branch; absent for detached commits.
		branch: Option<String>,
		/// Hosted draft URL, when applicable.
		pull_request: Option<String>,
	},
	/// A definite refusal or unsuccessful operation.
	Failed {
		/// Stable, content-free explanation.
		code: String,
	},
	/// Reconciliation could not establish what happened. Never retried.
	OutcomeUnknown,
}
/// Exact generated text used by a commit or draft Effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitMessage {
	/// Commit subject or PR title.
	pub title: String,
	/// Commit or PR body before Jet attribution markers.
	pub body: String,
	/// Stable reason deterministic text replaced Utility inference.
	pub fallback_reason: Option<String>,
}
/// One retained delivery operation, its exact policy, text attribution and outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitDelivery {
	/// Exact bounded text, absent for branch and push operations.
	pub message: Option<GitMessage>,
	/// User who acknowledged an uncertain outcome, without retrying it.
	pub acknowledged_by: Option<crate::ClientId>,
	/// Stable Effect identity.
	pub delivery_id: Uuid,
	/// Owning Conversation.
	pub conversation_id: ConversationId,
	/// Checkpoint supplying content, absent for manual branch and push Commands.
	pub checkpoint: Option<GitCheckpoint>,
	/// Explicit allowlisted operation.
	pub operation: GitOperation,
	/// Exact automation policy at admission.
	pub policy: GitDeliveryPolicy,
	/// Bounded Utility job containing the exact text and inference attribution.
	pub utility_job: Option<Uuid>,
	/// Latest durable outcome.
	pub outcome: GitDeliveryOutcome,
}
