//! Applies ordered Harness observations to the Run projection.

use super::{Observation, SourcePrefix, State, invalid};
use crate::{
	CoreError, EventKind, ManagedProcess, ManagedProcessRole, RunActivity,
	RunLifecycle,
};

/// Whether the execution is still running. A Run being stopped keeps
/// producing output until its native process actually ends, and none of it
/// is discarded because a control request was admitted (ADR-0083).
pub(super) fn executing(lifecycle: RunLifecycle) -> bool {
	matches!(lifecycle, RunLifecycle::Active | RunLifecycle::Stopping)
}

pub(super) fn apply(
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
			state.quota_wait = reason == RunActivity::WaitingForQuota;
			activity(state, Some(reason), &mut events)
		}
		// Recording the request grants nothing: the Craft is still holding
		// it, and only a decision Core sends back releases it (ADR-0012).
		Observation::ApprovalRequested(request) if executing(lifecycle) => {
			request.validate()?;
			events.push(EventKind::ApprovalRequested { request });
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
		| Observation::Model(_)
		| Observation::Usage(_)
		| Observation::NativeConversation(_)
		| Observation::ProcessTitle { .. }
		| Observation::ConversationTitle(_)
		| Observation::RunTitle(_)
		| Observation::ApprovalRequested(_)
		| Observation::Ended(_)
		| Observation::LaunchFailed
		| Observation::Disconnected => return Err(invalid()),
	}
	Ok((next, events))
}

pub(super) fn activity(
	state: &mut State,
	activity: Option<RunActivity>,
	events: &mut Vec<EventKind>,
) {
	if state.activity != activity {
		state.activity = activity;
		events.push(EventKind::RunActivityChanged { activity });
	}
}
