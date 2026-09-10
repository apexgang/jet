//! Auto-continue policy Commands and fenced Queries.
use crate::connection::{Client, ClientError};
use crate::requests::unexpected;
use jet_protocol::{QueryRequest, QueryResponse};
use uuid::Uuid;
impl Client {
	/// Reads the durable policy and latest retry decision (protocol minor 34).
	///
	/// # Errors
	/// Returns a feature, transport, or stable remote error.
	pub async fn auto_continue(
		&self,
		target: jet_protocol::AutoContinueTarget,
	) -> Result<jet_protocol::AutoContinueSnapshot, ClientError> {
		self.require_minor(jet_protocol::AUTO_CONTINUE_MINOR)?;
		match self.query(QueryRequest::AutoContinue { target }).await? {
			QueryResponse::AutoContinue(snapshot) => Ok(snapshot),
			other @ (QueryResponse::GitDeliveries { .. }
			| QueryResponse::ExtensionCatalog(_)
			| QueryResponse::ExtensionChange(_)
			| QueryResponse::RemoteToolReview(_)
			| QueryResponse::Utility(_)
			| QueryResponse::CraftInstallationPreview(_)
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

impl Client {
	/// Set an Account-binding default or a one-shot Conversation override.
	/// # Errors
	/// Returns a feature, transport, or stable remote error.
	pub async fn set_auto_continue(
		&self,
		command_id: Uuid,
		target: jet_protocol::AutoContinueTarget,
		policy: jet_protocol::AutoContinuePolicy,
	) -> Result<(), ClientError> {
		self.require_minor(jet_protocol::AUTO_CONTINUE_MINOR)?;
		let response = self
			.execute_command(
				command_id,
				jet_protocol::CommandRequest::SetAutoContinue {
					target,
					policy,
				},
			)
			.await?;
		if response == jet_protocol::CommandResponse::AutoContinueConfigured {
			Ok(())
		} else {
			Err(unexpected(&response))
		}
	}
}
