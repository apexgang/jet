//! Durable Effects and restart reconciliation (ADR-0064, ADR-0067).

use jet_store::{
	EffectKindRecord, EffectRecord, EffectSafetyRecord, EffectStateRecord,
	WriteTransaction,
};
use uuid::Uuid;

use crate::{CommandId, Core, CoreError, PromotionId, RunId, promotion_effect};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffectKind {
	StartRun {
		run_id: RunId,
	},
	/// Apply one recorded Workspace promotion to its destination
	/// (ADR-0025).
	PromoteWorkspace {
		promotion_id: PromotionId,
	},
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
	fn execute(
		&mut self,
		effect: &Effect,
	) -> impl std::future::Future<Output = EffectResult> + Send;

	/// Observes an interrupted attempt without changing external state.
	fn reconcile(
		&mut self,
		effect: &Effect,
	) -> impl std::future::Future<Output = EffectResult> + Send;
}

impl Core {
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "the guard must span the Adapter call between the two store \
		          transactions; releasing it earlier would let a second \
		          worker claim the same in-flight Effect (ADR-0067)"
	)]
	/// Performs every pending Effect of one `kind` and reconciles every
	/// interrupted one through `adapter`, so a worker that performs one
	/// kind of work leaves the others to theirs.
	pub(crate) async fn reconcile_effects(
		&self,
		adapter: &mut impl EffectAdapter,
		kind: EffectKindRecord,
	) -> Result<Vec<Effect>, CoreError> {
		// ASVS 15.4.1: one core serializes all Effect decisions, so two
		// workers cannot execute the same durable request concurrently. The
		// guard spans store awaits, so it is the async mutex.
		let _guard = self.effect_reconciliation.lock().await;
		let records = self
			.store
			.read(async |tx| tx.unresolved_effects_of(kind).await)
			.await?;
		let mut effects = Vec::with_capacity(records.len());
		for record in records {
			effects.push(self.reconcile_effect(adapter, record).await?);
		}
		Ok(effects)
	}

	async fn reconcile_effect(
		&self,
		adapter: &mut impl EffectAdapter,
		record: EffectRecord,
	) -> Result<Effect, CoreError> {
		let effect = Effect::try_from(record)?;
		match effect.state {
			EffectState::Pending => self.execute_effect(adapter, effect).await,
			EffectState::InFlight => match adapter.reconcile(&effect).await {
				EffectResult::Completed => {
					self.finish_effect(effect, EffectStateRecord::Completed)
						.await
				}
				EffectResult::Failed => {
					self.finish_effect(effect, EffectStateRecord::Failed).await
				}
				EffectResult::Unknown
					if may_retry(effect.safety, effect.attempt_count) =>
				{
					self.execute_effect(adapter, effect).await
				}
				EffectResult::Unknown => {
					self.finish_effect(
						effect,
						EffectStateRecord::OutcomeUnknown,
					)
					.await
				}
			},
			EffectState::Completed
			| EffectState::Failed
			| EffectState::OutcomeUnknown => Ok(effect),
		}
	}

	async fn execute_effect(
		&self,
		adapter: &mut impl EffectAdapter,
		effect: Effect,
	) -> Result<Effect, CoreError> {
		let record = self
			.store
			.write(async |tx| tx.begin_effect_attempt(effect.effect_id).await)
			.await?;
		let in_flight = Effect::try_from(record)?;
		match adapter.execute(&in_flight).await {
			EffectResult::Completed => {
				self.finish_effect(in_flight, EffectStateRecord::Completed)
					.await
			}
			EffectResult::Failed => {
				self.finish_effect(in_flight, EffectStateRecord::Failed)
					.await
			}
			EffectResult::Unknown => Ok(in_flight),
		}
	}

	/// Records an Effect's terminal state together with what it settles:
	/// the outcome of the work is durable in the same transaction as the
	/// Effect that did it (ADR-0064).
	async fn finish_effect(
		&self,
		effect: Effect,
		state: EffectStateRecord,
	) -> Result<Effect, CoreError> {
		let now_unix_ms = self.now_unix_ms();
		let record = self
			.store
			.write(async |tx| {
				let record = tx.finish_effect(effect.effect_id, state).await?;
				settle(tx, &effect, state, now_unix_ms).await?;
				Ok::<_, CoreError>(record)
			})
			.await?;
		Effect::try_from(record)
	}
}

/// Settles what the Effect was for, in the transaction that finishes it.
async fn settle(
	tx: &mut WriteTransaction,
	effect: &Effect,
	state: EffectStateRecord,
	now_unix_ms: i64,
) -> Result<(), CoreError> {
	match effect.kind {
		EffectKind::StartRun { .. } => Ok(()),
		EffectKind::PromoteWorkspace { promotion_id } => {
			promotion_effect::settle(tx, promotion_id, state, now_unix_ms).await
		}
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
			EffectKindRecord::PromoteWorkspace => {
				EffectKind::PromoteWorkspace {
					promotion_id: PromotionId(record.promotion_id.ok_or_else(
						|| {
							CoreError::internal(
								"effect.invalid",
								"a workspace.promote Effect has no promotion identity",
							)
						},
					)?),
				}
			}
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
