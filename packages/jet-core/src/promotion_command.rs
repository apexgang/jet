//! Recording a Workspace promotion the user confirmed (ADR-0025,
//! ADR-0064).
//!
//! The promotion carries the binding its preview showed. Before the
//! transaction opens, the preview is computed again from the repository
//! as it is now and compared with what was carried: a Workspace or a
//! destination that has moved on makes the preview stale, and a stale
//! preview is refused rather than applied to a state the user never saw.
//! Inside the transaction the promotion is recorded with the Effect that
//! will apply it, or, when the preview could not settle every path, as
//! conflicted with those paths and no Effect at all: the destination is
//! never written over, and the Workspace keeps the conflict state for
//! the user to resolve.

use jet_store::{
	EffectKindRecord, EffectSafetyRecord, NewEffect, NewWorkspacePromotion,
	PromotionStateRecord, WriteTransaction,
};
use uuid::Uuid;

use crate::command::{CommandId, CommandOutcome};
use crate::error::CoreError;
use crate::event::{EventKind, EventSubject};
use crate::promotion::{
	self, Computed, PromotionBinding, PromotionConflict, PromotionState,
	WorkspacePromotion,
};
use crate::{Actor, Core};

/// Most attempts the Effect of a branch promotion may make. Its one
/// mutation is a compare-and-swap of the branch, which either happened
/// or did not, so a lost acknowledgement is safe to look at again.
const BRANCH_ATTEMPTS: u32 = 3;

/// A promotion whose binding was checked against the repository as it is
/// now, ready to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedPromotion {
	binding: PromotionBinding,
	changed_paths: u32,
	conflicts: Vec<PromotionConflict>,
}

/// Computes the preview again and refuses a binding it no longer matches,
/// before the transaction opens (ADR-0025, ADR-0093).
///
/// # Errors
///
/// Returns `workspace.promotion_unbound` when the preview was shown to
/// another client, `workspace.promotion_stale` when the Workspace or the
/// destination changed since, `workspace.promotion_empty` when there is
/// nothing to promote, and what the preview itself refuses.
pub(crate) async fn prepare(
	core: &Core,
	actor: &Actor,
	binding: &PromotionBinding,
) -> Result<PreparedPromotion, CoreError> {
	if binding.actor != actor.client_id() {
		return Err(CoreError::invalid_input(
			"workspace.promotion_unbound",
			"the preview was shown to another client; preview the promotion \
			 again",
		));
	}
	binding.destination.validate()?;
	let (workspace, project_root) = core
		.store
		.read(async |tx| {
			let Some(workspace) = tx.workspace(binding.workspace_id.0).await?
			else {
				return Err(promotion::workspace_not_found());
			};
			let Some(project) = tx.project(workspace.project_id).await? else {
				return Err(CoreError::not_found(
					"project.not_found",
					"the Project is not registered",
				));
			};
			Ok((workspace, std::path::PathBuf::from(project.root)))
		})
		.await?;
	let Computed {
		binding: current,
		changed_paths,
		conflicts,
		..
	} = promotion::compute(
		&core.workspace_home,
		actor,
		&workspace,
		&project_root,
		binding.destination.clone(),
	)
	.await?;
	if current != *binding {
		return Err(CoreError::conflict(
			"workspace.promotion_stale",
			"the Workspace or the destination changed since the preview; \
			 preview the promotion again",
		));
	}
	if changed_paths == 0 {
		return Err(CoreError::invalid_input(
			"workspace.promotion_empty",
			"the Workspace changes nothing in the destination",
		));
	}
	Ok(PreparedPromotion {
		binding: current,
		changed_paths,
		conflicts,
	})
}

/// Records a prepared promotion: applying, with the Effect that applies
/// it, or conflicted, with the paths that keep it from being applied.
///
/// # Errors
///
/// Returns `workspace.not_found` when the Workspace is gone,
/// `workspace.promotion_in_progress` while an earlier promotion of the
/// Workspace is still applying, and what the store reports when the rows
/// cannot be written.
pub(crate) async fn record(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	prepared: PreparedPromotion,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let PreparedPromotion {
		binding,
		changed_paths,
		conflicts,
	} = prepared;
	let Some(workspace) = tx.workspace(binding.workspace_id.0).await? else {
		return Err(promotion::workspace_not_found());
	};
	if let Some(latest) = tx.latest_promotion(workspace.workspace_id).await?
		&& latest.state == PromotionStateRecord::Applying
	{
		return Err(CoreError::conflict(
			"workspace.promotion_in_progress",
			"an earlier promotion of this Workspace is still being applied",
		));
	}
	let state = if conflicts.is_empty() {
		PromotionStateRecord::Applying
	} else {
		PromotionStateRecord::Conflicted
	};
	let promotion_id = Uuid::now_v7();
	let promotion: WorkspacePromotion = tx
		.insert_promotion(NewWorkspacePromotion {
			promotion_id,
			workspace_id: workspace.workspace_id,
			promoted_by: actor.record(),
			destination: (&binding.destination).into(),
			base_commit: binding.base_commit.clone(),
			workspace_tree: binding.workspace_tree.clone(),
			destination_commit: binding.destination_commit.clone(),
			destination_tree: binding.destination_tree.clone(),
			result_tree: binding.result_tree.clone(),
			changed_paths,
			state,
			conflicts: conflicts.iter().map(Into::into).collect(),
			recorded_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	let event = EventKind::WorkspacePromotionRecorded {
		workspace_id: promotion.binding.workspace_id,
		promotion_id: promotion.promotion_id,
		binding: promotion.binding.clone(),
		state: promotion.state,
		conflicts: promotion.conflicts.clone(),
	};
	tx.append_event(event.to_record(
		actor,
		EventSubject::Conversation(crate::ConversationId(
			workspace.conversation_id,
		)),
		now_unix_ms,
	)?)
	.await?;
	if promotion.state == PromotionState::Applying {
		// ASVS 2.3.3: the Effect commits with the promotion it applies, so
		// an acknowledged promotion cannot be lost before its work begins
		// (ADR-0064). Writing a checkout cannot be repeated safely after a
		// lost acknowledgement; moving a branch is one compare-and-swap
		// that can be looked at again (ADR-0067).
		let effect_id = Uuid::now_v7();
		let safety = match binding.destination {
			promotion::PromotionDestination::LocalCheckout => {
				EffectSafetyRecord::Ambiguous
			}
			promotion::PromotionDestination::Branch(_) => {
				EffectSafetyRecord::Idempotent {
					external_key: promotion_id,
					max_attempts: BRANCH_ATTEMPTS,
				}
			}
		};
		tx.insert_effect(&NewEffect {
			effect_id,
			command_id: command_id.0,
			run_id: None,
			promotion_id: Some(promotion_id),
			kind: EffectKindRecord::PromoteWorkspace,
			safety,
		})
		.await?;
	}
	Ok(CommandOutcome::WorkspacePromotionRecorded(promotion))
}

#[cfg(test)]
#[path = "promotion_command_tests.rs"]
mod tests;
