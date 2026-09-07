//! Interrupt turn and Stop Run: two distinct durable requests against one
//! managed execution (ADR-0083).
//!
//! A request is admitted in the Command's transaction and carried out by a
//! separate Effect, so a client that disappears after the acknowledgement
//! never leaves the execution half-controlled (ADR-0095). Interruption
//! prefers the Harness's own cancellation and leaves the Run able to take
//! the next turn; stopping, and interruption a Craft cannot perform
//! natively, escalate through the helper's signal ladder and record which
//! step actually ended the work.
use crate::{Actor, CommandId, CommandOutcome, CoreError, Run, RunId};
use serde::{Deserialize, Serialize};

/// What an interactive control request asks of one managed execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunControl {
	/// End the current turn and leave the Run able to accept the next one.
	InterruptTurn,
	/// End the whole execution, including its native processes.
	StopRun,
}

/// How far Jet had to go before the execution actually stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationStage {
	/// The Harness cancelled its own turn; no signal was sent.
	NativeCancellation,
	/// The native process ended after an interrupt signal.
	Interrupt,
	/// It ignored the interrupt and ended after a terminate signal.
	Terminate,
	/// It ignored both and was killed.
	Kill,
	/// Every signal was delivered and the end was never observed.
	Unobserved,
}

/// The exact terminal outcome of one control request, recorded once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTermination {
	/// The request this outcome answers.
	pub control: RunControl,
	/// The step that actually ended the work.
	pub stage: TerminationStage,
}

/// One step of the escalation ladder Core asks the execution host to
/// deliver. Core never escalates without an admitted request, and the host
/// never escalates on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionSignal {
	/// Interrupt, which a Harness may handle and shut down cleanly.
	Interrupt,
	/// Terminate, which it may still handle.
	Terminate,
	/// Kill, which it cannot.
	Kill,
}

impl ExecutionSignal {
	/// The outcome recorded when this step ends the execution.
	pub(crate) fn stage(self) -> TerminationStage {
		match self {
			Self::Interrupt => TerminationStage::Interrupt,
			Self::Terminate => TerminationStage::Terminate,
			Self::Kill => TerminationStage::Kill,
		}
	}
}

/// One admitted request, kept with the execution it controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ControlRequest {
	pub(crate) control: RunControl,
	/// The turn that was active when the request was admitted. An
	/// interruption names the turn it may cancel, so a Craft can never
	/// apply it to work admitted afterwards.
	pub(crate) turn_id: Option<uuid::Uuid>,
}

/// Records an admitted control request against a live managed Run.
///
/// Stopping is accepted from any live lifecycle. Interruption requires an
/// active Run, since there is no turn to cancel before one exists, which
/// also refuses an interruption of a Run that is already stopping.
///
/// A repeated request keeps the turn the first one named and drives the
/// ladder again; asking to stop a Run that was only being interrupted
/// replaces the weaker request. Neither one escalates by itself: how often
/// a client asks never chooses how hard Jet stops the work.
pub(crate) async fn record(
	tx: &mut jet_store::WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	run_id: RunId,
	control: RunControl,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let run = tx.run(run_id.0).await?.ok_or_else(unmanaged)?;
	let record = tx.run_execution(run_id.0).await?.ok_or_else(unmanaged)?;
	if run.lifecycle.is_terminal() {
		return Err(CoreError::conflict(
			"run.not_controllable",
			"the Run has already ended",
		));
	}
	if control == RunControl::InterruptTurn
		&& run.lifecycle != jet_store::RunLifecycle::Active
	{
		return Err(CoreError::conflict(
			"run.no_active_turn",
			"the Run is not executing a turn to interrupt",
		));
	}
	let mut state: crate::run_state::State =
		crate::run_state::decode(&record.state)?;
	if state.control.map(|request| request.control) != Some(control) {
		let conversation_id = crate::ConversationId(run.conversation_id);
		let queue = crate::turn_queue::load(tx, conversation_id).await?;
		state.control = Some(ControlRequest {
			control,
			turn_id: queue.active_turn(run_id),
		});
		state.termination = None;
		crate::run_state::save(tx, run_id, &state).await?;
	}
	crate::run_state::append_control(tx, actor, &run.into(), control, now)
		.await?;
	tx.insert_effect(&jet_store::NewEffect {
		effect_id: uuid::Uuid::now_v7(),
		command_id: command_id.0,
		run_id: Some(run_id.0),
		promotion_id: None,
		terminal_id: None,
		kind: jet_store::EffectKindRecord::ControlRun,
		safety: jet_store::EffectSafetyRecord::Ambiguous,
	})
	.await?;
	let run = tx.run(run_id.0).await?.ok_or_else(unmanaged)?;
	Ok(CommandOutcome::RunControlAccepted {
		run: Run::from(run),
		control,
	})
}

pub(crate) fn unmanaged() -> CoreError {
	CoreError::not_found(
		"run.execution_not_found",
		"the managed Run does not exist",
	)
}
