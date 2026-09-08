//! Turn queue reads through the typed client boundary.
use crate::connection::{Client, ClientError};
use crate::requests::unexpected;
use jet_protocol::{QueryRequest, QueryResponse};
use uuid::Uuid;
impl Client {
	/// Reads the authoritative Turn queue and Event fence (protocol minor 16).
	/// Vector order is queue position; admission identity is stable as it moves.
	///
	/// # Errors
	/// Returns a feature, transport, or stable remote error.
	pub async fn turn_queue(
		&self,
		conversation_id: Uuid,
	) -> Result<jet_protocol::TurnQueue, ClientError> {
		self.require_minor(jet_protocol::TURN_QUEUE_MINOR)?;
		match self
			.query(QueryRequest::TurnQueue { conversation_id })
			.await?
		{
			QueryResponse::TurnQueue(queue) => Ok(queue),
			other @ (QueryResponse::ScheduledTasks(_)
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
