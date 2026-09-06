use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{
	NewWorkspacePromotion, PromotionConflictKindRecord,
	PromotionConflictRecord, PromotionDestinationRecord, PromotionStateRecord,
	WorkspacePromotionRecord,
};
use crate::{
	ActorRecord, EffectKindRecord, EffectSafetyRecord, NewConversation,
	NewEffect, NewProject, NewWorkspace, RetentionPolicy, Store, StoreError,
	WorkingTreeRecord,
};

const NOW_UNIX_MS: i64 = 1_700_000_000_000;

fn actor() -> ActorRecord {
	ActorRecord::InteractiveClient {
		client_id: Uuid::nil(),
	}
}

/// A Project with one Workspace, recorded so promotions have something
/// to reference.
async fn workspace(store: &Store) -> Uuid {
	let project_id = Uuid::now_v7();
	let conversation_id = Uuid::now_v7();
	let workspace_id = Uuid::now_v7();
	store
		.write(async |tx| {
			tx.insert_project(NewProject {
				project_id,
				root: "/home/jet/repo".into(),
				registered_by: actor(),
				registered_at_unix_ms: NOW_UNIX_MS,
			})
			.await?;
			tx.insert_conversation(NewConversation {
				conversation_id,
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRecord::Workspace { project_id },
				created_at_unix_ms: NOW_UNIX_MS,
			})
			.await?;
			tx.insert_workspace(NewWorkspace {
				workspace_id,
				conversation_id,
				project_id,
				root: "/home/jet/.jet/ws/a".into(),
				base_selection: "HEAD".into(),
				base_commit: "0".repeat(40),
				seed: None,
				created_at_unix_ms: NOW_UNIX_MS,
			})
			.await?;
			Ok::<_, StoreError>(())
		})
		.await
		.unwrap();
	workspace_id
}

fn promotion(
	workspace_id: Uuid,
	destination: PromotionDestinationRecord,
	state: PromotionStateRecord,
	conflicts: Vec<PromotionConflictRecord>,
) -> NewWorkspacePromotion {
	NewWorkspacePromotion {
		promotion_id: Uuid::now_v7(),
		workspace_id,
		promoted_by: actor(),
		destination,
		base_commit: "0".repeat(40),
		workspace_tree: "1".repeat(40),
		destination_commit: "2".repeat(40),
		destination_tree: "3".repeat(40),
		result_tree: "4".repeat(40),
		destination_dirty: true,
		changed_paths: 2,
		state,
		conflicts,
		recorded_at_unix_ms: NOW_UNIX_MS,
	}
}

fn recorded(
	promotion: &NewWorkspacePromotion,
	state: PromotionStateRecord,
	settled_at_unix_ms: Option<i64>,
) -> WorkspacePromotionRecord {
	WorkspacePromotionRecord {
		promotion_id: promotion.promotion_id,
		workspace_id: promotion.workspace_id,
		promoted_by: promotion.promoted_by,
		destination: promotion.destination.clone(),
		base_commit: promotion.base_commit.clone(),
		workspace_tree: promotion.workspace_tree.clone(),
		destination_commit: promotion.destination_commit.clone(),
		destination_tree: promotion.destination_tree.clone(),
		result_tree: promotion.result_tree.clone(),
		destination_dirty: promotion.destination_dirty,
		changed_paths: promotion.changed_paths,
		state,
		conflicts: promotion.conflicts.clone(),
		recorded_at_unix_ms: promotion.recorded_at_unix_ms,
		settled_at_unix_ms,
	}
}

/// A conflicted promotion is settled as it is recorded and keeps its
/// paths in order; an applying one settles once, with the outcome its
/// Effect records, and the newest promotion of a Workspace is the one
/// read back with it. The Effect that applies a promotion names it
/// (ADR-0025, ADR-0064).
#[tokio::test]
async fn a_promotion_is_recorded_with_its_conflicts_and_settled_once() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let workspace_id = workspace(&store).await;
	let conflicted = promotion(
		workspace_id,
		PromotionDestinationRecord::LocalCheckout,
		PromotionStateRecord::Conflicted,
		vec![
			PromotionConflictRecord {
				path: "src/lib.rs".into(),
				kind: PromotionConflictKindRecord::Diverged,
			},
			PromotionConflictRecord {
				path: "local.txt".into(),
				kind: PromotionConflictKindRecord::Untracked,
			},
		],
	);
	let applying = promotion(
		workspace_id,
		PromotionDestinationRecord::Branch("release".into()),
		PromotionStateRecord::Applying,
		vec![],
	);
	let effect_id = Uuid::now_v7();

	let (first, latest_after_first, second, settled, settled_twice, latest) =
		store
			.write(async |tx| {
				let first = tx.insert_promotion(conflicted.clone()).await?;
				let latest_after_first =
					tx.latest_promotion(workspace_id).await?;
				let second = tx.insert_promotion(applying.clone()).await?;
				tx.insert_effect(&NewEffect {
					effect_id,
					command_id: Uuid::now_v7(),
					run_id: None,
					promotion_id: Some(applying.promotion_id),
					kind: EffectKindRecord::PromoteWorkspace,
					safety: EffectSafetyRecord::Ambiguous,
				})
				.await?;
				let settled = tx
					.settle_promotion(
						applying.promotion_id,
						PromotionStateRecord::Promoted,
						NOW_UNIX_MS + 1,
					)
					.await?;
				let settled_twice = tx
					.settle_promotion(
						applying.promotion_id,
						PromotionStateRecord::Failed,
						NOW_UNIX_MS + 2,
					)
					.await
					.is_err();
				let latest = tx.latest_promotion(workspace_id).await?;
				Ok::<_, StoreError>((
					first,
					latest_after_first,
					second,
					settled,
					settled_twice,
					latest,
				))
			})
			.await
			.unwrap();
	let effects = store
		.read(async |tx| {
			tx.unresolved_effects_of(EffectKindRecord::PromoteWorkspace)
				.await
		})
		.await
		.unwrap();

	assert_eq!(
		(
			&first,
			latest_after_first.as_ref(),
			&second,
			&settled,
			settled_twice,
			latest.as_ref(),
			effects
				.iter()
				.map(|effect| (effect.effect_id, effect.promotion_id))
				.collect::<Vec<_>>(),
		),
		(
			&recorded(
				&conflicted,
				PromotionStateRecord::Conflicted,
				Some(NOW_UNIX_MS)
			),
			Some(&recorded(
				&conflicted,
				PromotionStateRecord::Conflicted,
				Some(NOW_UNIX_MS)
			)),
			&recorded(&applying, PromotionStateRecord::Applying, None),
			&recorded(
				&applying,
				PromotionStateRecord::Promoted,
				Some(NOW_UNIX_MS + 1)
			),
			true,
			Some(&recorded(
				&applying,
				PromotionStateRecord::Promoted,
				Some(NOW_UNIX_MS + 1)
			)),
			vec![(effect_id, Some(applying.promotion_id))],
		)
	);
}
