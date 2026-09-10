//! Conversation and Run name Commands (ADR-0044, ADR-0066).

use crate::{
	connection::{Client, ClientError},
	requests::unexpected,
};
use jet_protocol::{CommandRequest, CommandResponse, Conversation, Run};
use uuid::Uuid;

impl Client {
	/// Sets a Conversation's authoritative manual name at an observed Revision.
	///
	/// # Errors
	///
	/// Returns [`ClientError::FeatureUnavailable`] when the negotiated minor
	/// predates names, [`ClientError::Remote`] when the Conversation is unknown,
	/// its Revision is stale, or the name is invalid, and transport failures
	/// otherwise.
	pub async fn set_conversation_name(
		&self,
		command_id: Uuid,
		conversation_id: Uuid,
		expected_revision: u64,
		name: impl Into<String>,
	) -> Result<Conversation, ClientError> {
		self.require_minor(jet_protocol::NAMES_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::SetConversationName {
					conversation_id,
					expected_revision,
					name: name.into(),
				},
			)
			.await?
		{
			CommandResponse::ConversationNamed(conversation) => {
				Ok(conversation)
			}
			other => Err(unexpected(&other)),
		}
	}

	/// Sets a Run's authoritative manual name at an observed Revision.
	///
	/// # Errors
	///
	/// Returns [`ClientError::FeatureUnavailable`] when the negotiated minor
	/// predates names, [`ClientError::Remote`] when the Run is unknown, its
	/// Revision is stale, or the name is invalid, and transport failures
	/// otherwise.
	pub async fn set_run_name(
		&self,
		command_id: Uuid,
		run_id: Uuid,
		expected_revision: u64,
		name: impl Into<String>,
	) -> Result<Run, ClientError> {
		self.require_minor(jet_protocol::NAMES_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::SetRunName {
					run_id,
					expected_revision,
					name: name.into(),
				},
			)
			.await?
		{
			CommandResponse::RunNamed(run) => Ok(run),
			other => Err(unexpected(&other)),
		}
	}
}
