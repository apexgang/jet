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

/// How far Jet had to go before the execution actually stopped. Everything
/// past `Interrupt` is a forced termination: the Harness was given the
/// chance to end its own work and did not take it.
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
	/// The turn that was executing when the request was admitted, which is
	/// also the identity the pinned Craft was given for it. Naming it here
	/// keeps a cancellation from ever reaching work admitted afterwards.
	pub(crate) correlation: Option<uuid::Uuid>,
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
	// Nothing to control before the native process exists. Cancelling a
	// launch is a different, ambiguous decision, and an execution that
	// never gets past starting is recovered, not signalled (ADR-0067).
	if matches!(
		run.lifecycle,
		jet_store::RunLifecycle::Created | jet_store::RunLifecycle::Starting
	) {
		return Err(CoreError::conflict(
			"run.not_started",
			"the Run has no live execution to control yet",
		));
	}
	let conversation_id = crate::ConversationId(run.conversation_id);
	let queue = crate::turn_queue::load(tx, conversation_id).await?;
	let active = queue.active_turn(run_id);
	if control == RunControl::InterruptTurn
		&& (run.lifecycle != jet_store::RunLifecycle::Active
			|| active.is_none())
	{
		return Err(CoreError::conflict(
			"run.no_active_turn",
			"the Run is not executing a turn to interrupt",
		));
	}
	let mut state: crate::run_state::State =
		crate::run_state::decode(&record.state)?;
	if state.control.map(|request| request.control) != Some(control) {
		state.control = Some(ControlRequest {
			control,
			correlation: active,
		});
		state.termination = None;
		crate::run_state::save(tx, run_id, &state).await?;
	}
	append_control(tx, actor, &run.into(), control, now).await?;
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

/// Records an admitted control request, moving a stop into `stopping` so no
/// later turn is claimed while the execution is being ended.
async fn append_control(
	tx: &mut jet_store::WriteTransaction,
	actor: &crate::Actor,
	run: &Run,
	control: RunControl,
	now: i64,
) -> Result<(), CoreError> {
	crate::run_state::append(
		tx,
		&crate::EventActor::from(actor.clone()),
		run,
		crate::EventKind::RunControlRequested { control },
		now,
	)
	.await?;
	if control == RunControl::StopRun
		&& run.lifecycle == jet_store::RunLifecycle::Active
	{
		tx.update_run_lifecycle(
			run.run_id.0,
			jet_store::RunLifecycle::Stopping,
			now,
		)
		.await?;
		crate::run_state::append(
			tx,
			&crate::EventActor::from(actor.clone()),
			run,
			crate::EventKind::RunLifecycleChanged {
				from: jet_store::RunLifecycle::Active,
				to: jet_store::RunLifecycle::Stopping,
			},
			now,
		)
		.await?;
	}
	Ok(())
}

/// Records how a control request actually ended the work it targeted.
pub(crate) async fn append_termination(
	tx: &mut jet_store::WriteTransaction,
	run: &Run,
	termination: RunTermination,
	now: i64,
) -> Result<(), CoreError> {
	let record = tx
		.run_execution(run.run_id.0)
		.await?
		.ok_or_else(unmanaged)?;
	let plan: crate::LaunchPlan = crate::run_state::decode(&record.plan)?;
	crate::run_state::append(
		tx,
		&crate::EventActor::RunSupervisor {
			run_id: run.run_id,
			authorized_by: plan.client_id,
		},
		run,
		crate::EventKind::RunTerminated { termination },
		now,
	)
	.await
}

pub(crate) fn unmanaged() -> CoreError {
	CoreError::not_found(
		"run.execution_not_found",
		"the managed Run does not exist",
	)
}
