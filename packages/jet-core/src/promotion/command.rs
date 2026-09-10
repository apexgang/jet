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

use crate::{
	Actor, Core,
	command::{CommandId, CommandOutcome},
	error::CoreError,
	event::{EventKind, EventSubject},
	promotion::{
		self, Compared, PromotionBinding, PromotionDestination, PromotionState,
		WorkspacePromotion,
	},
};
use jet_store::{
	EffectKindRecord, EffectSafetyRecord, NewEffect, NewWorkspacePromotion,
	PromotionStateRecord, WriteTransaction,
};
use uuid::Uuid;

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
			promotion::workspace_and_project(tx, binding.workspace_id).await
		})
		.await?;
	core.check_disk_at(project_root.clone(), 0).await?;
	let Compared {
		binding: current,
		changed_paths,
		..
	} = promotion::compare(
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
	let state = if binding.conflicts.is_empty() {
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
			destination_dirty: binding.destination_dirty,
			changed_paths,
			state,
			conflicts: binding.conflicts.iter().map(Into::into).collect(),
			recorded_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	let event = EventKind::WorkspacePromotionRecorded {
		workspace_id: promotion.binding.workspace_id,
		promotion_id: promotion.promotion_id,
		binding: promotion.binding.clone(),
		state: promotion.state,
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
			PromotionDestination::LocalCheckout => {
				EffectSafetyRecord::Ambiguous
			}
			PromotionDestination::Branch(_) => EffectSafetyRecord::Idempotent {
				external_key: promotion_id,
				max_attempts: BRANCH_ATTEMPTS,
			},
		};
		tx.insert_effect(&NewEffect {
			effect_id,
			command_id: command_id.0,
			run_id: None,
			promotion_id: Some(promotion_id),
			terminal_id: None,
			kind: EffectKindRecord::PromoteWorkspace,
			safety,
		})
		.await?;
	}
	Ok(CommandOutcome::WorkspacePromotionRecorded(promotion))
}

#[cfg(test)]
mod tests {
	use jet_store::{EffectKindRecord, EffectStateRecord};
	use pretty_assertions::assert_eq;

	use crate::test_support::{
		Diverged, actor, conversation_snapshot as snapshot, diverged, events,
		preview_promotion, request, status,
	};
	use crate::{
		Actor, ClientId, Command, CommandOutcome, ConflictKind, Core,
		CoreError, ErrorCategory, EventKind, PromotionBinding,
		PromotionConflict, PromotionDestination, PromotionPreview,
		PromotionState, Workspace, WorkspacePromotion,
	};

	async fn preview(
		core: &Core,
		workspace: &Workspace,
		destination: PromotionDestination,
	) -> PromotionPreview {
		preview_promotion(core, workspace.workspace_id, destination)
			.await
			.unwrap()
	}

	async fn promote(
		core: &Core,
		actor: &Actor,
		binding: PromotionBinding,
	) -> Result<WorkspacePromotion, CoreError> {
		let outcome = core
			.execute(actor, request(Command::PromoteWorkspace { binding }))
			.await?;
		let CommandOutcome::WorkspacePromotionRecorded(promotion) = outcome
		else {
			panic!("expected CommandOutcome::WorkspacePromotionRecorded");
		};
		Ok(promotion)
	}

	/// The unresolved promotion Effects, as their promotion identities and
	/// states.
	async fn pending_effects(
		core: &Core,
	) -> Vec<(uuid::Uuid, EffectStateRecord)> {
		core.store
			.read(async |tx| {
				Ok::<_, CoreError>(
					tx.unresolved_effects_of(
						EffectKindRecord::PromoteWorkspace,
					)
					.await?
					.into_iter()
					.map(|effect| (effect.promotion_id.unwrap(), effect.state))
					.collect(),
				)
			})
			.await
			.unwrap()
	}

	/// A promotion confirmed from a clean preview is recorded as applying,
	/// with the Effect that applies it committed beside it and journaled,
	/// and the Workspace's snapshot shows where it stands. Nothing is
	/// written before the Effect runs (ADR-0025, ADR-0064).
	#[tokio::test]
	async fn a_clean_promotion_is_recorded_with_its_effect() {
		let dir = tempfile::tempdir().unwrap();
		let Diverged {
			core,
			repository,
			workspace,
			..
		} = diverged(dir.path()).await;
		let previewed =
			preview(&core, &workspace, PromotionDestination::LocalCheckout)
				.await;

		let promotion = promote(&core, &actor(), previewed.binding.clone())
			.await
			.unwrap();
		let shown = snapshot(&core, workspace.conversation_id)
			.await
			.workspace
			.unwrap()
			.promotion;
		let journal = events(&core).await;

		assert_eq!(
			(
				&promotion,
				shown.as_ref(),
				journal.last(),
				pending_effects(&core).await,
				status(&repository),
			),
			(
				&WorkspacePromotion {
					promotion_id: promotion.promotion_id,
					binding: previewed.binding.clone(),
					changed_paths: 3,
					state: PromotionState::Applying,
					recorded_at: promotion.recorded_at,
					settled_at: None,
				},
				Some(&promotion),
				Some(&EventKind::WorkspacePromotionRecorded {
					workspace_id: workspace.workspace_id,
					promotion_id: promotion.promotion_id,
					binding: previewed.binding,
					state: PromotionState::Applying,
				}),
				vec![(promotion.promotion_id.0, EffectStateRecord::Pending)],
				" M f.txt\nA  o.txt\n?? notes.txt\n".into(),
			)
		);
	}

	/// A promotion confirmed from a conflicted preview writes nothing to the
	/// destination and needs no Effect: it is recorded conflicted with the
	/// paths it could not settle, which the Workspace keeps for the user to
	/// resolve (ADR-0025).
	#[tokio::test]
	async fn a_conflicted_promotion_keeps_its_conflicts_and_writes_nothing() {
		let dir = tempfile::tempdir().unwrap();
		let Diverged {
			core,
			repository,
			workspace,
			..
		} = diverged(dir.path()).await;
		std::fs::write(repository.join("f.txt"), "X\nb\nC\n").unwrap();
		let previewed =
			preview(&core, &workspace, PromotionDestination::LocalCheckout)
				.await;

		let promotion = promote(&core, &actor(), previewed.binding.clone())
			.await
			.unwrap();
		let shown = snapshot(&core, workspace.conversation_id)
			.await
			.workspace
			.unwrap()
			.promotion;

		assert_eq!(
			(
				&promotion,
				shown.as_ref(),
				pending_effects(&core).await,
				std::fs::read_to_string(repository.join("f.txt")).unwrap(),
				status(&repository),
			),
			(
				&WorkspacePromotion {
					promotion_id: promotion.promotion_id,
					binding: PromotionBinding {
						conflicts: vec![PromotionConflict {
							path: "f.txt".into(),
							kind: ConflictKind::Diverged,
						}],
						..previewed.binding
					},
					changed_paths: 3,
					state: PromotionState::Conflicted,
					recorded_at: promotion.recorded_at,
					settled_at: Some(promotion.recorded_at),
				},
				Some(&promotion),
				vec![],
				"X\nb\nC\n".into(),
				" M f.txt\nA  o.txt\n?? notes.txt\n".into(),
			)
		);
	}

	/// A binding is refused when the preview was shown to another client,
	/// when the destination or the Workspace moved on since it was shown,
	/// when the risk it showed is no longer the risk, when it would change
	/// nothing, and while an earlier promotion of the Workspace is still
	/// applying; none of them records anything (ADR-0025).
	#[tokio::test]
	async fn a_binding_the_world_moved_past_is_refused_without_a_trace() {
		let dir = tempfile::tempdir().unwrap();
		let Diverged {
			core,
			repository,
			workspace,
			..
		} = diverged(dir.path()).await;
		let other = Actor::InteractiveClient {
			client_id: ClientId(uuid::Uuid::from_u128(7)),
		};
		let previewed =
			preview(&core, &workspace, PromotionDestination::LocalCheckout)
				.await;
		let unbound = promote(&core, &other, previewed.binding.clone())
			.await
			.unwrap_err();
		// An ignored file appearing where the Workspace adds a path changes
		// no tree, only the risk the preview showed.
		std::fs::write(repository.join(".git/info/exclude"), "new.txt\n")
			.unwrap();
		std::fs::write(repository.join("new.txt"), "ignored\n").unwrap();
		let risk_moved = promote(&core, &actor(), previewed.binding.clone())
			.await
			.unwrap_err();
		std::fs::remove_file(repository.join("new.txt")).unwrap();
		std::fs::remove_file(repository.join(".git/info/exclude")).unwrap();
		std::fs::write(repository.join("f.txt"), "a\nb\nc\nd\n").unwrap();
		let destination_moved =
			promote(&core, &actor(), previewed.binding.clone())
				.await
				.unwrap_err();
		let previewed =
			preview(&core, &workspace, PromotionDestination::LocalCheckout)
				.await;
		std::fs::write(workspace.root.join("new.txt"), "newer\n").unwrap();
		let workspace_moved = promote(&core, &actor(), previewed.binding)
			.await
			.unwrap_err();
		std::fs::write(repository.join("f.txt"), "A\nb\nc\n").unwrap();
		std::fs::write(repository.join("new.txt"), "newer\n").unwrap();
		std::fs::remove_file(repository.join("k.txt")).unwrap();
		let previewed =
			preview(&core, &workspace, PromotionDestination::LocalCheckout)
				.await;
		let empty = promote(&core, &actor(), previewed.binding)
			.await
			.unwrap_err();
		let journal_before = events(&core).await.len();
		std::fs::remove_file(repository.join("new.txt")).unwrap();
		let previewed =
			preview(&core, &workspace, PromotionDestination::LocalCheckout)
				.await;
		promote(&core, &actor(), previewed.binding.clone())
			.await
			.unwrap();
		let in_progress = promote(&core, &actor(), previewed.binding)
			.await
			.unwrap_err();

		assert_eq!(
			(
				(unbound.category, unbound.code),
				(risk_moved.category, risk_moved.code),
				(destination_moved.category, destination_moved.code),
				(workspace_moved.category, workspace_moved.code),
				(empty.category, empty.code),
				(in_progress.category, in_progress.code),
				journal_before,
				events(&core).await.len(),
			),
			(
				(
					ErrorCategory::InvalidInput,
					"workspace.promotion_unbound".into()
				),
				(ErrorCategory::Conflict, "workspace.promotion_stale".into()),
				(ErrorCategory::Conflict, "workspace.promotion_stale".into()),
				(ErrorCategory::Conflict, "workspace.promotion_stale".into()),
				(
					ErrorCategory::InvalidInput,
					"workspace.promotion_empty".into()
				),
				(
					ErrorCategory::Conflict,
					"workspace.promotion_in_progress".into()
				),
				4,
				5,
			)
		);
	}
}
