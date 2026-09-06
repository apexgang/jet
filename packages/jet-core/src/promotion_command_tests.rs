use jet_store::{EffectKindRecord, EffectStateRecord};
use pretty_assertions::assert_eq;

use crate::test_support::{
	Diverged, actor, conversation_snapshot as snapshot, diverged, events,
	preview_promotion, request, status,
};
use crate::{
	Actor, ClientId, Command, CommandOutcome, ConflictKind, Core, CoreError,
	ErrorCategory, EventKind, PromotionBinding, PromotionConflict,
	PromotionDestination, PromotionPreview, PromotionState, Workspace,
	WorkspacePromotion,
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
	let CommandOutcome::WorkspacePromotionRecorded(promotion) = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	Ok(promotion)
}

/// The unresolved promotion Effects, as their promotion identities and
/// states.
async fn pending_effects(core: &Core) -> Vec<(uuid::Uuid, EffectStateRecord)> {
	core.store
		.read(async |tx| {
			Ok::<_, CoreError>(
				tx.unresolved_effects_of(EffectKindRecord::PromoteWorkspace)
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
		preview(&core, &workspace, PromotionDestination::LocalCheckout).await;

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
		preview(&core, &workspace, PromotionDestination::LocalCheckout).await;

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
		preview(&core, &workspace, PromotionDestination::LocalCheckout).await;
	let unbound = promote(&core, &other, previewed.binding.clone())
		.await
		.unwrap_err();
	// An ignored file appearing where the Workspace adds a path changes
	// no tree, only the risk the preview showed.
	std::fs::write(repository.join(".git/info/exclude"), "new.txt\n").unwrap();
	std::fs::write(repository.join("new.txt"), "ignored\n").unwrap();
	let risk_moved = promote(&core, &actor(), previewed.binding.clone())
		.await
		.unwrap_err();
	std::fs::remove_file(repository.join("new.txt")).unwrap();
	std::fs::remove_file(repository.join(".git/info/exclude")).unwrap();
	std::fs::write(repository.join("f.txt"), "a\nb\nc\nd\n").unwrap();
	let destination_moved = promote(&core, &actor(), previewed.binding.clone())
		.await
		.unwrap_err();
	let previewed =
		preview(&core, &workspace, PromotionDestination::LocalCheckout).await;
	std::fs::write(workspace.root.join("new.txt"), "newer\n").unwrap();
	let workspace_moved = promote(&core, &actor(), previewed.binding)
		.await
		.unwrap_err();
	std::fs::write(repository.join("f.txt"), "A\nb\nc\n").unwrap();
	std::fs::write(repository.join("new.txt"), "newer\n").unwrap();
	std::fs::remove_file(repository.join("k.txt")).unwrap();
	let previewed =
		preview(&core, &workspace, PromotionDestination::LocalCheckout).await;
	let empty = promote(&core, &actor(), previewed.binding)
		.await
		.unwrap_err();
	let journal_before = events(&core).await.len();
	std::fs::remove_file(repository.join("new.txt")).unwrap();
	let previewed =
		preview(&core, &workspace, PromotionDestination::LocalCheckout).await;
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
