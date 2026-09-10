//! Working-tree ownership and durable User-edit intents.

use super::{ActorRecord, column_error};
use crate::StoreError;
use uuid::Uuid;

/// A direct edit durably accepted before its filesystem replacement begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUserEditIntent {
	/// Authenticated client that submitted the Command.
	pub actor: ActorRecord,
	/// Actor-scoped Command identity.
	pub command_id: Uuid,
	/// Digest that prevents identity reuse with different content.
	pub request_digest: [u8; 32],
	/// When the Command was accepted.
	pub recorded_at_unix_ms: i64,
	/// Private, bounded JSON plan needed to finish after restart.
	pub plan: String,
}

/// A pending direct edit reconstructed from its write-ahead intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserEditIntentRecord {
	/// Authenticated client that submitted the Command.
	pub actor: ActorRecord,
	/// Actor-scoped Command identity.
	pub command_id: Uuid,
	/// Original request digest.
	pub request_digest: [u8; 32],
	/// Original acceptance time.
	pub recorded_at_unix_ms: i64,
	/// Private, bounded JSON plan.
	pub plan: String,
}

/// Where a Conversation does its work (ADR-0025).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkingTreeRecord {
	/// In no Project. Nothing on disk belongs to the Conversation.
	NoProject,
	/// In a managed Workspace of a Project, recorded in `workspaces`.
	Workspace {
		/// The Project the Workspace was created from.
		project_id: Uuid,
	},
	/// In the Project's own Local checkout, which Jet does not isolate.
	LocalCheckout {
		/// The Project whose checkout it works in.
		project_id: Uuid,
	},
}

impl WorkingTreeRecord {
	/// The durable spelling of the kind, beside the Project column.
	pub(crate) fn columns(self) -> (&'static str, Option<Uuid>) {
		match self {
			Self::NoProject => ("none", None),
			Self::Workspace { project_id } => ("workspace", Some(project_id)),
			Self::LocalCheckout { project_id } => {
				("local_checkout", Some(project_id))
			}
		}
	}

	pub(crate) fn parse(
		kind: &str,
		project_id: Option<Uuid>,
	) -> Result<Self, StoreError> {
		match (kind, project_id) {
			("none", None) => Ok(Self::NoProject),
			("workspace", Some(project_id)) => {
				Ok(Self::Workspace { project_id })
			}
			("local_checkout", Some(project_id)) => {
				Ok(Self::LocalCheckout { project_id })
			}
			(kind, project_id) => Err(column_error(
				"working_tree",
				format!(
					"working tree {kind:?} with project {project_id:?} is not \
					 a recorded combination"
				),
			)),
		}
	}
}
