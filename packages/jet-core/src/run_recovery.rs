//! Reconnect only executions matched to authoritative state; never relaunch them.
use crate::run_state::{self, Observation, State};
use crate::{
	Core, CoreError, RunId, RunRecoveryCursor, RunRecoveryError, WorkingTree,
};
use std::{
	collections::{HashMap, HashSet},
	sync::{Arc, Mutex},
	time::{Duration, Instant},
};

#[derive(Default)]
pub(crate) struct Recovery {
	pub(crate) monitors: Mutex<HashSet<RunId>>,
	/// The pinned connection each supervised execution is reachable
	/// through, so an admitted control request can ask its Craft to cancel
	/// natively instead of signalling the process (ADR-0083).
	pub(crate) connections:
		Mutex<HashMap<RunId, Arc<dyn crate::RunConnection>>>,
	retries: Mutex<HashMap<RunId, (u32, Instant)>>,
}

impl std::fmt::Debug for Recovery {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Recovery").finish_non_exhaustive()
	}
}

impl Recovery {
	pub(crate) fn failed(&self, id: RunId) -> bool {
		let mut retries = self.retries.lock().expect("retry lock");
		let (attempt, next) = retries.entry(id).or_insert((0, Instant::now()));
		*attempt = (*attempt + 1).min(3);
		*next = Instant::now() + Duration::from_secs(1 << *attempt);
		*attempt >= 3
	}
	pub(crate) fn progressed(&self, id: RunId) {
		self.retries.lock().expect("retry lock").remove(&id);
	}
}

impl Core {
	/// The live connection of a supervised execution, when one is attached.
	pub(crate) fn live_connection(
		&self,
		id: RunId,
	) -> Option<Arc<dyn crate::RunConnection>> {
		self.run_recovery
			.connections
			.lock()
			.expect("connection lock")
			.get(&id)
			.map(Arc::clone)
	}
}

impl Core {
	/// Reconciles live helper identities and previously active Runs. Craft retries
	/// use bounded backoff and keep the exact accepted artifact pinned.
	///
	/// # Errors
	/// Returns a store or discovery error; failures never authorize native starts.
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "serializes adoption with launch and interactive resolution Effects"
	)]
	pub async fn recover_runs(self: &Arc<Self>) -> Result<(), CoreError> {
		let Some(host) = &self.run_host else {
			return Ok(());
		};
		let _gate = self.effect_reconciliation.lock().await;
		let mut ids: HashSet<_> =
			host.discover(self.run_home()).await?.into_iter().collect();
		let mut after = String::new();
		loop {
			let page = self
				.store
				.read(async |tx| tx.active_execution_ids(&after).await)
				.await?;
			let Some(last) = page.last() else {
				break;
			};
			after = last.to_string();
			ids.extend(page.into_iter().map(RunId));
		}

		let mut after = None;
		loop {
			let page = self.orphaned_executions(after).await?;
			ids.extend(
				page.executions
					.into_iter()
					.filter(|o| o.role == crate::ExecutionRole::Run)
					.map(|o| o.execution_id),
			);
			after = page.next;
			if after.is_none() {
				break;
			}
		}
		for id in ids {
			if self
				.run_recovery
				.monitors
				.lock()
				.expect("monitor lock")
				.contains(&id)
			{
				continue;
			}
			let (execution, orphan) = self
				.store
				.read(async |tx| {
					Ok::<_, CoreError>((
						tx.run_execution(id.0).await?,
						tx.orphaned_execution(id.0).await?,
					))
				})
				.await?;
			let helper_pid = execution
				.as_ref()
				.and_then(|r| run_state::decode::<State>(&r.state).ok())
				.and_then(|state| {
					state
						.processes
						.into_iter()
						.find(|p| p.role == crate::ManagedProcessRole::Helper)
						.map(|p| p.pid)
				})
				.or_else(|| {
					orphan
						.as_ref()
						.and_then(|json| {
							run_state::decode::<crate::OrphanedExecution>(json)
								.ok()
						})
						.and_then(|o| o.metadata.map(|m| m.helper_pid))
				});
			let accepted = execution.as_ref().and_then(|record| {
				run_state::decode::<crate::LaunchPlan>(&record.plan).ok()
			});
			if matches!(
				host.probe(self.run_home(), id, accepted, helper_pid).await,
				Err(RunRecoveryError::Gone)
			) {
				if execution.is_some() {
					self.observe_run(id, Observation::Lost).await?;
				}
				self.store
					.write(async |tx| tx.remove_orphaned_execution(id.0).await)
					.await?;
				self.run_recovery.progressed(id);
				continue;
			}
			// Leaving an orphan suppresses adoption, never liveness reconciliation.
			if orphan.is_some() {
				self.mark_orphan(id).await?;
				continue;
			}
			if self
				.run_recovery
				.retries
				.lock()
				.expect("retry lock")
				.get(&id)
				.is_some_and(|(_, next)| *next > Instant::now())
			{
				continue;
			}
			match self.reconnect_run(id).await {
				Ok(()) => {}
				Err(RunRecoveryError::Gone) => {
					self.observe_run(id, Observation::Lost).await?;
				}
				Err(RunRecoveryError::Unsafe) => {
					self.mark_orphan(id).await?;
				}
				Err(RunRecoveryError::Unavailable) => {
					let _ =
						self.observe_run(id, Observation::Disconnected).await;
					if self.run_recovery.failed(id) {
						self.mark_orphan(id).await?;
					}
				}
			}
		}
		Ok(())
	}

	pub(crate) async fn recovery_context(
		&self,
		id: RunId,
	) -> Result<(crate::LaunchPlan, RunRecoveryCursor, bool), RunRecoveryError>
	{
		let (plan, state, terminal) = self
			.store
			.read(async |tx| {
				let execution =
					tx.run_execution(id.0).await?.ok_or_else(unsafe_state)?;
				let run = tx.run(id.0).await?.ok_or_else(unsafe_state)?;
				let plan: crate::LaunchPlan =
					run_state::decode(&execution.plan)?;
				let state: State = run_state::decode(&execution.state)?;
				let conversation = tx
					.conversation(run.conversation_id)
					.await?
					.ok_or_else(unsafe_state)?;
				let (project_id, workspace) =
					match WorkingTree::from(conversation.working_tree) {
						WorkingTree::Workspace { project_id } => (
							project_id,
							tx.workspace_of(run.conversation_id).await?,
						),
						WorkingTree::LocalCheckout { project_id } => {
							(project_id, None)
						}
						WorkingTree::NoProject => return Err(unsafe_state()),
					};
				let project =
					tx.project(project_id.0).await?.ok_or_else(unsafe_state)?;
				let root = match workspace {
					Some(workspace) if workspace.project_id == project_id.0 => {
						workspace.root
					}
					Some(_) => return Err(unsafe_state()),
					None if matches!(
						conversation.working_tree,
						jet_store::WorkingTreeRecord::LocalCheckout { .. }
					) =>
					{
						project.root.clone()
					}
					None => return Err(unsafe_state()),
				};
				if plan.root != std::path::Path::new(&root)
					|| plan.project_root != std::path::Path::new(&project.root)
				{
					return Err(unsafe_state());
				}
				let finished = run.lifecycle.is_terminal()
					&& state.partial_source.count == 0;
				Ok::<_, CoreError>((plan, state, finished))
			})
			.await
			.map_err(|_| RunRecoveryError::Unsafe)?;
		let cursor = RunRecoveryCursor {
			offset: state.source_offset,
			checkpoint: state.checkpoint,
			helper_pid: state
				.processes
				.iter()
				.find(|p| p.role == crate::ManagedProcessRole::Helper)
				.map(|p| p.pid),
		};
		Ok((plan, cursor, terminal))
	}

	pub(crate) async fn reconnect_run(
		self: &Arc<Self>,
		id: RunId,
	) -> Result<(), RunRecoveryError> {
		let (plan, cursor, terminal) = self.recovery_context(id).await?;
		self.craft_recovery_allowed(&plan.craft)
			.await
			.map_err(|_| RunRecoveryError::Unsafe)?;
		let connection = self
			.run_host
			.as_ref()
			.ok_or(RunRecoveryError::Unsafe)?
			.recover(self.run_home(), id, plan, cursor)
			.await?;
		if terminal {
			connection
				.finish()
				.await
				.map_err(|_| RunRecoveryError::Unavailable)?;
		} else {
			self.observe_run(id, Observation::Reconnected)
				.await
				.map_err(|_| RunRecoveryError::Unavailable)?;
			crate::run_effect::spawn_monitor(
				Arc::clone(self),
				id,
				connection,
				vec![],
			);
		}
		Ok(())
	}
}
fn unsafe_state() -> CoreError {
	CoreError::conflict(
		"run.unsafe_recovery",
		"the execution does not match authoritative Plane state",
	)
}
