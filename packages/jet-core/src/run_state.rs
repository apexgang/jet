//! Atomic projection and semantic Event updates for managed executions.
use crate::event::EventSubject;
use crate::{
	ConversationId, Core, CoreError, EventKind, EventSequence, ManagedProcess,
	ManagedProcessRole, Run, RunActivity, RunExecution, RunId, RunLifecycle,
};
use jet_store::{ReadTransaction, WriteTransaction};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct State {
	pub(crate) activity: Option<RunActivity>,
	pub(crate) processes: Vec<ManagedProcess>,
	pub(crate) native_conversation: Option<String>,
	pub(crate) exit_code: Option<i32>,
	#[serde(default)]
	pub(crate) source_offset: u64,
}

pub(crate) async fn snapshot(
	tx: &mut ReadTransaction,
	run_id: RunId,
) -> Result<RunExecution, CoreError> {
	let run = tx.run(run_id.0).await?.ok_or_else(missing)?;
	let record = tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
	let state: State = decode(&record.state)?;
	Ok(RunExecution {
		cursor: EventSequence(tx.event_cursor().await?),
		run: run.into(),
		activity: state.activity,
		processes: state.processes,
		native_conversation: state.native_conversation,
		exit_code: state.exit_code,
	})
}

/// Facts from the trusted Run Adapter, validated against the durable lifecycle.
pub enum Observation {
	/// The helper reported that it spawned a Harness.
	Started {
		/// Actual helper OS identity.
		helper_pid: u32,
		/// Native OS identity supplied by the helper.
		harness_pid: u32,
	},
	/// An active Harness began working or waiting.
	Activity(RunActivity),
	/// Lossless native JSON and its portable views.
	Output {
		/// Original native JSON bytes.
		native_json: String,
		/// Portable Presentation blocks, preserving unknown data.
		presentation_json: Vec<String>,
	},
	/// Native identity for a later explicit resume.
	NativeConversation(String),
	/// End offset of source whose observations preceded this marker.
	Progress(u64),
	/// Reaped native exit status, absent for signal termination.
	Ended(Option<i32>),
	/// Definite launch rejection with no surviving Harness.
	LaunchFailed,
	/// The supervising connection was lost.
	Disconnected,
}

impl Core {
	pub(crate) async fn observe_run(
		&self,
		run_id: RunId,
		observation: Observation,
	) -> Result<(), CoreError> {
		let now = self.now_unix_ms();
		self.store
			.write(async |tx| record(tx, run_id, observation, now).await)
			.await
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
	let actor = match &observation {
		Observation::Activity(_)
		| Observation::Output { .. }
		| Observation::NativeConversation(_) => crate::EventActor::Harness {
			run_id,
			authorized_by,
		},
		Observation::Started { .. }
		| Observation::Progress(_)
		| Observation::Ended(_)
		| Observation::LaunchFailed
		| Observation::Disconnected => crate::EventActor::RunSupervisor {
			run_id,
			authorized_by,
		},
	};
	let (lifecycle, events) = apply(run.lifecycle, &mut state, observation)?;
	if lifecycle != run.lifecycle {
		tx.update_run_lifecycle(run_id.0, lifecycle, now).await?;
		append(
			tx,
			&actor,
			&run.into(),
			EventKind::RunLifecycleChanged {
				from: run.lifecycle,
				to: lifecycle,
			},
			now,
		)
		.await?;
	} else if !events.is_empty() {
		tx.update_run_lifecycle(run_id.0, lifecycle, now).await?;
	}
	for event in events {
		append(tx, &actor, &run.into(), event, now).await?;
	}
	tx.update_run_execution(
		run_id.0,
		&serde_json::to_string(&state)
			.map_err(|e| CoreError::internal("run.encode", e.to_string()))?,
	)
	.await?;
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

fn apply(
	lifecycle: RunLifecycle,
	state: &mut State,
	observation: Observation,
) -> Result<(RunLifecycle, Vec<EventKind>), CoreError> {
	let mut next = lifecycle;
	let mut events = Vec::new();
	match observation {
		Observation::Progress(offset) => {
			if offset <= state.source_offset {
				return Err(invalid());
			}
			state.source_offset = offset;
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
					running: true,
				},
				ManagedProcess {
					pid: harness_pid,
					role: ManagedProcessRole::Harness,
					running: true,
				},
			];
			events.push(EventKind::RunProcessesChanged {
				processes: state.processes.clone(),
			});
			activity(state, Some(RunActivity::Working), &mut events);
		}
		Observation::Activity(reason) if lifecycle == RunLifecycle::Active => {
			activity(state, Some(reason), &mut events)
		}
		Observation::Disconnected if lifecycle == RunLifecycle::Active => {
			activity(state, Some(RunActivity::Reconnecting), &mut events)
		}
		Observation::Disconnected
			if lifecycle == RunLifecycle::Starting
				|| lifecycle.is_terminal() => {}
		Observation::Output {
			native_json,
			presentation_json,
		} if lifecycle == RunLifecycle::Active
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
			if lifecycle == RunLifecycle::Active
				&& !identity.is_empty()
				&& identity.len() <= 4096 =>
		{
			state.native_conversation = Some(identity.clone());
			events.push(EventKind::RunNativeConversation {
				native_conversation: identity,
			});
		}
		Observation::Ended(code) if lifecycle == RunLifecycle::Active => {
			next = if code == Some(0) {
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
		| Observation::Activity(_)
		| Observation::Output { .. }
		| Observation::NativeConversation(_)
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

async fn append(
	tx: &mut WriteTransaction,
	actor: &crate::EventActor,
	run: &Run,
	event: EventKind,
	now: i64,
) -> Result<(), CoreError> {
	let record = event.to_record_as(
		actor.clone(),
		EventSubject::Run {
			conversation_id: ConversationId(run.conversation_id.0),
			run_id: run.run_id,
		},
		now,
	)?;
	tx.append_event(record).await?;
	Ok(())
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(
	json: &str,
) -> Result<T, CoreError> {
	serde_json::from_str(json)
		.map_err(|e| CoreError::internal("run.invalid_record", e.to_string()))
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
