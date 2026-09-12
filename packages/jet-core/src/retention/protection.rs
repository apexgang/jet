//! What protects a Conversation from being forgotten (ADR-0001, ADR-0015).

use super::{Protection, WorkspaceState};
use crate::{ConversationId, CoreError};
use jet_store::ReadTransaction;

/// Everything protecting `conversation_id` in this snapshot of the store,
/// in a fixed order, plus what `workspace` says about its working tree.
/// The working tree is inspected outside the transaction by the caller,
/// because that takes Git, and a Conversation without a managed Workspace
/// has none to protect.
///
/// # Errors
///
/// Returns `conversation.not_found` when there is no such Conversation,
/// and a store category [`CoreError`] when the rows cannot be read.
pub(crate) async fn protections(
	tx: &mut ReadTransaction,
	conversation_id: ConversationId,
	workspace: WorkspaceState,
) -> Result<Vec<Protection>, CoreError> {
	let mut found = Vec::new();
	let runs = tx.runs(conversation_id.0).await?;
	if runs.iter().any(|run| !run.lifecycle.is_terminal()) {
		found.push(Protection::ActiveRun);
	}
	let queue = crate::turn::queue::load(tx, conversation_id).await?;
	if queue.entries.iter().any(|entry| {
		matches!(
			entry.turn.state,
			crate::TurnState::Queued | crate::TurnState::Active
		)
	}) {
		found.push(Protection::PendingTurn);
	}
	if !tx.scheduled_tasks(conversation_id.0).await?.is_empty() {
		found.push(Protection::EnabledSchedule);
	}
	if workspace.dirty {
		found.push(Protection::DirtyWorkspace);
	}
	if workspace.unpushed {
		found.push(Protection::UnpushedWork);
	}
	if tx
		.conversation_has_unresolved_effects(conversation_id.0)
		.await?
		|| tx.git_delivery_blocks(conversation_id.0).await?
	{
		found.push(Protection::UnresolvedEffect);
	}
	Ok(found)
}

/// Whether `protections` includes live work, which manual forgetting
/// refuses rather than waits out (ADR-0011).
pub(crate) fn has_live_work(protections: &[Protection]) -> bool {
	protections
		.iter()
		.any(|p| matches!(p, Protection::ActiveRun | Protection::PendingTurn))
}
