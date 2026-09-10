//! Carrying out an admitted control request, and recording exactly how the
//! work ended (ADR-0083).
//!
//! Native cancellation is preferred and leaves the Run active. Everything
//! else escalates one step at a time through the helper's signal ladder,
//! waiting for the execution's own end between steps, so the Harness keeps
//! every chance to shut down cleanly and its partial output still reaches
//! Core through the ordinary source path.
use crate::{
	Core, CoreError, ExecutionSignal, RunControl, RunId, RunTermination,
	TerminationStage,
	effect::{Effect, EffectAdapter, EffectKind, EffectResult},
	run::execution_control::{ControlRequest, unmanaged},
};
use std::{sync::Arc, time::Duration};

/// How long one escalation step is given to end the execution before the
/// next, harder step is delivered.
const LADDER: [(ExecutionSignal, Duration); 3] = [
	(ExecutionSignal::Interrupt, Duration::from_secs(3)),
	(ExecutionSignal::Terminate, Duration::from_secs(3)),
	(ExecutionSignal::Kill, Duration::from_secs(4)),
];
/// How often the ladder re-reads the authoritative lifecycle while waiting.
const POLL: Duration = Duration::from_millis(25);

impl Core {
	/// Carries out every admitted Interrupt turn and Stop Run request.
	///
	/// # Errors
	/// Returns a store error if an Effect outcome cannot be recorded.
	pub async fn perform_run_controls(
		self: &Arc<Self>,
	) -> Result<(), CoreError> {
		self.reconcile_effects(
			&mut Controls(self),
			jet_store::EffectKindRecord::ControlRun,
		)
		.await?;
		Ok(())
	}

	async fn control_request(
		&self,
		run_id: RunId,
	) -> Result<(Option<ControlRequest>, bool), CoreError> {
		self.store
			.read(async |tx| {
				let run = tx.run(run_id.0).await?.ok_or_else(unmanaged)?;
				let record =
					tx.run_execution(run_id.0).await?.ok_or_else(unmanaged)?;
				let state: crate::run::state::State =
					crate::run::state::decode(&record.state)?;
				Ok((state.control, run.lifecycle.is_terminal()))
			})
			.await
	}

	/// Waits for the execution's own end, so a Harness that handles the
	/// signal is never escalated past.
	async fn ended_within(&self, run_id: RunId, wait: Duration) -> bool {
		let deadline = tokio::time::Instant::now() + wait;
		loop {
			match self.control_request(run_id).await {
				Ok((_, true)) => return true,
				Ok((_, false)) => {}
				Err(_) => return false,
			}
			if tokio::time::Instant::now() >= deadline {
				return false;
			}
			tokio::time::sleep(POLL).await;
		}
	}

	async fn settle_control(
		&self,
		run_id: RunId,
		termination: RunTermination,
	) -> Result<(), CoreError> {
		let now = self.now_unix_ms();
		self.store
			.write(async |tx| {
				let run = tx.run(run_id.0).await?.ok_or_else(unmanaged)?;
				let record =
					tx.run_execution(run_id.0).await?.ok_or_else(unmanaged)?;
				let mut state: crate::run::state::State =
					crate::run::state::decode(&record.state)?;
				if state.termination == Some(termination) {
					return Ok(());
				}
				state.control = None;
				state.termination = Some(termination);
				crate::run::state::save(tx, run_id, &state).await?;
				crate::run::execution_control::append_termination(
					tx,
					&run.into(),
					termination,
					now,
				)
				.await
			})
			.await
	}

	/// Drops a request the execution outlived on its own. Nothing stopped
	/// this Run, so nothing is recorded as having stopped it.
	async fn abandon_control(&self, run_id: RunId) -> Result<(), CoreError> {
		self.store
			.write(async |tx| {
				let record =
					tx.run_execution(run_id.0).await?.ok_or_else(unmanaged)?;
				let mut state: crate::run::state::State =
					crate::run::state::decode(&record.state)?;
				state.control = None;
				crate::run::state::save(tx, run_id, &state).await
			})
			.await
	}
}

struct Controls<'a>(&'a Arc<Core>);

impl Controls<'_> {
	async fn cancel_natively(
		&self,
		run_id: RunId,
		request: ControlRequest,
	) -> Option<EffectResult> {
		if request.control != RunControl::InterruptTurn {
			return None;
		}
		let correlation = request.correlation?;
		let connection = self.0.live_connection(run_id)?;
		if !connection.supports_native_cancellation() {
			return None;
		}
		// The Craft answers with its own turn boundary; the Run stays active
		// and the outcome is recorded when that boundary commits.
		Some(match connection.interrupt(correlation).await {
			Ok(()) => EffectResult::Completed,
			Err(_) => EffectResult::Unknown,
		})
	}

	async fn escalate(
		&self,
		run_id: RunId,
		control: RunControl,
	) -> EffectResult {
		let Some(host) = &self.0.run_host else {
			return EffectResult::Unknown;
		};
		let mut stage = TerminationStage::Unobserved;
		let mut delivered = None;
		for (signal, wait) in LADDER {
			if host
				.signal(self.0.run_home(), run_id, signal)
				.await
				.is_err()
			{
				// A helper that cannot be signalled may have gone because the
				// step before it worked. Only a Run that is actually over
				// settles here; nothing guesses that the work stopped.
				if !self.0.ended_within(run_id, Duration::ZERO).await {
					return EffectResult::Unknown;
				}
				stage = delivered.map_or(
					TerminationStage::Unobserved,
					ExecutionSignal::stage,
				);
				break;
			}
			delivered = Some(signal);
			if self.0.ended_within(run_id, wait).await {
				stage = signal.stage();
				break;
			}
		}
		match self
			.0
			.settle_control(run_id, RunTermination { control, stage })
			.await
		{
			Ok(()) => EffectResult::Completed,
			Err(_) => EffectResult::Unknown,
		}
	}
}

impl EffectAdapter for Controls<'_> {
	async fn execute(&mut self, effect: &Effect) -> EffectResult {
		let EffectKind::ControlRun { run_id } = effect.kind else {
			return EffectResult::Unknown;
		};
		let Ok((request, terminal)) = self.0.control_request(run_id).await
		else {
			return EffectResult::Unknown;
		};
		let Some(request) = request else {
			// Its outcome was already recorded, including by a Craft that
			// cancelled the turn natively.
			return EffectResult::Completed;
		};
		if terminal {
			return match self.0.abandon_control(run_id).await {
				Ok(()) => EffectResult::Completed,
				Err(_) => EffectResult::Unknown,
			};
		}
		if let Some(result) = self.cancel_natively(run_id, request).await {
			return result;
		}
		self.escalate(run_id, request.control).await
	}

	async fn reconcile(&mut self, effect: &Effect) -> EffectResult {
		// An interrupted ladder is observed, never continued blindly: a
		// signal already delivered may have ended work whose outcome is not
		// yet visible. Asking again is an explicit Command.
		let EffectKind::ControlRun { run_id } = effect.kind else {
			return EffectResult::Unknown;
		};
		match self.0.control_request(run_id).await {
			Ok((None, _)) => EffectResult::Completed,
			Ok((Some(_), true)) => match self.0.abandon_control(run_id).await {
				Ok(()) => EffectResult::Completed,
				Err(_) => EffectResult::Unknown,
			},
			Ok((Some(_), false)) | Err(_) => EffectResult::Unknown,
		}
	}
}
