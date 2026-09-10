//! Checkpoint and patch reads for client protocol minor 17.
use crate::{Client, ClientError};
use jet_protocol::{
	ChangeArtifactChunk, ChangeDiff, DiffScope, QueryRequest, QueryResponse,
};
use uuid::Uuid;
impl Client {
	/// Reads current, per-turn, final, or historical changes of a Run.
	///
	/// # Errors
	/// Returns a compatibility, remote, or transport error.
	pub async fn change_diff(
		&self,
		run_id: Uuid,
		scope: DiffScope,
	) -> Result<Box<ChangeDiff>, ClientError> {
		self.read_change_diff(QueryRequest::ChangeDiff { run_id, scope })
			.await
	}
	/// Continues changed-file metadata using a prior diff's opaque cursor.
	///
	/// # Errors
	/// Returns a compatibility, stale-pagination, remote, or transport error.
	pub async fn next_change_diff(
		&self,
		cursor: jet_protocol::PageCursor,
	) -> Result<Box<ChangeDiff>, ClientError> {
		self.read_change_diff(QueryRequest::NextChangeDiff { cursor })
			.await
	}
	async fn read_change_diff(
		&self,
		query: QueryRequest,
	) -> Result<Box<ChangeDiff>, ClientError> {
		self.require_minor(jet_protocol::CHANGE_CHECKPOINTS_MINOR)?;
		match self.query(query).await? {
			QueryResponse::ChangeDiff(diff) => Ok(diff),
			other @ (QueryResponse::GitDeliveries { .. }
			| QueryResponse::ExtensionCatalog(_)
			| QueryResponse::ExtensionChange(_)
			| QueryResponse::RemoteToolReview(_)
			| QueryResponse::Utility(_)
			| QueryResponse::CraftInstallationPreview(_)
			| QueryResponse::AutoContinue(_)
			| QueryResponse::ScheduledTasks(_)
			| QueryResponse::EditableFile(_)
			| QueryResponse::WorkspaceTerminals { .. }
			| QueryResponse::OrphanedExecutions(_)
			| QueryResponse::TurnQueue(_)
			| QueryResponse::Status(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::Usage(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)
			| QueryResponse::Projects(_)
			| QueryResponse::ProjectPreview(_)
			| QueryResponse::ProjectEntry(_)
			| QueryResponse::PromotionPreview(_)
			| QueryResponse::ChangeArtifact(_)
			| QueryResponse::RunExecution(_)
			| QueryResponse::Search(_)
			| QueryResponse::ExternalConversations(_)) => {
				Err(crate::requests::unexpected(&other))
			}
		}
	}
	/// Reads at most 64 KiB of a patch Artifact. Verify the assembled bytes
	/// against its SHA-256 before applying or exporting a downloaded patch.
	///
	/// # Errors
	/// Returns a compatibility, remote, or transport error.
	pub async fn change_artifact(
		&self,
		sha256: String,
		offset: u64,
	) -> Result<ChangeArtifactChunk, ClientError> {
		self.require_minor(jet_protocol::CHANGE_CHECKPOINTS_MINOR)?;
		match self
			.query(QueryRequest::ChangeArtifact { sha256, offset })
			.await?
		{
			QueryResponse::ChangeArtifact(chunk) => Ok(chunk),
			other @ (QueryResponse::GitDeliveries { .. }
			| QueryResponse::ExtensionCatalog(_)
			| QueryResponse::ExtensionChange(_)
			| QueryResponse::RemoteToolReview(_)
			| QueryResponse::Utility(_)
			| QueryResponse::CraftInstallationPreview(_)
			| QueryResponse::AutoContinue(_)
			| QueryResponse::ScheduledTasks(_)
			| QueryResponse::EditableFile(_)
			| QueryResponse::WorkspaceTerminals { .. }
			| QueryResponse::OrphanedExecutions(_)
			| QueryResponse::TurnQueue(_)
			| QueryResponse::Status(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::Usage(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)
			| QueryResponse::Projects(_)
			| QueryResponse::ProjectPreview(_)
			| QueryResponse::ProjectEntry(_)
			| QueryResponse::PromotionPreview(_)
			| QueryResponse::ChangeDiff(_)
			| QueryResponse::RunExecution(_)
			| QueryResponse::Search(_)
			| QueryResponse::ExternalConversations(_)) => {
				Err(crate::requests::unexpected(&other))
			}
		}
	}
}
