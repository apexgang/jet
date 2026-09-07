//! Direct user edits through registered roots (ADR-0041, ADR-0064, ADR-0101).

use std::path::PathBuf;

use jet_store::{
	NewCommandReceipt, NewUserEditIntent, ReadTransaction,
	UserEditIntentRecord, WriteTransaction,
};
use serde::{Deserialize, Serialize};

use crate::event::{EventSequence, EventSubject};
use crate::relative_path::RelativePath;
use crate::user_input_files::{
	observe, revision, root, valid_revision, write_atomic,
};
use crate::{
	Actor, ChangeEvidence, ChangeOrigin, CommandId, CommandOutcome,
	ConversationId, Core, CoreError, EventKind, ProjectId, RecoveryAction,
	RunId, WorkspaceId,
};

/// Largest file that the direct-edit protocol reads or replaces.
pub(crate) const MAX_EDIT_BYTES: usize = 128 * 1024;

/// A registered root through which a file is addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileTarget {
	/// A registered Project's Local checkout.
	Project {
		/// Project identity.
		project_id: ProjectId,
	},
	/// A Conversation-owned Workspace.
	Workspace {
		/// Workspace identity.
		workspace_id: WorkspaceId,
	},
}

/// Exact Git content and mode observed for one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRevision {
	/// Git blob object, or all zeroes for a missing file.
	pub object: String,
	/// Git mode, or `000000` for a missing file.
	pub mode: String,
}

/// Bounded UTF-8 content read through a registered root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditableFile {
	/// Journal fence observed with the target record.
	pub cursor: EventSequence,
	/// Registered root that was read.
	pub target: FileTarget,
	/// Validated relative path.
	pub path: RelativePath,
	/// Exact optimistic-concurrency precondition.
	pub revision: FileRevision,
	/// Content, or `None` when the path is missing.
	pub content: Option<String>,
}

/// One validated inline review comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewComment {
	/// Path relative to the Conversation working tree.
	pub path: RelativePath,
	/// One-based line number.
	pub line: u32,
	/// User-authored comment.
	pub comment: String,
}

/// Durable result of a direct edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserEdit {
	/// Registered root that was edited.
	pub target: FileTarget,
	/// Validated relative path.
	pub path: RelativePath,
	/// Exact resulting state.
	pub revision: FileRevision,
}

/// Filesystem state prepared outside the write transaction.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct PreparedUserEdit {
	target: FileTarget,
	path: RelativePath,
	root: PathBuf,
	before_revision: FileRevision,
	permission_mode: Option<u32>,
	after_revision: FileRevision,
	content: String,
	no_change: bool,
	intent_recorded: bool,
	#[serde(skip)]
	resumed: bool,
}

pub(crate) struct IntentContext<'a> {
	pub(crate) actor: &'a Actor,
	pub(crate) command_id: CommandId,
	pub(crate) request_digest: [u8; 32],
	pub(crate) recorded_at_unix_ms: i64,
}

/// Reads one editable file with a target-record journal fence.
pub(crate) async fn read(
	core: &Core,
	target: FileTarget,
	path: RelativePath,
) -> Result<EditableFile, CoreError> {
	let (cursor, root) = core
		.store
		.read(async |tx| {
			Ok::<_, CoreError>((
				EventSequence(tx.event_cursor().await?),
				root(tx, target).await?,
			))
		})
		.await?;
	let observed = observe(root.clone(), path.clone()).await?;
	Ok(EditableFile {
		cursor,
		target,
		path,
		revision: observed.revision,
		content: observed.content,
	})
}

/// Resolves, bounds, and revision-checks an edit before its transaction.
pub(crate) async fn prepare(
	core: &Core,
	context: IntentContext<'_>,
	target: FileTarget,
	path: RelativePath,
	expected_revision: FileRevision,
	content: String,
) -> Result<PreparedUserEdit, CoreError> {
	let IntentContext {
		actor,
		command_id,
		request_digest,
		recorded_at_unix_ms,
	} = context;
	if let Some(intent) = core
		.store
		.read(async |tx| {
			tx.user_edit_intent(actor.record(), command_id.0).await
		})
		.await?
	{
		return resume(intent, request_digest);
	}
	// ASVS 5.1.1, 5.1.3: validate the complete untrusted payload at the
	// protocol boundary before filesystem or durable state is changed.
	if content.len() > MAX_EDIT_BYTES {
		return Err(CoreError::invalid_input(
			"user_edit.too_large",
			"editable content must be at most 131072 UTF-8 bytes",
		));
	}
	if !valid_revision(&expected_revision) {
		return Err(CoreError::invalid_input(
			"user_edit.invalid_revision",
			"an expected file Revision has a Git object and regular-file mode",
		));
	}
	let root = core.store.read(async |tx| root(tx, target).await).await?;
	let before = observe(root.clone(), path.clone()).await?;
	let after_mode = before.permission_mode.map_or("100644", |mode| {
		if mode & 0o111 == 0 {
			"100644"
		} else {
			"100755"
		}
	});
	let after_revision =
		revision(&root, &path, Some(&content), after_mode).await?;
	if before.revision != expected_revision {
		return Err(stale(target, &path, before.revision));
	}
	let no_change = before.revision == after_revision
		&& before.content.as_deref() == Some(content.as_str());
	let prepared = PreparedUserEdit {
		target,
		path,
		root,
		before_revision: before.revision,
		permission_mode: before.permission_mode,
		after_revision,
		content,
		no_change,
		intent_recorded: !no_change,
		resumed: false,
	};
	if no_change {
		return Ok(prepared);
	}
	let plan = serde_json::to_string(&prepared).map_err(|error| {
		CoreError::internal("user_edit.unencodable", error.to_string())
	})?;
	core.store
		.write(async |tx| {
			tx.insert_user_edit_intent(&NewUserEditIntent {
				actor: actor.record(),
				command_id: command_id.0,
				request_digest,
				recorded_at_unix_ms,
				plan,
			})
			.await?;
			let intent = tx
				.user_edit_intent(actor.record(), command_id.0)
				.await?
				.ok_or_else(|| {
					CoreError::internal(
						"user_edit.intent_missing",
						"a recorded user edit intent disappeared",
					)
				})?;
			resume(intent, request_digest).map(|_| ())
		})
		.await?;
	Ok(prepared)
}

/// Revalidates and applies a prepared edit, then records direct-user
/// provenance and active-turn evidence in the receipt transaction.
pub(crate) async fn apply(
	core: &Core,
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	prepared: PreparedUserEdit,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let PreparedUserEdit {
		target,
		path,
		root: prepared_root,
		before_revision,
		permission_mode,
		after_revision,
		content,
		no_change,
		intent_recorded,
		resumed,
	} = prepared;
	let current_root = match root(tx, target).await {
		Ok(root) => root,
		Err(error) => {
			return abandon(tx, actor, command_id, intent_recorded, error)
				.await;
		}
	};
	if current_root != prepared_root {
		if intent_recorded {
			tx.delete_user_edit_intent(actor.record(), command_id.0)
				.await?;
		}
		return Err(CoreError::conflict(
			"user_edit.target_changed",
			"the registered target changed while the edit was being prepared",
		));
	}
	let current = match observe(current_root.clone(), path.clone()).await {
		Ok(current) => current,
		Err(error) => {
			return abandon(tx, actor, command_id, intent_recorded, error)
				.await;
		}
	};
	let recovered = resumed
		&& intent_recorded
		&& current.revision == after_revision
		&& current.content.as_deref() == Some(content.as_str());
	if current.revision != before_revision && !recovered {
		if intent_recorded {
			tx.delete_user_edit_intent(actor.record(), command_id.0)
				.await?;
		}
		return Err(stale(target, &path, current.revision));
	}
	if !no_change {
		let (subject, change_run) = subject_and_run(tx, target).await?;
		let write_root = current_root.clone();
		let write_path = path.clone();
		if !recovered
			&& let Err(error) =
				write_atomic(write_root, write_path, content, permission_mode)
					.await
		{
			return abandon(tx, actor, command_id, intent_recorded, error)
				.await;
		}
		// ASVS 16.3.3, 16.4.1: a typed JSON Event records the authenticated
		// Actor without treating user-controlled text as log structure.
		tx.append_event(
			EventKind::UserEditApplied {
				target,
				path: path.clone(),
				before_revision: before_revision.clone(),
				after_revision: after_revision.clone(),
			}
			.to_record(actor, subject, now)?,
		)
		.await?;
		// A desired state found during recovery proves idempotent completion,
		// but not that Jet (rather than an external editor) performed the
		// transition. Preserve sound attribution by recording evidence only
		// when this process completed the atomic write.
		if !recovered && let Some((run_id, window)) = change_run {
			let evidence = ChangeEvidence {
				activity_id: command_id.0.to_string(),
				origin: ChangeOrigin::UserEdit {
					client_id: actor.client_id(),
				},
				path: path.as_str().into(),
				before_object: before_revision.object,
				after_object: after_revision.object.clone(),
				before_mode: before_revision.mode,
				after_mode: after_revision.mode.clone(),
			};
			match window {
				crate::change_evidence::Window::ActiveTurn => {
					crate::change_evidence::record(core, tx, run_id, evidence)
						.await?;
				}
				crate::change_evidence::Window::BetweenTurns => {
					crate::change_evidence::record_between_turns(
						core, tx, run_id, evidence,
					)
					.await?;
				}
			}
		}
		tx.delete_user_edit_intent(actor.record(), command_id.0)
			.await?;
	}
	Ok(CommandOutcome::UserEditApplied(UserEdit {
		target,
		path,
		revision: after_revision,
	}))
}

async fn abandon(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	intent_recorded: bool,
	error: CoreError,
) -> Result<CommandOutcome, CoreError> {
	if intent_recorded && error.is_authoritative_result() {
		tx.delete_user_edit_intent(actor.record(), command_id.0)
			.await?;
	}
	Err(error)
}

fn resume(
	intent: UserEditIntentRecord,
	request_digest: [u8; 32],
) -> Result<PreparedUserEdit, CoreError> {
	if intent.request_digest != request_digest {
		return Err(CoreError::conflict(
			"command.identity_reused",
			"the Command identity was already used for different content",
		));
	}
	let mut prepared: PreparedUserEdit = serde_json::from_str(&intent.plan)
		.map_err(|error| {
			CoreError::internal("user_edit.intent_invalid", error.to_string())
		})?;
	prepared.resumed = true;
	Ok(prepared)
}

impl Core {
	/// Finishes direct edits whose write-ahead intent survived an interruption.
	///
	/// # Errors
	///
	/// Returns a store or filesystem error when reconciliation cannot yet make
	/// progress. Authoritative conflicts are retained as Command receipts.
	pub async fn perform_user_edits(&self) -> Result<(), CoreError> {
		let intents = self
			.store
			.read(async |tx| tx.user_edit_intents().await)
			.await?;
		for intent in intents {
			let actor = Actor::from_record(intent.actor);
			let command_id = CommandId(intent.command_id);
			let prepared = resume(intent.clone(), intent.request_digest)?;
			self.store
				.write(async |tx| {
					if tx
						.command_receipt(intent.actor, intent.command_id)
						.await?
						.is_some()
					{
						tx.delete_user_edit_intent(
							intent.actor,
							intent.command_id,
						)
						.await?;
						return Ok(());
					}
					let result = apply(
						self,
						tx,
						&actor,
						command_id,
						prepared,
						intent.recorded_at_unix_ms,
					)
					.await;
					if let Err(error) = &result {
						if !error.is_authoritative_result() {
							return Err(error.clone());
						}
						tx.delete_user_edit_intent(
							intent.actor,
							intent.command_id,
						)
						.await?;
					}
					tx.insert_command_receipt(&NewCommandReceipt {
						actor: intent.actor,
						command_id: intent.command_id,
						request_digest: intent.request_digest,
						recorded_at_unix_ms: intent.recorded_at_unix_ms,
						outcome_version:
							crate::command_receipt::OUTCOME_VERSION,
						outcome: crate::command_receipt::encode_result(
							&result,
						)?,
					})
					.await?;
					Ok::<_, CoreError>(())
				})
				.await?;
		}
		Ok(())
	}
}

async fn subject_and_run(
	tx: &mut ReadTransaction,
	target: FileTarget,
) -> Result<
	(
		EventSubject,
		Option<(RunId, crate::change_evidence::Window)>,
	),
	CoreError,
> {
	let (subject, runs) = match target {
		FileTarget::Project { project_id } => (
			EventSubject::Plane,
			tx.local_checkout_runs(project_id.0).await?,
		),
		FileTarget::Workspace { workspace_id } => {
			let workspace =
				tx.workspace(workspace_id.0).await?.ok_or_else(|| {
					CoreError::not_found(
						"workspace.not_found",
						"the Workspace does not exist",
					)
				})?;
			let conversation_id = ConversationId(workspace.conversation_id);
			(
				EventSubject::Conversation(conversation_id),
				tx.runs(conversation_id.0).await?,
			)
		}
	};
	Ok((subject, change_run(tx, runs).await?))
}

async fn change_run(
	tx: &mut ReadTransaction,
	runs: Vec<jet_store::RunRecord>,
) -> Result<Option<(RunId, crate::change_evidence::Window)>, CoreError> {
	for run in runs
		.into_iter()
		.rev()
		.filter(|run| !run.lifecycle.is_terminal())
	{
		let execution =
			tx.run_execution(run.run_id).await?.ok_or_else(|| {
				CoreError::internal(
					"user_edit.run_execution_missing",
					"an active Run has no execution state",
				)
			})?;
		let state: crate::run_state::State =
			crate::run_state::decode(&execution.state)?;
		if let Some(changes) = state.changes {
			let window = if changes.active.is_some() {
				crate::change_evidence::Window::ActiveTurn
			} else {
				crate::change_evidence::Window::BetweenTurns
			};
			return Ok(Some((RunId(run.run_id), window)));
		}
	}
	Ok(None)
}

fn stale(
	target: FileTarget,
	path: &RelativePath,
	current_revision: FileRevision,
) -> CoreError {
	CoreError::conflict_with_action(
		"user_edit.stale_revision",
		"the file changed after this edit was prepared; refresh it first",
		RecoveryAction::RefreshFile {
			target,
			path: path.as_str().into(),
			current_revision,
		},
	)
}
