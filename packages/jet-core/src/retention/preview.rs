//! The reads: what is in Jet Trash, and what forgetting a Conversation
//! would meet, read before deciding.

use super::{ConversationTrash, RetentionPreview, WorkspaceState, protections};
use crate::{ConversationId, Core, CoreError, EventSequence, QueryResult};

impl Core {
	/// Reads everything in Jet Trash under one Event fence.
	pub(crate) async fn conversation_trash(
		&self,
	) -> Result<QueryResult, CoreError> {
		self.store
			.read(async |tx| {
				Ok(QueryResult::ConversationTrash(ConversationTrash {
					cursor: EventSequence(tx.event_cursor().await?),
					entries: tx
						.trash_entries()
						.await?
						.into_iter()
						.map(Into::into)
						.collect(),
				}))
			})
			.await
	}

	/// Reads the protections of `conversation_id`, its Trash entry, and
	/// how many audit records name it. The Workspace is inspected between
	/// two reads, outside any transaction, because it takes Git.
	pub(crate) async fn retention_preview(
		&self,
		conversation_id: ConversationId,
	) -> Result<QueryResult, CoreError> {
		let workspace = self
			.store
			.read(async |tx| {
				if tx.conversation(conversation_id.0).await?.is_none() {
					return Err(CoreError::not_found(
						"conversation.not_found",
						"the Conversation does not exist",
					));
				}
				Ok(tx.workspace_of(conversation_id.0).await?)
			})
			.await?;
		let state = WorkspaceState::of(workspace.as_ref()).await?;
		self.store
			.read(async |tx| {
				Ok(QueryResult::RetentionPreview(RetentionPreview {
					conversation_id,
					protections: protections(tx, conversation_id, state)
						.await?,
					trash: tx
						.trash_entry(conversation_id.0)
						.await?
						.map(Into::into),
					audit_records: tx
						.audit_target_records(
							"conversation",
							&conversation_id.0.to_string(),
						)
						.await?,
				}))
			})
			.await
	}
}
