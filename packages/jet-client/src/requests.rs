//! The Queries and Commands a client can issue once connected.

use jet_protocol::{
	CommandRequest, CommandResponse, Conversation, ConversationList,
	ConversationSnapshot, EventPage, PageCursor, PlaneStatus, QueryRequest,
	QueryResponse, RetentionPolicy, Run, RunLifecycle,
};
use uuid::Uuid;

use crate::connection::{Client, ClientError};

impl Client {
	/// Runs the status Query and returns the Plane status snapshot.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn status(&self) -> Result<PlaneStatus, ClientError> {
		match self.query(QueryRequest::Status).await? {
			QueryResponse::Status(status)
				if self.negotiated_minor()
					>= jet_protocol::FENCED_READS_MINOR
					&& status.cursor.is_none() =>
			{
				Err(ClientError::Unexpected(
					"jetd omitted the negotiated status fence".into(),
				))
			}
			QueryResponse::Status(status) => Ok(status),
			other @ (QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)) => Err(unexpected(&other)),
		}
	}

	/// Reads the first bounded page of Conversations with its journal fence.
	/// A minor-zero daemon instead returns its legacy complete list.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn conversations(&self) -> Result<ConversationList, ClientError> {
		match self.query(QueryRequest::Conversations).await? {
			QueryResponse::Conversations(list) => Ok(list),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)) => Err(unexpected(&other)),
		}
	}

	/// Continues a Conversation keyset snapshot from an opaque cursor
	/// returned by [`Client::conversations`] or this method.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] with restart metadata when the cursor
	/// expired or the snapshot changed, or the transport failure otherwise.
	pub async fn next_conversations(
		&self,
		cursor: PageCursor,
	) -> Result<ConversationList, ClientError> {
		self.require_minor(jet_protocol::FENCED_READS_MINOR)?;
		match self
			.query(QueryRequest::NextConversations { cursor })
			.await?
		{
			QueryResponse::Conversations(list) => Ok(list),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)) => Err(unexpected(&other)),
		}
	}

	/// Reads one Conversation with all of its Runs and the journal cursor
	/// the snapshot was read at.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Conversation does not exist
	/// or the daemon reports another stable error, or the transport failure
	/// otherwise.
	pub async fn conversation(
		&self,
		conversation_id: Uuid,
	) -> Result<ConversationSnapshot, ClientError> {
		match self
			.query(QueryRequest::Conversation { conversation_id })
			.await?
		{
			QueryResponse::Conversation(snapshot) => Ok(snapshot),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Events(_)) => Err(unexpected(&other)),
		}
	}

	/// Reads one page of journal Events strictly after `sequence`; zero
	/// starts from the beginning of the journal. The page's cursor tells
	/// whether later pages exist (ADR-0092).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn events_after(
		&self,
		sequence: u64,
	) -> Result<EventPage, ClientError> {
		match self.query(QueryRequest::Events { after: sequence }).await? {
			QueryResponse::Events(page) => Ok(page),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)) => Err(unexpected(&other)),
		}
	}

	/// Creates a Conversation with no Runs under the Command identity
	/// `command_id`, which a retry must reuse (ADR-0093).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn create_conversation(
		&self,
		command_id: Uuid,
		retention: RetentionPolicy,
	) -> Result<Conversation, ClientError> {
		match self
			.execute_command(
				command_id,
				CommandRequest::CreateConversation { retention },
			)
			.await?
		{
			CommandResponse::ConversationCreated(conversation) => {
				Ok(conversation)
			}
			other @ (CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)) => Err(unexpected(&other)),
		}
	}

	/// Records a new Run of a Conversation that has no live Run under the
	/// Command identity `command_id`, which a retry must reuse (ADR-0093).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Conversation does not exist
	/// or already has a live Run, or the transport failure otherwise.
	pub async fn create_run(
		&self,
		command_id: Uuid,
		conversation_id: Uuid,
	) -> Result<Run, ClientError> {
		match self
			.execute_command(
				command_id,
				CommandRequest::CreateRun { conversation_id },
			)
			.await?
		{
			CommandResponse::RunCreated(run) => Ok(run),
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunTransitioned(_)) => Err(unexpected(&other)),
		}
	}

	/// Moves a Run forward to `lifecycle` if its current Revision is
	/// `expected_revision`, under the Command identity `command_id`, which a
	/// retry must reuse (ADR-0093).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Run does not exist or the
	/// transition is not allowed, or the transport failure otherwise.
	pub async fn transition_run(
		&self,
		command_id: Uuid,
		run_id: Uuid,
		expected_revision: u64,
		lifecycle: RunLifecycle,
	) -> Result<Run, ClientError> {
		match self
			.execute_command(
				command_id,
				CommandRequest::TransitionRun {
					run_id,
					expected_revision,
					lifecycle,
				},
			)
			.await?
		{
			CommandResponse::RunTransitioned(run) => Ok(run),
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)) => Err(unexpected(&other)),
		}
	}
}

fn unexpected(reply: &impl std::fmt::Debug) -> ClientError {
	ClientError::Unexpected(format!("{reply:?}"))
}
