//! Explicit cross-Harness continuation (ADR-0021).
use crate::{Client, ClientError};
use jet_protocol::{
	CommandRequest, CommandResponse, Conversation, HandoffRequest,
};
use uuid::Uuid;

impl Client {
	/// Creates a separate Workspace and admits its first Run on the selected
	/// Harness using a bounded package captured from the source's current work.
	///
	/// # Errors
	/// Returns feature-unavailable for an older daemon, a remote refusal for
	/// invalid or oversized selections, or a transport error.
	pub async fn handoff_conversation(
		&self,
		command_id: Uuid,
		request: HandoffRequest,
	) -> Result<Conversation, ClientError> {
		self.require_minor(jet_protocol::HANDOFFS_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::HandoffConversation(request),
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
