//! Admission binds each operation to immutable content, policy and repository state.
use crate::{
	Actor, ChangeCheckpoint, ClientId, CommandId, CommandOutcome,
	ConversationId, CoreError, GitCheckpoint, GitDelivery, GitDeliveryOutcome,
	GitDeliveryPolicy, GitOperation, SettingKey, SettingScope, SettingValue,
	UtilityRequest,
};
use jet_store::{
	EffectKindRecord, EffectSafetyRecord, NewEffect, ReadTransaction,
	WorkingTreeRecord, WriteTransaction,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Document {
	pub delivery: GitDelivery,
	pub client_id: ClientId,
	pub root: PathBuf,
	pub head: String,
	pub tree: Option<String>,
	pub index: String,
	pub branch: Option<String>,
	pub remote_url: Option<String>,
	pub previous: Option<Uuid>,
	pub prepared_commit: Option<String>,
	pub draft_number: Option<u64>,
	pub draft_url: Option<String>,
}
pub(crate) fn refused(code: &'static str) -> CoreError {
	CoreError::conflict(
		code,
		"Git delivery could not proceed with the recorded repository state and policy",
	)
}
pub(crate) async fn load(
	tx: &mut ReadTransaction,
	id: Uuid,
) -> Result<Document, CoreError> {
	crate::run_state::decode(
		&tx.git_delivery(id)
			.await?
			.ok_or_else(|| refused("git.delivery_not_found"))?,
	)
}
pub(crate) async fn save(
	tx: &mut WriteTransaction,
	doc: &Document,
) -> Result<(), CoreError> {
	tx.save_git_delivery(
		doc.delivery.delivery_id,
		doc.delivery.conversation_id.0,
		&serde_json::to_string(doc)
			.map_err(|_| refused("git.invalid_record"))?,
	)
	.await?;
	Ok(())
}
pub(crate) async fn query(
	tx: &mut ReadTransaction,
	id: ConversationId,
) -> Result<Vec<GitDelivery>, CoreError> {
	tx.git_deliveries(id.0)
		.await?
		.into_iter()
		.map(|json| {
			crate::run_state::decode::<Document>(&json).map(|doc| doc.delivery)
		})
		.collect()
}
pub(crate) async fn root(
	tx: &mut ReadTransaction,
	id: ConversationId,
) -> Result<PathBuf, CoreError> {
	let conversation = tx
		.conversation(id.0)
		.await?
		.ok_or_else(|| refused("git.conversation_not_found"))?;
	let project_id = match conversation.working_tree {
		WorkingTreeRecord::Workspace { project_id }
		| WorkingTreeRecord::LocalCheckout { project_id } => project_id,
		WorkingTreeRecord::NoProject => {
			return Err(refused("git.project_required"));
		}
	};
	let project = tx
		.project(project_id)
		.await?
		.ok_or_else(|| refused("git.project_required"))?;
	match conversation.working_tree {
		WorkingTreeRecord::Workspace { .. } => Ok(PathBuf::from(
			tx.workspace_of(id.0)
				.await?
				.ok_or_else(|| refused("git.workspace_missing"))?
				.root,
		)),
		WorkingTreeRecord::LocalCheckout { .. } => {
			Ok(PathBuf::from(project.root))
		}
		WorkingTreeRecord::NoProject => Err(refused("git.project_required")),
	}
}
pub(crate) async fn policy(
	tx: &mut ReadTransaction,
	id: ConversationId,
) -> Result<GitDeliveryPolicy, CoreError> {
	let values = crate::setting::resolve(
		&[
			SettingKey::GitAutoBranch,
			SettingKey::GitAutoCommit,
			SettingKey::GitAutoPush,
			SettingKey::GitAutoDraftPullRequest,
			SettingKey::GitBranchPrefix,
		],
		&tx.settings_for_scope(
			SettingScope::Conversation {
				conversation_id: id,
			}
			.record(),
		)
		.await?,
	);
	let SettingValue::Text(prefix) = &values[4].value else {
		return Err(refused("git.invalid_policy"));
	};
	Ok(GitDeliveryPolicy {
		automatic: false,
		branch: values[0].value == SettingValue::Flag(true),
		commit: values[1].value == SettingValue::Flag(true),
		push: values[2].value == SettingValue::Flag(true),
		draft_pull_request: values[3].value == SettingValue::Flag(true),
		branch_prefix: prefix.clone(),
	})
}
pub(crate) async fn idle(
	tx: &mut ReadTransaction,
	id: ConversationId,
) -> Result<(), CoreError> {
	let conversation = tx
		.conversation(id.0)
		.await?
		.ok_or_else(|| refused("git.conversation_not_found"))?;
	let runs = match conversation.working_tree {
		WorkingTreeRecord::LocalCheckout { project_id } => {
			tx.local_checkout_runs(project_id).await?
		}
		WorkingTreeRecord::Workspace { .. } | WorkingTreeRecord::NoProject => {
			tx.runs(id.0).await?
		}
	};
	for run in runs {
		let active = !run.lifecycle.is_terminal();
		if active && run.conversation_id != id.0 {
			return Err(refused("git.run_active"));
		}
		let Some(execution) = tx.run_execution(run.run_id).await? else {
			if active {
				return Err(refused("git.run_active"));
			}
			continue;
		};
		let state: crate::run_state::State =
			crate::run_state::decode(&execution.state)?;
		if state.partial_source.count != 0
			|| (active
				&& state.changes.as_ref().is_none_or(|c| c.active.is_some()))
		{
			return Err(refused("git.run_active"));
		}
	}
	Ok(())
}
pub(crate) async fn admit(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	id: ConversationId,
	checkpoint: Option<GitCheckpoint>,
	operation: GitOperation,
) -> Result<CommandOutcome, CoreError> {
	if tx.git_delivery_blocks(id.0).await? {
		return Err(refused("git.delivery_unresolved"));
	}
	idle(tx, id).await?;
	let policy = policy(tx, id).await?;
	let source = match checkpoint {
		Some(source) => {
			let checkpoint: ChangeCheckpoint = crate::run_state::decode(
				&tx.change_checkpoint(source.run_id.0, source.turn)
					.await?
					.ok_or_else(|| refused("git.checkpoint_required"))?,
			)?;
			if checkpoint.conversation_id != id {
				return Err(refused("git.checkpoint_mismatch"));
			}
			Some(checkpoint)
		}
		None => None,
	};
	let doc = queue(
		tx,
		actor,
		command_id,
		id,
		source.as_ref(),
		Step {
			operation,
			policy,
			previous: None,
		},
	)
	.await?;
	Ok(CommandOutcome::GitDeliveryQueued { delivery_id: doc })
}
pub(crate) async fn acknowledge(
	tx: &mut WriteTransaction,
	actor: &Actor,
	id: Uuid,
) -> Result<CommandOutcome, CoreError> {
	let mut doc = load(tx, id).await?;
	if doc.delivery.outcome != GitDeliveryOutcome::OutcomeUnknown {
		return Err(refused("git.outcome_not_unknown"));
	}
	doc.delivery.acknowledged_by = Some(actor.client_id());
	tx.acknowledge_git_delivery(id).await?;
	save(tx, &doc).await?;
	Ok(CommandOutcome::GitDeliveryAcknowledged { delivery_id: id })
}
pub(crate) async fn automate(
	tx: &mut WriteTransaction,
	checkpoint: &ChangeCheckpoint,
	client_id: ClientId,
) -> Result<(), CoreError> {
	if checkpoint.outcome != crate::TurnOutcome::Completed
		|| tx.git_delivery_blocks(checkpoint.conversation_id.0).await?
	{
		return Ok(());
	}
	let mut policy = policy(tx, checkpoint.conversation_id).await?;
	policy.automatic = true;
	let mut steps = Vec::new();
	if policy.branch
		&& (checkpoint.before.tree != checkpoint.after.tree
			|| checkpoint.before.commit != checkpoint.after.commit)
	{
		let name = tx
			.git_branch(checkpoint.conversation_id.0)
			.await?
			.unwrap_or_else(|| {
				format!(
					"{}{}",
					policy.branch_prefix, checkpoint.conversation_id.0
				)
			});
		steps.push(GitOperation::Branch { name });
	}
	if policy.commit {
		steps.push(GitOperation::Commit);
	}
	if policy.push {
		steps.push(GitOperation::Push {
			remote: "origin".into(),
		});
	}
	if policy.draft_pull_request {
		steps.push(GitOperation::DraftPullRequest {
			remote: "origin".into(),
			base: None,
		});
	}
	let actor = Actor::InteractiveClient { client_id };
	let mut previous = None;
	for operation in steps {
		previous = Some(
			queue(
				tx,
				&actor,
				CommandId(Uuid::now_v7()),
				checkpoint.conversation_id,
				Some(checkpoint),
				Step {
					operation,
					policy: policy.clone(),
					previous,
				},
			)
			.await?,
		);
	}
	Ok(())
}
struct Step {
	operation: GitOperation,
	policy: GitDeliveryPolicy,
	previous: Option<Uuid>,
}
async fn queue(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	id: ConversationId,
	checkpoint: Option<&ChangeCheckpoint>,
	step: Step,
) -> Result<Uuid, CoreError> {
	let Step {
		operation,
		policy,
		previous,
	} = step;
	crate::git_delivery_io::validate_operation(&operation)?;
	if matches!(
		operation,
		GitOperation::Commit | GitOperation::DraftPullRequest { .. }
	) && checkpoint.is_none()
	{
		return Err(refused("git.checkpoint_required"));
	}
	let root = root(tx, id).await?;
	let observed = crate::git_delivery_io::inspect(&root, &operation).await;
	// A broken repository must not roll back a completed Harness checkpoint.
	let (head, index, branch, remote_url, failure) = match observed {
		Ok(state) => (
			state.head,
			state.index,
			state.branch,
			state.remote_url,
			None,
		),
		Err(error) if policy.automatic => {
			(String::new(), String::new(), None, None, Some(error.code))
		}
		Err(error) => return Err(error),
	};
	let source = checkpoint.map(|c| GitCheckpoint {
		run_id: c.run_id,
		turn: c.turn,
	});
	let utility_job = if matches!(
		operation,
		GitOperation::Commit | GitOperation::DraftPullRequest { .. }
	) {
		let source =
			source.ok_or_else(|| refused("git.checkpoint_required"))?;
		match crate::utility_work::admit(
			tx,
			actor,
			command_id,
			UtilityRequest::GitText {
				run_id: source.run_id,
				turn: source.turn,
			},
		)
		.await
		{
			Ok(CommandOutcome::UtilityQueued { job_id }) => Some(job_id),
			Ok(_) => return Err(refused("git.invalid_utility")),
			Err(error) if error.code == "utility.queue_full" => None,
			Err(error) => return Err(error),
		}
	} else {
		None
	};
	let delivery_id = Uuid::now_v7();
	let mut doc = Document {
		delivery: GitDelivery {
			message: if source.is_some()
				&& matches!(
					operation,
					GitOperation::Commit
						| GitOperation::DraftPullRequest { .. }
				) {
				Some(crate::GitMessage {
					title: format!(
						"Updated Conversation changes (turn {})",
						source.map_or(0, |c| c.turn)
					),
					body: String::new(),
					fallback_reason: Some(
						if utility_job.is_some() {
							"utility.pending"
						} else {
							"utility.queue_full"
						}
						.into(),
					),
				})
			} else {
				None
			},
			acknowledged_by: None,
			delivery_id,
			conversation_id: id,
			checkpoint: source,
			operation,
			policy,
			utility_job,
			outcome: GitDeliveryOutcome::Pending,
		},
		root,
		client_id: actor.client_id(),
		head,
		index,
		branch,
		remote_url,
		previous,
		tree: checkpoint.map(|c| c.after.tree.clone()),
		prepared_commit: None,
		draft_number: None,
		draft_url: tx.git_draft(id.0).await?,
	};
	if failure.is_none()
		&& let Some(checkpoint) = checkpoint
	{
		// A manual continuation may follow a successful delivery commit. Rebind
		// to the current tip only when its immutable tree is this checkpoint.
		if doc.head != checkpoint.after.commit {
			let current_tree = crate::git_delivery_io::git(
				&doc.root,
				&["rev-parse", "HEAD^{tree}"],
			)
			.await;
			if doc.delivery.policy.automatic
				|| !current_tree
					.as_ref()
					.is_ok_and(|tree| tree == &checkpoint.after.tree)
			{
				doc.delivery.outcome = GitDeliveryOutcome::Failed {
					code: "git.head_changed".into(),
				};
			}
		}
		let dirty_baseline = if doc.delivery.policy.automatic
			&& matches!(doc.delivery.operation, GitOperation::Commit)
		{
			let tree = crate::git_delivery_io::git(
				&doc.root,
				&[
					"rev-parse",
					&format!("{}^{{tree}}", checkpoint.before.commit),
				],
			)
			.await;
			!tree
				.as_ref()
				.is_ok_and(|tree| tree == &checkpoint.before.tree)
		} else {
			false
		};
		if !checkpoint.after.omitted_files.is_empty() || dirty_baseline {
			doc.delivery.outcome = GitDeliveryOutcome::Failed {
				code: "git.incomplete_or_dirty_baseline".into(),
			};
		}
	}
	if let Some(code) = failure {
		doc.delivery.outcome = GitDeliveryOutcome::Failed { code };
	}
	tx.insert_effect(&NewEffect {
		effect_id: delivery_id,
		command_id: command_id.0,
		run_id: source.map(|c| c.run_id.0),
		promotion_id: None,
		terminal_id: None,
		kind: EffectKindRecord::GitDelivery,
		safety: EffectSafetyRecord::Ambiguous,
	})
	.await?;
	if matches!(doc.delivery.outcome, GitDeliveryOutcome::Failed { .. }) {
		tx.begin_effect_attempt(delivery_id).await?;
		tx.finish_effect(delivery_id, jet_store::EffectStateRecord::Failed)
			.await?;
	}
	save(tx, &doc).await?;
	Ok(delivery_id)
}

impl Document {
	pub fn title(&self) -> &str {
		self.delivery
			.message
			.as_ref()
			.map_or("", |message| message.title.as_str())
	}
	pub fn body(&self) -> &str {
		self.delivery
			.message
			.as_ref()
			.map_or("", |message| message.body.as_str())
	}
}
