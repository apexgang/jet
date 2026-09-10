//! The Effect that applies a recorded Workspace promotion (ADR-0025,
//! ADR-0064, ADR-0067).
//!
//! The Adapter reads the promotion its Effect names, checks the
//! destination against what the preview bound right before writing, and
//! applies the bound result with the Git operations in
//! [`crate::promotion::apply`]. A destination that moved is a definite
//! failure that changes nothing. An attempt interrupted after writing
//! began is looked at again: a checkout that holds the result is done, a
//! branch still at its tip is safe to try once more, and anything else
//! is an outcome Jet does not know and does not guess.

use crate::{
	Actor, ConversationId, Core,
	effect::{Effect, EffectAdapter, EffectKind, EffectResult},
	error::CoreError,
	event::{EventKind, EventSubject},
	promotion::{
		self, PromotionId, WorkspacePromotion,
		apply::{self as promotion_apply, Observed},
	},
	workspace::{self, WorkspaceHome, WorkspaceId},
};
use jet_store::{
	EffectKindRecord, EffectStateRecord, PromotionDestinationRecord,
	PromotionStateRecord, Store, WorkspacePromotionRecord, WriteTransaction,
};
use std::path::PathBuf;

/// Most passes one call makes over the outbox: enough for a branch
/// promotion to use every attempt its safety allows.
const PASSES: u32 = 4;

/// Performs promotions through the Plane's Git, with the Plane store to
/// read what each Effect names.
struct GitPromoter<'a> {
	store: &'a Store,
	home: &'a WorkspaceHome,
}

/// What an Effect names, read from the store: the promotion and the
/// Project it applies to.
struct Target {
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
		// An attempt whose outcome is unknown is looked at again in this
		// same call, the way a restart would look at it, so a promotion
		// settles now rather than at the next Command: a checkout becomes
		// an outcome unknown, a branch still at its tip is tried again
		// within its bound (ADR-0067).
		for _ in 0..PASSES {
			let effects = self
				.reconcile_effects(
					&mut adapter,
					EffectKindRecord::PromoteWorkspace,
				)
				.await?;
			if !effects
				.iter()
				.any(|effect| effect.state == EffectStateRecord::InFlight)
			{
				break;
			}
		}
		Ok(())
	}
}

impl EffectAdapter for GitPromoter<'_> {
	async fn execute(&mut self, effect: &Effect) -> EffectResult {
		let EffectKind::PromoteWorkspace { promotion_id } = effect.kind else {
			return EffectResult::Unknown;
		};
		let Some(target) = self.target(promotion_id).await else {
			return EffectResult::Unknown;
		};
		let Target {
			promotion,
			project_root,
		} = &target;
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
		let Some(target) = self.target(promotion_id).await else {
			return EffectResult::Unknown;
		};
		let Target {
			promotion,
			project_root,
		} = &target;
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
				PromotionDestinationRecord::LocalCheckout => {
					EffectResult::Failed
				}
				PromotionDestinationRecord::Branch(_) => EffectResult::Unknown,
			},
			Ok(Observed::Elsewhere) | Err(_) => EffectResult::Unknown,
		}
	}
}

impl GitPromoter<'_> {
	/// Reads what the Effect names. A promotion, Workspace, or Project the
	/// store no longer has is not something this Adapter can act on.
	async fn target(&self, promotion_id: PromotionId) -> Option<Target> {
		self.store
			.read(async |tx| {
				let Some(promotion) = tx.promotion(promotion_id.0).await?
				else {
					return Ok(None);
				};
				let (_, project_root) = promotion::workspace_and_project(
					tx,
					WorkspaceId(promotion.workspace_id),
				)
				.await?;
				Ok::<_, CoreError>(Some(Target {
					promotion,
					project_root,
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
mod tests {
	use std::path::Path;

	use jet_store::{EffectKindRecord, EffectRecord};
	use pretty_assertions::assert_eq;

	use crate::test_support::{
		Diverged, actor, conversation_snapshot as snapshot, diverged, events,
		git, preview_promotion, request, start_core, status,
	};
	use crate::{
		Command, CommandOutcome, Core, CoreError, EventKind,
		PromotionDestination, PromotionState, Workspace, WorkspacePromotion,
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
		let CommandOutcome::WorkspacePromotionRecorded(promotion) = outcome
		else {
			panic!("expected CommandOutcome::WorkspacePromotionRecorded");
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
					tx.unresolved_effects_of(
						EffectKindRecord::PromoteWorkspace,
					)
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
			promote(&core, &workspace, PromotionDestination::LocalCheckout)
				.await;

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
				"M  f.txt\nD  k.txt\nA  new.txt\nA  o.txt\n?? notes.txt\n"
					.into(),
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
				git(&repository, &["log", "-1", "--format=%s", "release"])
					.trim(),
				git(&repository, &["rev-parse", "HEAD"]).trim(),
				status(&repository),
			),
			(
				PromotionState::Promoted,
				recorded.binding.result_tree.as_str(),
				base.as_str(),
				format!("Promote Workspace {}", workspace.workspace_id.0)
					.as_str(),
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
					&[
						"read-tree",
						"--reset",
						"-u",
						&recorded.binding.result_tree,
					],
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
					&[
						"update-ref",
						&format!("refs/heads/{name}"),
						commit.trim(),
					],
				);
			}
			(PromotionDestination::LocalCheckout, Then::Elsewhere) => {
				std::fs::write(repository.join("f.txt"), "elsewhere\n")
					.unwrap();
			}
			(PromotionDestination::Branch(name), Then::Elsewhere) => {
				let commit = git(
					&repository,
					&[
						"commit-tree",
						"-p",
						&base,
						"-m",
						"Elsewhere",
						"HEAD^{tree}",
					],
				);
				git(
					&repository,
					&[
						"update-ref",
						&format!("refs/heads/{name}"),
						commit.trim(),
					],
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
}
