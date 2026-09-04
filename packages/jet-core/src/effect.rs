//! Durable Effects and restart reconciliation (ADR-0064, ADR-0067).

use jet_store::{
	EffectKindRecord, EffectRecord, EffectSafetyRecord, EffectStateRecord,
};
use uuid::Uuid;

use crate::{CommandId, Core, CoreError, RunId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffectKind {
	StartRun { run_id: RunId },
}

pub(crate) type EffectSafety = EffectSafetyRecord;
pub(crate) type EffectState = EffectStateRecord;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Effect {
	pub(crate) effect_id: Uuid,
	pub(crate) command_id: CommandId,
	pub(crate) kind: EffectKind,
	pub(crate) safety: EffectSafety,
	pub(crate) state: EffectState,
	pub(crate) attempt_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectResult {
	Completed,
	Failed,
	Unknown,
}

/// Adapter at the seam between durable orchestration and external work.
///
/// Implementations must use the Effect's recorded stable external key whenever
/// the target supports idempotency. They must return
/// [`EffectResult::Unknown`] rather than guessing when an acknowledgement is
/// lost.
pub(crate) trait EffectAdapter {
	/// Performs a new or provably safe repeated attempt.
	fn execute(&mut self, effect: &Effect) -> EffectResult;

	/// Observes an interrupted attempt without changing external state.
	fn reconcile(&mut self, effect: &Effect) -> EffectResult;
}

impl Core {
	pub(crate) fn reconcile_effects(
		&self,
		adapter: &mut impl EffectAdapter,
	) -> Result<Vec<Effect>, CoreError> {
		// ASVS 15.4.1: one core serializes all Effect decisions, so two
		// workers cannot execute the same durable request concurrently.
		let _guard = self
			.effect_reconciliation
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner);
		let records = self.store.read(|tx| tx.unresolved_effects())?;
		records
			.into_iter()
			.map(|record| self.reconcile_effect(adapter, record))
			.collect()
	}

	fn reconcile_effect(
		&self,
		adapter: &mut impl EffectAdapter,
		record: EffectRecord,
	) -> Result<Effect, CoreError> {
		let effect = Effect::try_from(record)?;
		match effect.state {
			EffectState::Pending => self.execute_effect(adapter, effect),
			EffectState::InFlight => match adapter.reconcile(&effect) {
				EffectResult::Completed => {
					self.finish_effect(effect, EffectStateRecord::Completed)
				}
				EffectResult::Failed => {
					self.finish_effect(effect, EffectStateRecord::Failed)
				}
				EffectResult::Unknown
					if may_retry(effect.safety, effect.attempt_count) =>
				{
					self.execute_effect(adapter, effect)
				}
				EffectResult::Unknown => self
					.finish_effect(effect, EffectStateRecord::OutcomeUnknown),
			},
			EffectState::Completed
			| EffectState::Failed
			| EffectState::OutcomeUnknown => Ok(effect),
		}
	}

	fn execute_effect(
		&self,
		adapter: &mut impl EffectAdapter,
		effect: Effect,
	) -> Result<Effect, CoreError> {
		let record = self
			.store
			.write(|tx| tx.begin_effect_attempt(effect.effect_id))?;
		let in_flight = Effect::try_from(record)?;
		match adapter.execute(&in_flight) {
			EffectResult::Completed => {
				self.finish_effect(in_flight, EffectStateRecord::Completed)
			}
			EffectResult::Failed => {
				self.finish_effect(in_flight, EffectStateRecord::Failed)
			}
			EffectResult::Unknown => Ok(in_flight),
		}
	}

	fn finish_effect(
		&self,
		effect: Effect,
		state: EffectStateRecord,
	) -> Result<Effect, CoreError> {
		let record = self
			.store
			.write(|tx| tx.finish_effect(effect.effect_id, state))?;
		Effect::try_from(record)
	}
}

impl TryFrom<EffectRecord> for Effect {
	type Error = CoreError;

	fn try_from(record: EffectRecord) -> Result<Self, CoreError> {
		let kind = match record.kind {
			EffectKindRecord::StartRun => EffectKind::StartRun {
				run_id: RunId(record.run_id.ok_or_else(|| {
					CoreError::internal(
						"effect.invalid",
						"a run.start Effect has no Run identity",
					)
				})?),
			},
		};
		Ok(Self {
			effect_id: record.effect_id,
			command_id: CommandId(record.command_id),
			kind,
			safety: record.safety,
			state: record.state,
			attempt_count: record.attempt_count,
		})
	}
}

fn may_retry(safety: EffectSafety, attempt_count: u32) -> bool {
	// ASVS 13.2.6: retry eligibility and its bound are explicit policy.
	match safety {
		EffectSafety::ReadOnly { max_attempts }
		| EffectSafety::Idempotent { max_attempts, .. } => {
			attempt_count < max_attempts
		}
		EffectSafety::Ambiguous => false,
	}
}
