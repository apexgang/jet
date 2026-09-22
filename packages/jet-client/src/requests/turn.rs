//! Turn queue reads through the typed client boundary.
use crate::{
	connection::{Client, ClientError},
	requests::unexpected,
};
use jet_protocol::{
	CommandRequest, CommandResponse, QueryRequest, QueryResponse, Turn,
	TurnSource,
};
use uuid::Uuid;

impl Client {
	/// Admits one Turn to the authoritative Conversation queue. The Plane
	/// assigns its order; a retry must reuse `command_id` and the exact body.
	///
	/// # Errors
	/// Returns a stable validation, queue, authorization, or transport error.
	pub async fn submit_turn(
		&self,
		command_id: Uuid,
		conversation_id: Uuid,
		source: TurnSource,
		prompt: &str,
	) -> Result<Turn, ClientError> {
		self.require_minor(jet_protocol::TURN_QUEUE_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::SubmitTurn {
					conversation_id,
					source,
					prompt: prompt.into(),
				},
			)
			.await?
		{
			CommandResponse::TurnAdmitted { turn } => Ok(turn),
			other => Err(unexpected(&other)),
		}
	}

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
			other @ (QueryResponse::GitDeliveries { .. }
			| QueryResponse::ExtensionCatalog(_)
			| QueryResponse::ExtensionChange(_)
			| QueryResponse::RemoteToolReview(_)
			| QueryResponse::Utility(_)
			| QueryResponse::CraftInstallationPreview(_)
			| QueryResponse::AutoContinue(_)
			| QueryResponse::ScheduledTasks(_)
			| QueryResponse::ConversationTrash(_)
			| QueryResponse::RetentionPreview(_)
			| QueryResponse::AutodeleteRules(_)
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
			| QueryResponse::UsageHistory(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)
			| QueryResponse::Projects(_)
			| QueryResponse::ProjectPreview(_)
			| QueryResponse::ProjectRemovalPreview(_)
			| QueryResponse::ProjectEntry(_)
			| QueryResponse::PromotionPreview(_)
			| QueryResponse::RunExecution(_)
			| QueryResponse::Search(_)
			| QueryResponse::ExternalConversations(_)) => Err(unexpected(&other)),
		}
	}

	/// Withdraws one queued user Turn admitted by this authenticated client.
	/// A retry must reuse `command_id`, `conversation_id`, and `turn_id`.
	///
	/// # Errors
	/// Returns a stable ownership, state, validation, or transport error.
	pub async fn withdraw_turn(
		&self,
		command_id: Uuid,
		conversation_id: Uuid,
		turn_id: Uuid,
	) -> Result<Turn, ClientError> {
		self.require_minor(jet_protocol::TURN_QUEUE_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::WithdrawTurn {
					conversation_id,
					turn_id,
				},
			)
			.await?
		{
			CommandResponse::TurnWithdrawn { turn } => Ok(turn),
			other => Err(unexpected(&other)),
		}
	}
}
