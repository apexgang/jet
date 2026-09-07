//! Conversation forks from immutable Change checkpoints (ADR-0035).

use std::{
	collections::{BTreeMap, BTreeSet},
	path::PathBuf,
};

use jet_store::{
	ForkContextEvents, ForkLaunchContextRecord, RetentionPolicy,
};
use serde::{Deserialize, Serialize};

use crate::command::CommandOutcome;
use crate::{
	Actor, ChangeCheckpoint, ConversationId, ConversationOrigin, Core,
	CoreError, Event, EventKind, ForkLaunchSource, LaunchPlan, RunId,
	WorkingTree, WorkspaceHome, run_state, tree_capture, workspace, worktree,
};

const CONTEXT_ROLE_BYTES: usize = 12 * 1024;
const CONTEXT_INPUT_BYTES: usize = 8 * 1024;
const CONTEXT_OUTPUT_BYTES: usize = 2 * 1024;
const CONTEXT_MAX_BYTES: usize = 32 * 1024;
const CONTEXT_ENTRY_LIMIT: usize = 256;

/// Filesystem and durable source state resolved before the write transaction.
pub(crate) struct PreparedFork {
	checkpoint: ChangeCheckpoint,
	retention: RetentionPolicy,
	workspace: workspace::PreparedWorkspace,
	source: Option<ForkLaunchSource>,
	context: ForkLaunchContext,
}

/// Structured, bounded history copied into a Conversation fork.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ForkLaunchContext {
	history_truncated: bool,
	entries: Vec<ForkContextEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ForkContextEntry {
	role: ForkContextRole,
	content: String,
	truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ForkContextRole {
	User,
	Assistant,
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
	let (
		checkpoint,
		retention,
		project_id,
		project_root,
		source,
		context_events,
	) = core
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
			let source = if let Some(execution) =
				tx.run_execution(source_run_id.0).await?
			{
				let source_plan: LaunchPlan = run_state::decode(&execution.plan)
					.map_err(|_| invalid_checkpoint())?;
				let source_state: run_state::State =
					run_state::decode(&execution.state)
						.map_err(|_| invalid_checkpoint())?;
				Some(ForkLaunchSource {
					craft: source_plan.craft,
					native_conversation: source_state
						.native_conversation
						.or(source_plan.native_conversation),
				})
			} else {
				None
			};
			let context_events = tx
				.fork_context_events(
					source_conversation_id.0,
					source_run_id.0,
					checkpoint_turn,
				)
				.await?;
			Ok::<_, CoreError>((
				checkpoint,
				conversation.retention,
				project_id,
				PathBuf::from(project.root),
				source,
				context_events,
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
	let context = capture_context(context_events)?;
	Ok(PreparedFork {
		checkpoint,
		retention,
		workspace,
		source,
		context,
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
	let PreparedFork {
		checkpoint,
		retention,
		workspace,
		source,
		context,
	} = prepared;
	let retained = tx
		.change_checkpoint(checkpoint.run_id.0, checkpoint.turn)
		.await?
		.ok_or_else(crate::checkpoint_state::missing)?;
	let retained: ChangeCheckpoint =
		run_state::decode(&retained).map_err(|_| invalid_checkpoint())?;
	if retained != checkpoint {
		return Err(CoreError::conflict(
			"fork.checkpoint_changed",
			"the selected checkpoint changed before the fork was recorded",
		));
	}
	let origin = ConversationOrigin::Forked {
		source_conversation_id: checkpoint.conversation_id,
		source_run_id: checkpoint.run_id,
		checkpoint_turn: checkpoint.turn,
	};
	let (source_craft, source_native_conversation) = match source {
		Some(source) => (
			Some(serde_json::to_string(&source.craft).map_err(|error| {
				CoreError::internal("fork.source_unencodable", error.to_string())
			})?),
			source.native_conversation.filter(|identity| {
				!identity.is_empty()
					&& identity.len() <= 4096
					&& !identity.chars().any(char::is_control)
			}),
		),
		None => (None, None),
	};
	let context_json = context.encode()?;
	// ASVS 2.3.3 and 15.4.2: provenance, identities, Events, and Workspace
	// registration commit atomically against the retained checkpoint.
	let outcome = workspace::create(
		tx,
		actor,
		retention,
		origin,
		workspace,
		home,
		now_unix_ms,
	)
	.await?;
	let CommandOutcome::ConversationCreated(conversation) = &outcome else {
		unreachable!("Workspace creation returns a Conversation")
	};
	tx.insert_conversation_fork_launch(&ForkLaunchContextRecord {
		conversation_id: conversation.conversation_id.0,
		checkpoint_commit: checkpoint.after.commit,
		checkpoint_tree: checkpoint.after.tree,
		source_craft,
		source_native_conversation,
		context_json,
	})
	.await?;
	Ok(outcome)
}

fn capture_context(
	context_events: ForkContextEvents,
) -> Result<ForkLaunchContext, CoreError> {
	let ForkContextEvents {
		events,
		earlier_events_omitted,
	} = context_events;
	let mut input = BTreeMap::<uuid::Uuid, (u64, String)>::new();
	let mut executed = BTreeSet::new();
	let mut candidates = Vec::new();
	let mut truncated = earlier_events_omitted;
	for record in events {
		let sequence = record.sequence;
		match Event::try_from(record)?.kind {
			EventKind::TurnInput { turn_id, text } => {
				input
					.entry(turn_id)
					.and_modify(|(_, input)| input.push_str(&text))
					.or_insert((sequence, text));
			}
			EventKind::TurnChanged { turn } if turn.run_id.is_some() => {
				executed.insert(turn.turn_id);
			}
			EventKind::RunOutput {
				native_json,
				presentation_json,
			} => {
				let content = if presentation_json.is_empty() {
					native_json
				} else {
					presentation_json.join("\n")
				};
				let (content, was_truncated) =
					truncate_utf8(&content, CONTEXT_OUTPUT_BYTES);
				truncated |= was_truncated;
				candidates.push((
					sequence,
					ForkContextEntry {
						role: ForkContextRole::Assistant,
						content: content.into(),
						truncated: was_truncated,
					},
				));
			}
			_ => continue,
		}
	}
	for (turn_id, (sequence, input)) in input {
		if !executed.contains(&turn_id) {
			continue;
		}
		let (content, was_truncated) =
			truncate_utf8(&input, CONTEXT_INPUT_BYTES);
		truncated |= was_truncated;
		candidates.push((
			sequence,
			ForkContextEntry {
				role: ForkContextRole::User,
				content: content.into(),
				truncated: was_truncated,
			},
		));
	}
	candidates.sort_by_key(|(sequence, _)| *sequence);
	let mut selected = Vec::new();
	for role in [ForkContextRole::User, ForkContextRole::Assistant] {
		let mut remaining = CONTEXT_ROLE_BYTES;
		for (sequence, entry) in candidates.iter().rev() {
			if entry.role != role {
				continue;
			}
			let size = encoded_entry(entry)?.len() + 1;
			if size <= remaining {
				selected.push((*sequence, entry.clone()));
				remaining -= size;
			} else {
				truncated = true;
			}
		}
	}
	selected.sort_by_key(|(sequence, _)| *sequence);
	let context = ForkLaunchContext {
		history_truncated: truncated,
		entries: selected.into_iter().map(|(_, entry)| entry).collect(),
	};
	context.encode()?;
	Ok(context)
}

impl ForkLaunchContext {
	pub(crate) fn decode(encoded: &str) -> Result<Self, CoreError> {
		let context: Self = serde_json::from_str(encoded).map_err(|error| {
			CoreError::internal("fork.context_invalid", error.to_string())
		})?;
		context.encode()?;
		Ok(context)
	}

	fn encode(&self) -> Result<String, CoreError> {
		let encoded = serde_json::to_string(self).map_err(|error| {
			CoreError::internal("fork.context_unencodable", error.to_string())
		})?;
		let entries_valid = self.entries.len() <= CONTEXT_ENTRY_LIMIT
			&& self.entries.iter().all(|entry| {
				let maximum = match entry.role {
					ForkContextRole::User => CONTEXT_INPUT_BYTES,
					ForkContextRole::Assistant => CONTEXT_OUTPUT_BYTES,
				};
				entry.content.len() <= maximum
			});
		if !entries_valid || encoded.len() > CONTEXT_MAX_BYTES {
			return Err(CoreError::internal(
				"fork.context_invalid",
				"the retained fork context exceeds its fixed bounds",
			));
		}
		Ok(encoded)
	}

	pub(crate) fn render(&self) -> Result<String, CoreError> {
		let history = self
			.entries
			.iter()
			.map(encoded_entry)
			.collect::<Result<Vec<_>, _>>()?
			.join("\n");
		Ok(format!(
			"history_format: jsonl\nhistory_truncated: {}\nhistory:\n{history}",
			self.history_truncated,
		))
	}
}

fn encoded_entry(entry: &ForkContextEntry) -> Result<String, CoreError> {
	Ok(serde_json::to_string(entry)
		.map_err(|error| {
			CoreError::internal("fork.context_unencodable", error.to_string())
		})?
		.replace('<', "\\u003c")
		.replace('>', "\\u003e"))
}

fn truncate_utf8(value: &str, maximum: usize) -> (&str, bool) {
	if value.len() <= maximum {
		return (value, false);
	}
	let mut end = maximum;
	while !value.is_char_boundary(end) {
		end -= 1;
	}
	(&value[..end], true)
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

#[cfg(test)]
#[path = "fork_tests.rs"]
mod tests;
