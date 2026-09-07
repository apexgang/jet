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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SourcePrefix {
	pub(crate) count: usize,
	pub(crate) digest: String,
}
impl SourcePrefix {
	pub(crate) fn include(
		&mut self,
		observation: &Observation,
	) -> Result<(), CoreError> {
		use sha2::{Digest, Sha256};
		let bytes = serde_json::to_vec(observation).map_err(|_| invalid())?;
		let mut hash = Sha256::new();
		hash.update(self.digest.as_bytes());
		hash.update(bytes);
		self.digest = format!("{:x}", hash.finalize());
		self.count += 1;
		Ok(())
	}
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
		activity: if state.disconnected {
			Some(RunActivity::Reconnecting)
		} else {
			state.activity
		},
		processes: state.processes,
		native_conversation: state.native_conversation,
		exit_code: state.exit_code,
	})
}

/// Facts from the trusted Run Adapter, validated against the durable lifecycle.
#[derive(Serialize)]
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
	Progress {
		/// End of the source batch.
		offset: u64,
		/// Adapter parser state at that boundary.
		checkpoint: String,
	},
	/// Reaped native exit status, absent for signal termination.
	Ended(Option<i32>),
	/// Definite launch rejection with no surviving Harness.
	LaunchFailed,
	/// The supervising connection was lost.
	Disconnected,
	/// A validated Craft has reattached to the original helper.
	Reconnected,
	/// The previous execution is proven gone; later work requires a new Run.
	Lost,
}

pub(crate) enum SourceBoundary {
	Pending,
	Complete { offset: u64, checkpoint: String },
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
		self.store.write(async |tx| {
            let execution = tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
            let state: State = decode(&execution.state)?;
            if matches!(&boundary, SourceBoundary::Complete { offset, .. } if *offset <= state.source_offset) { return Err(invalid()); }
            let mut prefix = state.partial_source;
            for observation in observations {
                prefix.include(&observation)?;
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
            Ok(())
        }).await
	}
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
		Observation::Lost => {
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
		Observation::Reconnected => {
			if state.disconnected {
				state.disconnected = false;
				events.push(EventKind::RunActivityChanged {
					activity: state.activity,
				});
			}
		}
		Observation::Disconnected if lifecycle == RunLifecycle::Active => {
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
