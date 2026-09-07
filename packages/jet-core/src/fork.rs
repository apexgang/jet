//! Conversation forks from immutable Change checkpoints (ADR-0035).

use std::path::PathBuf;

use jet_store::RetentionPolicy;

use crate::command::CommandOutcome;
use crate::{
	Actor, ChangeCheckpoint, ConversationId, ConversationOrigin, Core,
	CoreError, RunId, WorkingTree, WorkspaceHome, run_state, tree_capture,
	workspace, worktree,
};

/// Filesystem and durable source state resolved before the write transaction.
pub(crate) struct PreparedFork {
	checkpoint: ChangeCheckpoint,
	retention: RetentionPolicy,
	workspace: workspace::PreparedWorkspace,
}

/// Resolves one retained checkpoint into an immutable Workspace preparation.
/// It never inspects the source Workspace's current files.
pub(crate) async fn prepare(
	core: &Core,
	source_run_id: RunId,
	checkpoint_turn: u32,
) -> Result<PreparedFork, CoreError> {
	if checkpoint_turn == 0 {
		return Err(CoreError::invalid_input(
			"fork.checkpoint_invalid",
			"a Conversation fork selects a positive checkpoint turn",
		));
	}
	let (checkpoint, retention, project_id, project_root) = core
		.store
		.read(async |tx| {
			let run = tx
				.run(source_run_id.0)
				.await?
				.ok_or_else(source_not_found)?;
			let source_conversation_id = ConversationId(run.conversation_id);
			let conversation = tx
				.conversation(run.conversation_id)
				.await?
				.ok_or_else(source_not_found)?;
			let payload = tx
				.change_checkpoint(source_run_id.0, checkpoint_turn)
				.await?
				.ok_or_else(crate::checkpoint_state::missing)?;
			let checkpoint: ChangeCheckpoint = run_state::decode(&payload)
				.map_err(|_| invalid_checkpoint())?;
			if checkpoint.conversation_id != source_conversation_id
				|| checkpoint.run_id != source_run_id
				|| checkpoint.turn != checkpoint_turn
			{
				return Err(invalid_checkpoint());
			}
			let working_tree = WorkingTree::from(conversation.working_tree);
			let project_id = match working_tree {
				WorkingTree::Workspace { project_id } => {
					let workspace = tx
						.workspace_of(run.conversation_id)
						.await?
						.ok_or_else(invalid_checkpoint)?;
					if checkpoint.workspace_id.map(|id| id.0)
						!= Some(workspace.workspace_id)
					{
						return Err(invalid_checkpoint());
					}
					project_id
				}
				WorkingTree::LocalCheckout { project_id } => {
					if checkpoint.workspace_id.is_some() {
						return Err(invalid_checkpoint());
					}
					project_id
				}
				WorkingTree::NoProject => return Err(source_not_found()),
			};
			let project = tx
				.project(project_id.0)
				.await?
				.ok_or_else(source_not_found)?;
			Ok::<_, CoreError>((
				checkpoint,
				conversation.retention,
				project_id,
				PathBuf::from(project.root),
			))
		})
		.await?;
	if !checkpoint.after.omitted_files.is_empty() {
		return Err(CoreError::conflict(
			"fork.checkpoint_incomplete",
			"the selected checkpoint omitted file content and cannot produce an exact Workspace",
		));
	}
	let resolved =
		worktree::resolve_commit(&project_root, &checkpoint.after.commit)
			.await?;
	if resolved != checkpoint.after.commit {
		return Err(invalid_checkpoint());
	}
	let changes = tree_capture::diff_trees(
		&project_root,
		&checkpoint.after.commit,
		&checkpoint.after.tree,
		fork_git_failed,
	)
	.await?;
	let changed_paths = u32::try_from(changes.len()).unwrap_or(u32::MAX);
	let workspace = workspace::from_checkpoint(
		project_id,
		project_root,
		checkpoint.after.commit.clone(),
		checkpoint.after.tree.clone(),
		changed_paths,
	);
	Ok(PreparedFork {
		checkpoint,
		retention,
		workspace,
	})
}

/// Inserts the fork only while the selected immutable checkpoint still matches
/// what preparation read, then materializes its distinct Workspace.
pub(crate) async fn create(
	tx: &mut jet_store::WriteTransaction,
	actor: &Actor,
	prepared: PreparedFork,
	home: &WorkspaceHome,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let retained = tx
		.change_checkpoint(
			prepared.checkpoint.run_id.0,
			prepared.checkpoint.turn,
		)
		.await?
		.ok_or_else(crate::checkpoint_state::missing)?;
	let retained: ChangeCheckpoint =
		run_state::decode(&retained).map_err(|_| invalid_checkpoint())?;
	if retained != prepared.checkpoint {
		return Err(CoreError::conflict(
			"fork.checkpoint_changed",
			"the selected checkpoint changed before the fork was recorded",
		));
	}
	let origin = ConversationOrigin::Forked {
		source_conversation_id: prepared.checkpoint.conversation_id,
		source_run_id: prepared.checkpoint.run_id,
		checkpoint_turn: prepared.checkpoint.turn,
	};
	// ASVS 2.3.3 and 15.4.2: provenance, identities, Events, and Workspace
	// registration commit atomically against the retained checkpoint.
	workspace::create(
		tx,
		actor,
		prepared.retention,
		origin,
		prepared.workspace,
		home,
		now_unix_ms,
	)
	.await
}

fn source_not_found() -> CoreError {
	CoreError::not_found(
		"fork.source_not_found",
		"the source Conversation, Run, or Project does not exist",
	)
}

fn invalid_checkpoint() -> CoreError {
	CoreError::internal(
		"fork.checkpoint_invalid",
		"the selected checkpoint does not match its durable source",
	)
}

fn fork_git_failed(detail: String) -> CoreError {
	CoreError::unavailable(
		"fork.checkpoint_unavailable",
		"the selected checkpoint content is unavailable",
		detail.chars().take(512).collect::<String>(),
	)
}
