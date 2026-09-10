//! Admits Conversations and Runs in a Project Local checkout.

use super::project_not_found;
use crate::{
	Actor, ProjectId,
	command::{CommandOutcome, lifecycle},
	conversation::{Conversation, ConversationOrigin},
	error::CoreError,
	event::{EventKind, EventSubject},
};
use jet_store::{
	NewConversation, RetentionPolicy, WorkingTreeRecord, WriteTransaction,
};
use uuid::Uuid;

/// Records a Conversation that works in a Project's Local checkout.
///
/// # Errors
///
/// Returns `project.not_found` when the Project is not registered.
pub(crate) async fn create_in_local_checkout(
	tx: &mut WriteTransaction,
	actor: &Actor,
	retention: RetentionPolicy,
	origin: ConversationOrigin,
	project_id: ProjectId,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	if tx.project(project_id.0).await?.is_none() {
		return Err(project_not_found());
	}
	let conversation: Conversation = tx
		.insert_conversation(NewConversation {
			conversation_id: Uuid::now_v7(),
			retention,
			working_tree: WorkingTreeRecord::LocalCheckout {
				project_id: project_id.0,
			},
			origin: origin.record(),
			created_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	let event = EventKind::ConversationCreated {
		name: Some(conversation.name.clone()),
		retention,
		working_tree: conversation.working_tree,
		origin,
	};
	tx.append_event(event.to_record(
		actor,
		EventSubject::Conversation(conversation.conversation_id),
		now_unix_ms,
	)?)
	.await?;
	Ok(CommandOutcome::ConversationCreated(conversation))
}

/// Admits a new Run to a Project's Local checkout: one live managed Run
/// at a time (ADR-0025).
///
/// # Errors
///
/// Returns a `conflict` `run.local_checkout_busy` when another
/// Conversation's Run is live there. The message says what Jet cannot
/// do about the rest: processes outside its management are not locked.
pub(crate) async fn admit_local_checkout_run(
	tx: &mut WriteTransaction,
	project_id: ProjectId,
) -> Result<(), CoreError> {
	if lifecycle::any_live(&tx.local_checkout_runs(project_id.0).await?) {
		return Err(CoreError::conflict(
			"run.local_checkout_busy",
			"the Project's Local checkout already has a live managed Run; Jet \
			 admits one at a time there because it cannot lock the processes \
			 outside its management, so start this Conversation in a Workspace \
			 instead",
		));
	}
	Ok(())
}
