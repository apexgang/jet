use std::path::Path;

use jet_store::{EffectKindRecord, EffectRecord};
use pretty_assertions::assert_eq;

use crate::test_support::{
	Diverged, actor, conversation_snapshot as snapshot, diverged, events, git,
	preview_promotion, request, start_core, status,
};
use crate::{
	Command, CommandOutcome, Core, CoreError, EventKind, PromotionDestination,
	PromotionState, Workspace, WorkspacePromotion,
};

/// Previews and promotes `workspace` to `destination`, returning the
/// promotion as recorded.
async fn promote(
	core: &Core,
	workspace: &Workspace,
	destination: PromotionDestination,
) -> WorkspacePromotion {
	let previewed =
		preview_promotion(core, workspace.workspace_id, destination)
			.await
			.unwrap();
	let outcome = core
		.execute(
			&actor(),
			request(Command::PromoteWorkspace {
				binding: previewed.binding,
			}),
		)
		.await
		.unwrap();
	let CommandOutcome::WorkspacePromotionRecorded(promotion) = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	promotion
}

/// The Workspace's most recent promotion as its snapshot shows it.
async fn shown(core: &Core, workspace: &Workspace) -> WorkspacePromotion {
	snapshot(core, workspace.conversation_id)
		.await
		.workspace
		.unwrap()
		.promotion
		.unwrap()
}

/// The unresolved promotion Effects.
async fn unresolved(core: &Core) -> Vec<EffectRecord> {
	core.store
		.read(async |tx| {
			Ok::<_, CoreError>(
				tx.unresolved_effects_of(EffectKindRecord::PromoteWorkspace)
					.await?,
			)
		})
		.await
		.unwrap()
}

fn read(root: &Path, path: &str) -> Option<String> {
	std::fs::read_to_string(root.join(path)).ok()
}

/// A promotion's Effect writes the bound result into the Local checkout:
/// the merged files land, paths the Workspace deleted go, the changed
/// paths arrive staged beside what the user had staged, the untracked
/// file both sides hold stays untracked, HEAD does not move, and the
/// Workspace is left alone. The promotion settles as promoted, is
/// journaled, and leaves no Effect behind (ADR-0025, ADR-0064).
#[tokio::test]
async fn a_promotion_effect_writes_the_result_into_the_checkout() {
	let dir = tempfile::tempdir().unwrap();
	let Diverged {
		core,
		repository,
		base,
		workspace,
	} = diverged(dir.path()).await;
	let workspace_before = status(&workspace.root);
	let recorded =
		promote(&core, &workspace, PromotionDestination::LocalCheckout).await;

	core.perform_promotions().await.unwrap();
	let settled = shown(&core, &workspace).await;

	assert_eq!(
		(
			&settled,
			read(&repository, "f.txt"),
			read(&repository, "new.txt"),
			read(&repository, "k.txt"),
			status(&repository),
			git(&repository, &["rev-parse", "HEAD"]).trim(),
			status(&workspace.root),
			events(&core).await.last(),
			unresolved(&core).await.len(),
		),
		(
			&WorkspacePromotion {
				state: PromotionState::Promoted,
				settled_at: settled.settled_at,
				..recorded.clone()
			},
			Some("A\nb\nC\n".into()),
			Some("new\n".into()),
			None,
			"M  f.txt\nD  k.txt\nA  new.txt\nA  o.txt\n?? notes.txt\n".into(),
			base.as_str(),
			workspace_before,
			Some(&EventKind::WorkspacePromotionSettled {
				workspace_id: workspace.workspace_id,
				promotion_id: recorded.promotion_id,
				state: PromotionState::Promoted,
			}),
			0,
		)
	);
}

/// A promotion to a branch adds one commit holding the result on top of
/// the previewed tip, and touches neither the Local checkout nor the
/// branch it has checked out (ADR-0025).
#[tokio::test]
async fn a_promotion_effect_adds_one_commit_to_the_branch() {
	let dir = tempfile::tempdir().unwrap();
	let Diverged {
		core,
		repository,
		base,
		workspace,
	} = diverged(dir.path()).await;
	git(&repository, &["branch", "release", &base]);
	let checkout_before = status(&repository);
	let recorded = promote(
		&core,
		&workspace,
		PromotionDestination::Branch("release".into()),
	)
	.await;

	core.perform_promotions().await.unwrap();

	assert_eq!(
		(
			shown(&core, &workspace).await.state,
			git(&repository, &["rev-parse", "release^{tree}"]).trim(),
			git(&repository, &["rev-parse", "release^1"]).trim(),
			git(&repository, &["log", "-1", "--format=%s", "release"]).trim(),
			git(&repository, &["rev-parse", "HEAD"]).trim(),
			status(&repository),
		),
		(
			PromotionState::Promoted,
			recorded.binding.result_tree.as_str(),
			base.as_str(),
			format!("Promote Workspace {}", workspace.workspace_id.0).as_str(),
			base.as_str(),
			checkout_before,
		)
	);
}

/// A destination that moved between the promotion's commit and its
/// Effect fails the promotion before anything is written (ADR-0025).
#[tokio::test]
async fn a_promotion_effect_fails_when_the_destination_moved() {
	let dir = tempfile::tempdir().unwrap();
	let Diverged {
		core,
		repository,
		workspace,
		..
	} = diverged(dir.path()).await;
	promote(&core, &workspace, PromotionDestination::LocalCheckout).await;
	std::fs::write(repository.join("f.txt"), "a\nb\nC\nd\n").unwrap();
	let checkout_before = status(&repository);

	core.perform_promotions().await.unwrap();

	assert_eq!(
		(
			shown(&core, &workspace).await.state,
			read(&repository, "f.txt"),
			read(&repository, "new.txt"),
			status(&repository),
		),
		(
			PromotionState::Failed,
			Some("a\nb\nC\nd\n".into()),
			None,
			checkout_before,
		)
	);
}

/// What a restarted daemon finds after an interrupted attempt.
enum Then {
	/// Nothing was written before the interruption.
	Nothing,
	/// The result had landed before the interruption.
	Result,
	/// The destination is something else entirely.
	Elsewhere,
}

/// Records a promotion to `destination`, marks its Effect as an attempt
/// that never reported, arranges the destination as `then` says, and
/// returns where a restarted core settles it.
async fn interrupted(
	dir: &Path,
	destination: PromotionDestination,
	then: Then,
) -> PromotionState {
	let Diverged {
		core,
		repository,
		base,
		workspace,
	} = diverged(dir).await;
	git(&repository, &["branch", "release", &base]);
	let recorded = promote(&core, &workspace, destination.clone()).await;
	let effect_id = unresolved(&core).await[0].effect_id;
	core.store
		.write(async |tx| tx.begin_effect_attempt(effect_id).await)
		.await
		.unwrap();
	match (destination, then) {
		(_, Then::Nothing) => {}
		(PromotionDestination::LocalCheckout, Then::Result) => {
			git(
				&repository,
				&["read-tree", "--reset", "-u", &recorded.binding.result_tree],
			);
		}
		(PromotionDestination::Branch(name), Then::Result) => {
			let commit = git(
				&repository,
				&[
					"commit-tree",
					"-p",
					&base,
					"-m",
					"Landed",
					&recorded.binding.result_tree,
				],
			);
			git(
				&repository,
				&["update-ref", &format!("refs/heads/{name}"), commit.trim()],
			);
		}
		(PromotionDestination::LocalCheckout, Then::Elsewhere) => {
			std::fs::write(repository.join("f.txt"), "elsewhere\n").unwrap();
		}
		(PromotionDestination::Branch(name), Then::Elsewhere) => {
			let commit = git(
				&repository,
				&["commit-tree", "-p", &base, "-m", "Elsewhere", "HEAD^{tree}"],
			);
			git(
				&repository,
				&["update-ref", &format!("refs/heads/{name}"), commit.trim()],
			);
		}
	}
	drop(core);

	let restarted = start_core(&dir.join("plane.sqlite3")).await;
	restarted.perform_promotions().await.unwrap();
	assert_eq!(unresolved(&restarted).await, vec![]);
	shown(&restarted, &workspace).await.state
}

/// An attempt a previous daemon never finished is settled from what the
/// destination holds, never by guessing (ADR-0067). A checkout still as
/// previewed was never written and fails; one holding the result is
/// promoted; one holding something else is an outcome unknown, and is
/// not tried again because a checkout write cannot be repeated safely.
/// A branch still at its tip is tried once more under the same identity
/// and promoted; one already holding the result is promoted without
/// another commit; one moved elsewhere fails. Every case settles within
/// the one call, as it would after any Command.
#[tokio::test]
async fn an_interrupted_attempt_is_settled_from_the_destination() {
	let mut settled = Vec::new();
	for (destination, then) in [
		(PromotionDestination::LocalCheckout, Then::Nothing),
		(PromotionDestination::LocalCheckout, Then::Result),
		(PromotionDestination::LocalCheckout, Then::Elsewhere),
		(
			PromotionDestination::Branch("release".into()),
			Then::Nothing,
		),
		(PromotionDestination::Branch("release".into()), Then::Result),
		(
			PromotionDestination::Branch("release".into()),
			Then::Elsewhere,
		),
	] {
		let dir = tempfile::tempdir().unwrap();
		settled.push(interrupted(dir.path(), destination, then).await);
	}

	assert_eq!(
		settled,
		vec![
			PromotionState::Failed,
			PromotionState::Promoted,
			PromotionState::OutcomeUnknown,
			PromotionState::Promoted,
			PromotionState::Promoted,
			PromotionState::Failed,
		]
	);
}
