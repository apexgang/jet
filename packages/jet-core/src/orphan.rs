//! Interactive, durable decisions for executions recovery could not safely match.
use crate::effect::{Effect, EffectAdapter, EffectKind, EffectResult};
use crate::{Actor, CommandId, CommandOutcome, Core, CoreError, RunId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Non-secret, read-only identity of one live helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionMetadata {
	/// Fresh helper instance identity, used to reject stale decisions.
	pub instance: Uuid,
	/// OS identity of the helper.
	pub helper_pid: u32,
	/// Declared canonical working root.
	pub root: String,
	/// Declared registered Project root.
	pub project_root: String,
	/// Helper product version.
	pub version: String,
}
/// A persisted unsafe match, never an automatically resumable Run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrphanedExecution {
	/// Execution identity, which need not exist in authoritative Run state.
	pub execution_id: RunId,
	/// Owner-only metadata if it can safely be inspected.
	pub metadata: Option<ExecutionMetadata>,
}
/// A bounded page of Orphaned executions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanedExecutions {
	/// Metadata only; native source is never exposed here.
	pub executions: Vec<OrphanedExecution>,
	/// Resume after this identity when more executions remain.
	pub next: Option<RunId>,
}
/// Explicit interactive choices. Adoption never bypasses validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAction {
	/// Revalidate and attach to the original authoritative Run.
	Adopt,
	/// Preserve the execution without acknowledging source.
	Leave,
	/// Terminate the exact helper instance and its owned Harness.
	Terminate,
}
/// One user's decision, bound to the metadata they inspected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionResolution {
	/// Execution selected by the user.
	pub execution_id: RunId,
	/// Expected live instance; unavailable metadata permits only Leave.
	pub instance: Option<Uuid>,
	/// Explicit choice.
	pub action: ExecutionAction,
}

impl Core {
	pub(crate) async fn mark_orphan(&self, id: RunId) -> Result<(), CoreError> {
		let metadata = match &self.run_host {
			Some(host) => host.describe(self.run_home(), id).await.ok(),
			None => None,
		};
		let json = serde_json::to_string(&OrphanedExecution {
			execution_id: id,
			metadata,
		})
		.map_err(encoding)?;
		self.store
			.write(async |tx| tx.insert_orphaned_execution(id.0, &json).await)
			.await?;
		Ok(())
	}
	pub(crate) async fn orphaned_executions(
		&self,
		after: Option<RunId>,
	) -> Result<OrphanedExecutions, CoreError> {
		let after = after.map_or_else(String::new, |id| id.0.to_string());
		let rows = self
			.store
			.read(async |tx| tx.orphaned_executions(&after).await)
			.await?;
		let mut executions: Vec<OrphanedExecution> = rows
			.iter()
			.map(|s| crate::run_state::decode(s))
			.collect::<Result<_, _>>()?;
		let next =
			(executions.len() > 100).then(|| executions[99].execution_id);
		executions.truncate(100);
		Ok(OrphanedExecutions { executions, next })
	}
	pub(crate) async fn prepare_execution_resolution(
		&self,
		request: &ExecutionResolution,
	) -> Result<(), CoreError> {
		let state = self
			.store
			.read(async |tx| {
				tx.orphaned_execution(request.execution_id.0).await
			})
			.await?
			.ok_or_else(unavailable)?;
		let orphan: OrphanedExecution = crate::run_state::decode(&state)?;
		if request.instance != orphan.metadata.as_ref().map(|m| m.instance) {
			return Err(unavailable());
		}
		if request.action == ExecutionAction::Leave {
			return Ok(());
		}
		let host = self.run_host.as_ref().ok_or_else(unavailable)?;
		let current =
			host.describe(self.run_home(), request.execution_id).await?;
		if Some(current.instance) != request.instance {
			return Err(unavailable());
		}
		if request.action == ExecutionAction::Adopt {
			let (plan, cursor, terminal) = self
				.recovery_context(request.execution_id)
				.await
				.map_err(|_| unavailable())?;
			if terminal {
				return Err(unavailable());
			}
			host.validate_recovery(
				self.run_home(),
				request.execution_id,
				plan,
				cursor,
			)
			.await
			.map_err(|_| unavailable())?;
		}
		Ok(())
	}
	pub(crate) async fn perform_execution_resolutions(
		self: &Arc<Self>,
	) -> Result<(), CoreError> {
		self.reconcile_effects(
			&mut Resolutions(self),
			jet_store::EffectKindRecord::ResolveExecution,
		)
		.await?;
		Ok(())
	}
}

pub(crate) async fn record(
	tx: &mut jet_store::WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	request: ExecutionResolution,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	if tx
		.orphaned_execution(request.execution_id.0)
		.await?
		.is_none()
	{
		return Err(unavailable());
	}
	let effect_id = Uuid::now_v7();
	tx.insert_effect(&jet_store::NewEffect {
		effect_id,
		command_id: command_id.0,
		run_id: None,
		promotion_id: None,
		kind: jet_store::EffectKindRecord::ResolveExecution,
		safety: jet_store::EffectSafetyRecord::Ambiguous,
	})
	.await?;
	tx.insert_execution_resolution(
		effect_id,
		&serde_json::to_string(&request).map_err(encoding)?,
	)
	.await?;
	crate::audit::record(
		tx,
		actor,
		crate::audit::Decision::succeeded(
			crate::AuditDecision::ExecutionResolutionRequested,
			crate::audit::AuditSubject::Execution(request.execution_id),
		),
		now,
	)
	.await?;
	Ok(CommandOutcome::ExecutionResolutionRecorded(request))
}
struct Resolutions<'a>(&'a Arc<Core>);
impl EffectAdapter for Resolutions<'_> {
	async fn execute(&mut self, effect: &Effect) -> EffectResult {
		if effect.kind != EffectKind::ResolveExecution {
			return EffectResult::Unknown;
		}
		let request = match self
			.0
			.store
			.read(async |tx| tx.execution_resolution(effect.effect_id).await)
			.await
		{
			Ok(Some(json)) => {
				match crate::run_state::decode::<ExecutionResolution>(&json) {
					Ok(request) => request,
					Err(_) => return EffectResult::Unknown,
				}
			}
			Ok(None) | Err(_) => return EffectResult::Unknown,
		};
		if self.0.prepare_execution_resolution(&request).await.is_err() {
			return EffectResult::Failed;
		}
		match request.action {
			ExecutionAction::Leave => EffectResult::Completed,
			ExecutionAction::Adopt => {
				self.0.run_recovery.progressed(request.execution_id);
				match self.0.reconnect_run(request.execution_id).await {
					Ok(()) => EffectResult::Completed,
					Err(_) => EffectResult::Unknown,
				}
			}
			ExecutionAction::Terminate => match &self.0.run_host {
				Some(host) => {
					match host.terminate(self.0.run_home(), request).await {
						Ok(()) => EffectResult::Completed,
						Err(_) => EffectResult::Unknown,
					}
				}
				None => EffectResult::Unknown,
			},
		}
	}
	async fn reconcile(&mut self, _effect: &Effect) -> EffectResult {
		// A lost termination/adoption acknowledgement is surfaced; a later
		// explicit Command may revalidate and try again, never an automatic retry.
		EffectResult::Unknown
	}
}
pub(crate) async fn settle(
	tx: &mut jet_store::WriteTransaction,
	effect: &Effect,
	state: jet_store::EffectStateRecord,
) -> Result<(), CoreError> {
	if state == jet_store::EffectStateRecord::Completed {
		let json = tx
			.execution_resolution(effect.effect_id)
			.await?
			.ok_or_else(unavailable)?;
		let request: ExecutionResolution = crate::run_state::decode(&json)?;
		if request.action != ExecutionAction::Leave {
			tx.remove_orphaned_execution(request.execution_id.0).await?;
		}
	}
	Ok(())
}
fn encoding(error: serde_json::Error) -> CoreError {
	CoreError::internal("execution.encode", error.to_string())
}
fn unavailable() -> CoreError {
	CoreError::conflict(
		"execution.unsafe_resolution",
		"the execution cannot be resolved from this metadata",
	)
}
