//! Conversation-fork requests for client protocol minor 19.

use crate::{Client, ClientError};
use jet_protocol::{CommandRequest, CommandResponse, Conversation};
use uuid::Uuid;

impl Client {
	/// Creates a new Conversation and separate Workspace from one immutable
	/// checkpoint of `source_run_id` (ADR-0035).
	///
	/// # Errors
	///
	/// Returns [`ClientError::FeatureUnavailable`] for an older daemon,
	/// [`ClientError::Remote`] when the source or checkpoint is unavailable,
	/// or the transport failure otherwise.
	pub async fn fork_conversation(
		&self,
		command_id: Uuid,
		source_run_id: Uuid,
		checkpoint_turn: u32,
	) -> Result<Conversation, ClientError> {
		self.require_minor(jet_protocol::CONVERSATION_FORKS_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::ForkConversation {
					source_run_id,
					checkpoint_turn,
				},
			)
			.await?
		{
			CommandResponse::ConversationCreated(conversation) => {
				Ok(conversation)
			}
			other => Err(crate::requests::unexpected(&other)),
		}
	}
}
