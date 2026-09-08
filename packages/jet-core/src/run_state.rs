//! Atomic projection and semantic Event updates for managed executions.
use crate::turn_dispatch::Settlement;
use crate::{
	ConversationId, Core, CoreError, EventKind, ManagedProcess,
	ManagedProcessRole, RunActivity, RunId, RunLifecycle,
};
use jet_store::WriteTransaction;
use serde::{Deserialize, Serialize};

pub(crate) use crate::run_observation::{
	Observation, SourceBoundary, SourcePrefix,
};
pub(crate) use crate::run_state_storage::{append, decode, save, snapshot};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct State {
	#[serde(default)]
	pub(crate) changes: Option<crate::checkpoint_state::Tracking>,
	pub(crate) activity: Option<RunActivity>,
	#[serde(default)]
	pub(crate) disconnected: bool,
	pub(crate) processes: Vec<ManagedProcess>,
	pub(crate) native_conversation: Option<String>,
	pub(crate) exit_code: Option<i32>,
	#[serde(default)]
	pub(crate) source_offset: u64,
	#[serde(default)]
	pub(crate) checkpoint: String,
	#[serde(default)]
	pub(crate) partial_source: SourcePrefix,
	/// The admitted interactive control request, kept until the execution
	/// settles it (ADR-0083).
	#[serde(default)]
	pub(crate) control: Option<crate::execution_control::ControlRequest>,
	/// The recorded terminal outcome of that request.
	#[serde(default)]
	pub(crate) termination: Option<crate::RunTermination>,
}

impl Core {
	pub(crate) async fn source_prefix(
		&self,
		run_id: RunId,
	) -> Result<SourcePrefix, CoreError> {
		self.store
			.read(async |tx| {
				let record =
					tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
				Ok(decode::<State>(&record.state)?.partial_source)
			})
			.await
	}
	pub(crate) async fn commit_run_source(
		&self,
		run_id: RunId,
		observations: Vec<Observation>,
		boundary: SourceBoundary,
	) -> Result<(), CoreError> {
		let now = self.now_unix_ms();
		let terminal = self.store.write(async |tx| {
            let execution = tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
            let state: State = decode(&execution.state)?;
            if matches!(&boundary, SourceBoundary::Complete { offset, .. } if *offset <= state.source_offset) { return Err(invalid()); }
            let mut prefix = state.partial_source;
            for observation in observations {
                prefix.include(&observation)?;
                crate::checkpoint_state::observe(self, tx, run_id, &observation).await?;
                record(tx, run_id, observation, now).await?;
            }
            // ADR-0071: bounded groups commit with their replay prefix. Source
            // remains retained until its final parser checkpoint commits.
            if let SourceBoundary::Complete { offset, checkpoint } = boundary {
                record(tx, run_id, Observation::Progress { offset, checkpoint }, now).await?;
                prefix = SourcePrefix::default();
            }
            let execution = tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
            let mut state: State = decode(&execution.state)?;
            state.partial_source = prefix;
            tx.update_run_execution(run_id.0, &serde_json::to_string(&state).map_err(|_| invalid())?).await?;
            let terminal = tx.run(run_id.0).await?.ok_or_else(missing)?.lifecycle.is_terminal();
            Ok::<_, CoreError>(terminal && state.partial_source.count == 0)
        }).await?;
		if terminal {
			self.run_work.notify_one();
		}
		Ok(())
	}
	pub(crate) async fn observe_run(
		&self,
		run_id: RunId,
		observation: Observation,
	) -> Result<(), CoreError> {
		let now = self.now_unix_ms();
		let terminal = matches!(
			observation,
			Observation::Lost
				| Observation::LaunchFailed
				| Observation::Ended(_)
		);
		self.store
			.write(async |tx| {
				crate::checkpoint_state::observe(
					self,
					tx,
					run_id,
					&observation,
				)
				.await?;
				record(tx, run_id, observation, now).await
			})
			.await?;
		if terminal {
			self.run_work.notify_one();
		}
		Ok(())
	}
}

async fn record(
	tx: &mut WriteTransaction,
	run_id: RunId,
	observation: Observation,
	now: i64,
) -> Result<(), CoreError> {
	let record = tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
	let plan: crate::LaunchPlan = decode(&record.plan)?;
	let authorized_by = plan.client_id;
	let mut state: State = decode(&record.state)?;
	let run = tx.run(run_id.0).await?.ok_or_else(missing)?;
	if matches!(
		&observation,
		Observation::ConversationTitle(_)
			| Observation::RunTitle(_)
			| Observation::ProcessTitle { .. }
	) && !executing(run.lifecycle)
	{
		return Err(invalid());
	}
	let actor = match &observation {
		Observation::FileChanged(_)
		| Observation::TurnStarted
		| Observation::TurnEnded(_)
		| Observation::Completed(_)
		| Observation::Activity(_)
		| Observation::TurnCompleted { .. }
		| Observation::Output { .. }
		| Observation::NativeConversation(_)
		| Observation::ConversationTitle(_)
		| Observation::RunTitle(_)
		| Observation::ProcessTitle { .. } => crate::EventActor::Harness {
			run_id,
			authorized_by,
		},
		Observation::Started { .. }
		| Observation::Progress { .. }
		| Observation::Ended(_)
		| Observation::LaunchFailed
		| Observation::Lost
		| Observation::Reconnected
		| Observation::Disconnected => crate::EventActor::RunSupervisor {
			run_id,
			authorized_by,
		},
	};
	let observation = match observation {
		Observation::ConversationTitle(title) => {
			crate::name::apply_harness_conversation(
				tx,
				&actor,
				ConversationId(run.conversation_id),
				run_id,
				title,
				now,
			)
			.await?;
			return Ok(());
		}
		Observation::RunTitle(title) => {
			crate::name::apply_harness_run(tx, &actor, run_id, title, now)
				.await?;
			return Ok(());
		}
		other => other,
	};
	let settlement = match &observation {
		Observation::TurnCompleted { turn_id, .. } => {
			Some(Settlement::Completed { turn_id: *turn_id })
		}
		Observation::NativeConversation(_) | Observation::Completed(_) => {
			Some(Settlement::Completed {
				turn_id: plan.turn_id.unwrap_or(run_id.0),
			})
		}
		Observation::Ended(_)
		| Observation::LaunchFailed
		| Observation::Lost => Some(Settlement::Failed),
		Observation::Disconnected | Observation::Reconnected => {
			Some(Settlement::OutcomeUnknown)
		}
		// A cancelled turn releases the queue: the Run itself is still
		// alive and takes the next admitted input (ADR-0083).
		Observation::TurnEnded(crate::TurnOutcome::Interrupted)
			if state.control.is_some_and(|request| {
				request.control == crate::RunControl::InterruptTurn
			}) =>
		{
			Some(Settlement::Canceled)
		}
		Observation::Started { .. }
		| Observation::FileChanged(_)
		| Observation::TurnStarted
		| Observation::TurnEnded(_)
		| Observation::Activity(_)
		| Observation::Output { .. }
		| Observation::ProcessTitle { .. }
		| Observation::ConversationTitle(_)
		| Observation::RunTitle(_)
		| Observation::Progress { .. } => None,
	};
	let (lifecycle, events) = apply(run.lifecycle, &mut state, observation)?;
	if let Some(outcome) = settlement {
		crate::turn_dispatch::settle(tx, run_id, outcome, now).await?;
	}
	let event_run: crate::Run = run.clone().into();
	if lifecycle != run.lifecycle {
		let from = run.lifecycle;
		tx.update_run_lifecycle(run_id.0, lifecycle, now).await?;
		append(
			tx,
			&actor,
			&event_run,
			EventKind::RunLifecycleChanged {
				from,
				to: lifecycle,
			},
			now,
		)
		.await?;
	} else if !events.is_empty() {
		tx.update_run_lifecycle(run_id.0, lifecycle, now).await?;
	}
	for event in events {
		append(tx, &actor, &event_run, event, now).await?;
	}
	save(tx, run_id, &state).await?;
	Ok::<_, CoreError>(())
}

pub(crate) async fn settle_start(
	tx: &mut WriteTransaction,
	run_id: RunId,
	state: jet_store::EffectStateRecord,
	now: i64,
) -> Result<(), CoreError> {
	if tx.run_execution(run_id.0).await?.is_some()
		&& state == jet_store::EffectStateRecord::Failed
	{
		record(tx, run_id, Observation::LaunchFailed, now).await?;
	}
	Ok(())
}

/// Whether the execution is still running. A Run being stopped keeps
/// producing output until its native process actually ends, and none of it
/// is discarded because a control request was admitted (ADR-0083).
fn executing(lifecycle: RunLifecycle) -> bool {
	matches!(lifecycle, RunLifecycle::Active | RunLifecycle::Stopping)
}

fn apply(
	lifecycle: RunLifecycle,
	state: &mut State,
	observation: Observation,
) -> Result<(RunLifecycle, Vec<EventKind>), CoreError> {
	let mut next = lifecycle;
	let mut events = Vec::new();
	match observation {
		Observation::TurnEnded(outcome) if executing(lifecycle) => {
			// A Craft that cancelled the named turn natively answers the
			// request without ending the Run.
			if outcome == crate::TurnOutcome::Interrupted
				&& let Some(request) = state.control
				&& request.control == crate::RunControl::InterruptTurn
			{
				let termination = crate::RunTermination {
					control: request.control,
					stage: crate::TerminationStage::NativeCancellation,
				};
				state.control = None;
				state.termination = Some(termination);
				events.push(EventKind::RunTerminated { termination });
			}
		}
		Observation::FileChanged(_) | Observation::TurnStarted
			if executing(lifecycle) => {}
		Observation::Completed(identity) => {
			return apply(
				lifecycle,
				state,
				Observation::NativeConversation(identity),
			);
		}
		Observation::Lost => {
			// Proven process death makes the remaining source boundary unavailable.
			// Committed semantic Events remain durable; no source is acknowledged.
			state.partial_source = SourcePrefix::default();
			if !lifecycle.is_terminal() {
				next = RunLifecycle::Lost;
				state.disconnected = false;
				activity(state, None, &mut events);
				for process in &mut state.processes {
					process.running = false;
				}
				events.push(EventKind::RunProcessesChanged {
					processes: state.processes.clone(),
				});
			}
		}
		Observation::Progress { offset, checkpoint } => {
			if offset <= state.source_offset || checkpoint.len() > 65_536 {
				return Err(invalid());
			}
			state.source_offset = offset;
			state.checkpoint = checkpoint;
		}
		Observation::Started {
			helper_pid,
			harness_pid,
		} if lifecycle == RunLifecycle::Starting
			&& helper_pid > 0
			&& harness_pid > 0
			&& helper_pid != harness_pid =>
		{
			next = RunLifecycle::Active;
			state.processes = vec![
				ManagedProcess {
					pid: helper_pid,
					role: ManagedProcessRole::Helper,
					label: None,
					running: true,
				},
				ManagedProcess {
					pid: harness_pid,
					role: ManagedProcessRole::Harness,
					label: None,
					running: true,
				},
			];
			events.push(EventKind::RunProcessesChanged {
				processes: state.processes.clone(),
			});
			activity(state, Some(RunActivity::Working), &mut events);
		}
		Observation::Activity(reason) if executing(lifecycle) => {
			activity(state, Some(reason), &mut events)
		}
		Observation::ProcessTitle { pid, title } if executing(lifecycle) => {
			let label = crate::Name::process_label(title)?;
			let Some(process) = state
				.processes
				.iter_mut()
				.find(|process| process.pid == pid)
			else {
				return Err(invalid());
			};
			if process.label.as_deref() != Some(&label) {
				process.label = Some(label);
				events.push(EventKind::RunProcessesChanged {
					processes: state.processes.clone(),
				});
			}
		}
		Observation::Reconnected => {
			if state.disconnected {
				state.disconnected = false;
				events.push(EventKind::RunActivityChanged {
					activity: state.activity,
				});
			}
		}
		Observation::Disconnected if executing(lifecycle) => {
			if !state.disconnected {
				state.disconnected = true;
				events.push(EventKind::RunActivityChanged {
					activity: Some(RunActivity::Reconnecting),
				});
			}
		}
		Observation::Disconnected
			if lifecycle == RunLifecycle::Starting
				|| lifecycle.is_terminal() => {}
		Observation::Output {
			native_json,
			presentation_json,
		} if executing(lifecycle)
			&& native_json.len()
				+ presentation_json.iter().map(String::len).sum::<usize>()
				<= 128 * 1024
			&& presentation_json.len() <= 128 =>
		{
			events.push(EventKind::RunOutput {
				native_json,
				presentation_json,
			})
		}
		Observation::NativeConversation(identity)
		| Observation::TurnCompleted {
			native_conversation: identity,
			..
		} if executing(lifecycle)
			&& !identity.is_empty()
			&& identity.len() <= 4096 =>
		{
			state.native_conversation = Some(identity.clone());
			events.push(EventKind::RunNativeConversation {
				native_conversation: identity,
			});
		}
		Observation::Ended(code) if executing(lifecycle) => {
			// An admitted control request owns this end: the work stopped
			// because it was asked to, whatever status the OS reported.
			next = if state.control.is_some() {
				RunLifecycle::Canceled
			} else if code == Some(0) {
				RunLifecycle::Completed
			} else {
				RunLifecycle::Failed
			};
			state.exit_code = code;
			activity(state, None, &mut events);
			for process in &mut state.processes {
				process.running = false;
			}
			events.push(EventKind::RunProcessesChanged {
				processes: state.processes.clone(),
			});
		}
		Observation::LaunchFailed if lifecycle == RunLifecycle::Starting => {
			next = RunLifecycle::Failed
		}
		Observation::Started { .. }
		| Observation::FileChanged(_)
		| Observation::TurnStarted
		| Observation::TurnEnded(_)
		| Observation::Activity(_)
		| Observation::TurnCompleted { .. }
		| Observation::Output { .. }
		| Observation::NativeConversation(_)
		| Observation::ProcessTitle { .. }
		| Observation::ConversationTitle(_)
		| Observation::RunTitle(_)
		| Observation::Ended(_)
		| Observation::LaunchFailed
		| Observation::Disconnected => return Err(invalid()),
	}
	Ok((next, events))
}

fn activity(
	state: &mut State,
	activity: Option<RunActivity>,
	events: &mut Vec<EventKind>,
) {
	if state.activity != activity {
		state.activity = activity;
		events.push(EventKind::RunActivityChanged { activity });
	}
}

fn missing() -> CoreError {
	CoreError::not_found(
		"run.execution_not_found",
		"the managed Run does not exist",
	)
}
fn invalid() -> CoreError {
	CoreError::conflict(
		"run.invalid_observation",
		"the Craft observation conflicts with the Run lifecycle",
	)
}
