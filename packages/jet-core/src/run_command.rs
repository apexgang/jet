//! Managed Run admission and immutable launch plans (ADR-0064, ADR-0086).

use crate::run_craft::PinnedCraft;
use crate::{
	Actor, CommandId, CommandOutcome, ConversationId, Core, CoreError,
	RunLifecycle, WorkingTree, filesystem, repository,
};
use jet_store::{
	EffectKindRecord, EffectSafetyRecord, NewEffect, RunExecutionRecord,
	WriteTransaction,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Durable domain plan: authority, working roots, and the exact accepted artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchPlan {
	/// Domain plan format version.
	pub version: u32,
	/// Canonical permitted working root.
	pub root: PathBuf,
	/// Canonical registered Project root.
	pub project_root: PathBuf,
	/// Immutable accepted executable and opaque Adapter contract.
	pub craft: PinnedCraft,
	/// Authorized initial input.
	pub prompt: String,
	/// Client that authorized admission; separate from subsequent Event origins.
	pub client_id: crate::ClientId,
}

pub(crate) async fn prepare(
	core: &Core,
	actor: &Actor,
	conversation_id: ConversationId,
	craft: &str,
	prompt: &str,
) -> Result<LaunchPlan, CoreError> {
	if prompt.is_empty() || prompt.len() > 64 * 1024 {
		return Err(CoreError::invalid_input(
			"run.invalid_prompt",
			"initial input must contain 1 to 65536 bytes",
		));
	}
	let (root, project_root) = core
		.store
		.read(async |tx| {
			let conversation =
				tx.conversation(conversation_id.0).await?.ok_or_else(|| {
					CoreError::not_found(
						"conversation.not_found",
						"the Conversation does not exist",
					)
				})?;
			let working_tree = WorkingTree::from(conversation.working_tree);
			let project_id = match working_tree {
				WorkingTree::NoProject => {
					return Err(CoreError::invalid_input(
						"run.project_required",
						"a managed Run requires a registered Project",
					));
				}
				WorkingTree::Workspace { project_id }
				| WorkingTree::LocalCheckout { project_id } => project_id,
			};
			let project = tx.project(project_id.0).await?.ok_or_else(|| {
				CoreError::not_found(
					"project.not_found",
					"the Project is no longer registered",
				)
			})?;
			let project_root = PathBuf::from(project.root);
			let root = match working_tree {
				WorkingTree::Workspace { .. } => {
					let workspace = tx
						.workspace_of(conversation_id.0)
						.await?
						.ok_or_else(root_invalid)?;
					if workspace.project_id != project_id.0 {
						return Err(root_invalid());
					}
					PathBuf::from(workspace.root)
				}
				WorkingTree::LocalCheckout { .. } => project_root.clone(),
				WorkingTree::NoProject => unreachable!("rejected above"),
			};
			Ok::<_, CoreError>((root, project_root))
		})
		.await?;
	let plan = LaunchPlan {
		version: 1,
		root,
		project_root,
		craft: core
			.run_host
			.as_ref()
			.ok_or_else(|| {
				CoreError::conflict(
					"craft.unavailable",
					"no Run transport was configured",
				)
			})?
			.pin(core.run_home(), craft.into())
			.await?,
		prompt: prompt.into(),
		client_id: actor.client_id(),
	};
	plan.revalidate().await?;
	Ok(plan)
}

impl LaunchPlan {
	/// Rechecks roots and artifact immediately before external work.
	///
	/// # Errors
	/// Returns a conflict or unavailable error when the accepted boundary changed.
	pub async fn revalidate(&self) -> Result<(), CoreError> {
		if self.version != 1 {
			return Err(CoreError::conflict(
				"run.incompatible_pin",
				"this execution requires an unavailable protocol version",
			));
		}
		// ASVS 2.2.3, 5.3.2: validate canonical roots and their Git relationship,
		// again at execution, so a stale Command cannot launch in a replaced tree.
		for root in [&self.root, &self.project_root] {
			if filesystem::canonicalize(root.clone())
				.await
				.map_err(|_| root_invalid())?
				!= *root || repository::verdict(root).await?
				!= repository::Verdict::Registrable
			{
				return Err(root_invalid());
			}
		}
		let arguments =
			["rev-parse", "--path-format=absolute", "--git-common-dir"];
		let project = repository::git(&self.project_root, &arguments).await?;
		let workspace = repository::git(&self.root, &arguments).await?;
		if !project.status.success()
			|| !workspace.status.success()
			|| project.stdout != workspace.stdout
		{
			return Err(root_invalid());
		}
		self.craft.verify().await
	}
}

pub(crate) async fn record(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	conversation_id: ConversationId,
	plan: LaunchPlan,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let CommandOutcome::RunCreated(run) =
		crate::command::create_run(tx, actor, conversation_id, now).await?
	else {
		unreachable!("Run creation")
	};
	let state = serde_json::json!({"activity":null,"processes":[],"native_conversation":null,"exit_code":null});
	tx.insert_run_execution(
		run.run_id.0,
		&RunExecutionRecord {
			plan: serde_json::to_string(&plan).map_err(|e| {
				CoreError::internal("run.encode", e.to_string())
			})?,
			state: state.to_string(),
		},
	)
	.await?;
	let run = tx
		.update_run_lifecycle(run.run_id.0, RunLifecycle::Starting, now)
		.await?;
	tx.append_event(
		crate::EventKind::RunLifecycleChanged {
			from: RunLifecycle::Created,
			to: RunLifecycle::Starting,
		}
		.to_record(
			actor,
			crate::event::EventSubject::Run {
				conversation_id,
				run_id: crate::RunId(run.run_id),
			},
			now,
		)?,
	)
	.await?;
	tx.insert_effect(&NewEffect {
		effect_id: Uuid::now_v7(),
		command_id: command_id.0,
		run_id: Some(run.run_id),
		promotion_id: None,
		kind: EffectKindRecord::StartRun,
		safety: EffectSafetyRecord::Ambiguous,
	})
	.await?;
	Ok(CommandOutcome::RunCreated(run.into()))
}

fn root_invalid() -> CoreError {
	CoreError::conflict(
		"run.working_tree_unavailable",
		"the registered working tree is unavailable or changed",
	)
}

impl Core {
	pub(crate) fn run_home(&self) -> PathBuf {
		self.workspace_home
			.0
			.parent()
			.expect("Workspace home has a parent")
			.to_path_buf()
	}
}
