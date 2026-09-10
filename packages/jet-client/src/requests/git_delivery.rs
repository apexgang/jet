//! Durable Git delivery results through the typed client boundary.
use crate::{
	connection::{Client, ClientError},
	requests::unexpected,
};
use jet_protocol::{QueryRequest, QueryResponse};
use uuid::Uuid;

impl Client {
	/// Reads the latest 100 Git delivery operations (protocol minor 35).
	/// Results contain data only; this method grants no execution authority.
	///
	/// # Errors
	/// Returns a feature, transport, or stable remote error.
	pub async fn git_deliveries(
		&self,
		conversation_id: Uuid,
	) -> Result<Vec<jet_protocol::GitDelivery>, ClientError> {
		self.require_minor(jet_protocol::GIT_DELIVERY_MINOR)?;
		match self
			.query(QueryRequest::GitDeliveries { conversation_id })
			.await?
		{
			QueryResponse::GitDeliveries { deliveries } => Ok(deliveries),
			other @ (QueryResponse::Utility(_)
			| QueryResponse::ExtensionCatalog(_)
			| QueryResponse::ExtensionChange(_)
			| QueryResponse::RemoteToolReview(_)
			| QueryResponse::CraftInstallationPreview(_)
			| QueryResponse::TurnQueue(_)
			| QueryResponse::AutoContinue(_)
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
			| QueryResponse::Usage(_)
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
