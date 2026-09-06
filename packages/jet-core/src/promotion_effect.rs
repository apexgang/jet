//! The Effect that applies a recorded Workspace promotion (ADR-0025,
//! ADR-0064, ADR-0067).
//!
//! The Adapter reads the promotion its Effect names, checks the
//! destination against what the preview bound right before writing, and
//! applies the bound result with the Git operations in
//! [`crate::promotion_apply`]. A destination that moved is a definite
//! failure that changes nothing. An attempt interrupted after writing
//! began is looked at again: a checkout that holds the result is done, a
//! branch still at its previewed tip is safe to try once more, and
//! anything else is an outcome Jet does not know and does not guess.

use std::path::PathBuf;

use jet_store::{
	EffectKindRecord, EffectStateRecord, PromotionStateRecord, Store,
	WorkspacePromotionRecord, WriteTransaction,
};

use crate::effect::{Effect, EffectAdapter, EffectKind, EffectResult};
use crate::error::CoreError;
use crate::event::{EventKind, EventSubject};
use crate::promotion::{PromotionId, WorkspacePromotion};
use crate::promotion_apply::{self, Observed};
use crate::workspace::{self, WorkspaceHome};
use crate::{Actor, ConversationId, Core};

/// Performs promotions through the Plane's Git, with the Plane store to
/// read what each Effect names.
struct GitPromoter<'a> {
	store: &'a Store,
	home: &'a WorkspaceHome,
}

/// What an Effect names, read from the store.
struct Named {
	promotion: WorkspacePromotionRecord,
	project_root: PathBuf,
}

impl Core {
	/// Performs every pending Workspace promotion and reconciles every
	/// interrupted one, recording each outcome durably (ADR-0064,
	/// ADR-0067). `jetd` calls this once it has started, so an attempt a
	/// previous daemon did not finish is settled before new work, and
	/// after every Command, so a promotion is applied as soon as it is
	/// acknowledged. Run starts are performed elsewhere.
	///
	/// # Errors
	///
	/// Returns what the store reports when an outcome cannot be recorded.
	/// The work itself never fails this call: what Git could not do is
	/// recorded on the promotion.
	pub async fn perform_promotions(&self) -> Result<(), CoreError> {
		let mut adapter = GitPromoter {
			store: &self.store,
			home: &self.workspace_home,
		};
		self.reconcile_effects(
			&mut adapter,
			EffectKindRecord::PromoteWorkspace,
		)
		.await?;
		Ok(())
	}
}

impl EffectAdapter for GitPromoter<'_> {
	async fn execute(&mut self, effect: &Effect) -> EffectResult {
		let EffectKind::PromoteWorkspace { promotion_id } = effect.kind else {
			return EffectResult::Unknown;
		};
		let Some(named) = self.named(promotion_id).await else {
			return EffectResult::Unknown;
		};
		let Named {
			promotion,
			project_root,
		} = &named;
		let applied =
			workspace::with_scratch(self.home, "apply", async |scratch| {
				Ok(promotion_apply::apply(project_root, scratch, promotion)
					.await)
			})
			.await;
		match applied {
			Ok(result) => result,
			Err(_) => EffectResult::Unknown,
		}
	}

	async fn reconcile(&mut self, effect: &Effect) -> EffectResult {
		let EffectKind::PromoteWorkspace { promotion_id } = effect.kind else {
			return EffectResult::Unknown;
		};
		let Some(named) = self.named(promotion_id).await else {
			return EffectResult::Unknown;
		};
		let Named {
			promotion,
			project_root,
		} = &named;
		let observed =
			workspace::with_scratch(self.home, "observe", async |scratch| {
				promotion_apply::observe(project_root, scratch, promotion).await
			})
			.await;
		match observed {
			Ok(Observed::Applied) => EffectResult::Completed,
			// A checkout still exactly as previewed was never written: the
			// attempt failed before it began, and the user may promote
			// again. A branch still at its tip is looked at again by the
			// retry its safety allows (ADR-0067).
			Ok(Observed::Untouched) => match promotion.destination {
				jet_store::PromotionDestinationRecord::LocalCheckout => {
					EffectResult::Failed
				}
				jet_store::PromotionDestinationRecord::Branch(_) => {
					EffectResult::Unknown
				}
			},
			Ok(Observed::Elsewhere) | Err(_) => EffectResult::Unknown,
		}
	}
}

impl GitPromoter<'_> {
	/// Reads what the Effect names. A promotion, Workspace, or Project the
	/// store no longer has is not something this Adapter can act on.
	async fn named(&self, promotion_id: PromotionId) -> Option<Named> {
		self.store
			.read(async |tx| {
				let Some(promotion) = tx.promotion(promotion_id.0).await?
				else {
					return Ok(None);
				};
				let Some(workspace) =
					tx.workspace(promotion.workspace_id).await?
				else {
					return Ok(None);
				};
				let Some(project) = tx.project(workspace.project_id).await?
				else {
					return Ok(None);
				};
				Ok::<_, CoreError>(Some(Named {
					promotion,
					project_root: PathBuf::from(project.root),
				}))
			})
			.await
			.ok()
			.flatten()
	}
}

/// Settles the promotion an Effect applied, in the transaction that
/// finishes the Effect, and journals where it now stands.
pub(crate) async fn settle(
	tx: &mut WriteTransaction,
	promotion_id: PromotionId,
	state: EffectStateRecord,
	now_unix_ms: i64,
) -> Result<(), CoreError> {
	let settled = match state {
		EffectStateRecord::Completed => PromotionStateRecord::Promoted,
		EffectStateRecord::Failed => PromotionStateRecord::Failed,
		EffectStateRecord::OutcomeUnknown => {
			PromotionStateRecord::OutcomeUnknown
		}
		EffectStateRecord::Pending | EffectStateRecord::InFlight => {
			return Err(CoreError::internal(
				"workspace.promotion_unsettled",
				"a promotion Effect finished in a state that settles nothing",
			));
		}
	};
	let record = tx
		.settle_promotion(promotion_id.0, settled, now_unix_ms)
		.await?;
	let Some(workspace) = tx.workspace(record.workspace_id).await? else {
		return Err(CoreError::internal(
			"workspace.promotion_orphaned",
			"a settled promotion names a Workspace the store does not have",
		));
	};
	let actor = Actor::from_record(record.promoted_by);
	let promotion = WorkspacePromotion::from(record);
	let event = EventKind::WorkspacePromotionSettled {
		workspace_id: promotion.binding.workspace_id,
		promotion_id: promotion.promotion_id,
		state: promotion.state,
	};
	tx.append_event(event.to_record(
		&actor,
		EventSubject::Conversation(ConversationId(workspace.conversation_id)),
		now_unix_ms,
	)?)
	.await?;
	Ok(())
}

#[cfg(test)]
#[path = "promotion_effect_tests.rs"]
mod tests;
