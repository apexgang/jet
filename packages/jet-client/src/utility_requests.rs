//! Durable Utility results through the typed client boundary.
use crate::connection::{Client, ClientError};
use crate::requests::unexpected;
use jet_protocol::{QueryRequest, QueryResponse};
use uuid::Uuid;
impl Client {
	/// Reads the attributed result of a Utility request (protocol minor 22).
	/// Results contain data only; this method grants no execution authority.
	///
	/// # Errors
	/// Returns a feature, transport, or stable remote error.
	pub async fn utility(
		&self,
		job_id: Uuid,
	) -> Result<jet_protocol::UtilityJob, ClientError> {
		self.require_minor(jet_protocol::UTILITY_MINOR)?;
		match self.query(QueryRequest::Utility { job_id }).await? {
			QueryResponse::Utility(job) => Ok(job),
			other @ (QueryResponse::CraftInstallationPreview(_)
			| QueryResponse::TurnQueue(_)
			| QueryResponse::ScheduledTasks(_)
			| QueryResponse::EditableFile(_)
			| QueryResponse::WorkspaceTerminals { .. }
			| QueryResponse::Status(_)
			| QueryResponse::ChangeArtifact(_)
			| QueryResponse::ChangeDiff(_)
			| QueryResponse::OrphanedExecutions(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)
			| QueryResponse::Projects(_)
			| QueryResponse::ProjectPreview(_)
			| QueryResponse::ProjectEntry(_)
			| QueryResponse::PromotionPreview(_)
			| QueryResponse::RunExecution(_)
			| QueryResponse::Search(_)
			| QueryResponse::ExternalConversations(_)) => Err(unexpected(&other)),
		}
	}
}
