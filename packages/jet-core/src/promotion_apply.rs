//! The Git operations that apply a recorded promotion and observe
//! whether one was applied (ADR-0025, ADR-0067).
//!
//! Applying to the Local checkout writes the bound result over a
//! checkout verified, moments before, to be exactly as previewed. A
//! scratch index holding the previewed tree carries the two-tree read
//! that updates the files, and the checkout's own index is then told
//! the result's entries for those paths alone, so what the user had
//! staged elsewhere stays staged. The changes arrive staged, as a seed's
//! do, because a Git-link change has no unstaged form. Applying to a
//! branch writes one commit holding the result on top of the previewed
//! tip and moves the branch to it only if it is still at that tip.

use std::path::Path;

use jet_store::{PromotionDestinationRecord, WorkspacePromotionRecord};

use crate::effect::EffectResult;
use crate::error::CoreError;
use crate::promotion_merge::{self as merge, promotion_failed};
use crate::repository::{git, git_with_input};
use crate::tree_capture::{Change, ScratchIndex, diff_trees};

/// How long one application may take. A large result on a slow disk
/// stalls one Effect, not the Plane.
const APPLY_BUDGET: std::time::Duration = std::time::Duration::from_secs(300);

/// What a destination holds against a promotion's binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Observed {
	/// The bound result: the promotion was applied.
	Applied,
	/// Exactly what the preview bound: nothing was applied.
	Untouched,
	/// Something else: the destination moved, or an attempt stopped part
	/// way.
	Elsewhere,
}

/// Applies `promotion` to its destination at `root`, through scratch
/// indexes under `scratch`.
///
/// The answer is definite whenever it can be: a destination that is not
/// as previewed fails before anything is written, and one already
/// holding the result is complete. Only a failure after writing began
/// is unknown.
pub(crate) async fn apply(
	root: &Path,
	scratch: &Path,
	promotion: &WorkspacePromotionRecord,
) -> EffectResult {
	let applied = async {
		match &promotion.destination {
			PromotionDestinationRecord::LocalCheckout => {
				apply_to_checkout(root, scratch, promotion).await
			}
			PromotionDestinationRecord::Branch(name) => {
				apply_to_branch(root, name, promotion).await
			}
		}
	};
	match tokio::time::timeout(APPLY_BUDGET, applied).await {
		Ok(result) => result,
		Err(_) => EffectResult::Unknown,
	}
}

async fn apply_to_checkout(
	root: &Path,
	scratch: &Path,
	promotion: &WorkspacePromotionRecord,
) -> EffectResult {
	match observe_checkout(root, &scratch.join("before"), promotion).await {
		Ok(Observed::Untouched) => {}
		Ok(Observed::Applied) => return EffectResult::Completed,
		// Nothing has been written yet, so a destination that is not as
		// previewed, or a Git that cannot look, is a definite failure.
		Ok(Observed::Elsewhere) | Err(_) => return EffectResult::Failed,
	}
	let index_path = scratch.join("apply");
	let index = ScratchIndex::new(root, &index_path, promotion_failed);
	let prepared = async {
		index
			.run(&[
				"read-tree",
				"--end-of-options",
				&promotion.destination_tree,
			])
			.await?;
		index.run(&["update-index", "--refresh"]).await
	};
	if prepared.await.is_err() {
		return EffectResult::Failed;
	}
	// From here on the checkout is being written.
	let written = async {
		index
			.run(&[
				"read-tree",
				"-m",
				"-u",
				"--end-of-options",
				&promotion.destination_tree,
				&promotion.result_tree,
			])
			.await?;
		stage_result(root, promotion).await
	};
	if written.await.is_err() {
		return EffectResult::Unknown;
	}
	match observe_checkout(root, &scratch.join("after"), promotion).await {
		Ok(Observed::Applied) => EffectResult::Completed,
		Ok(Observed::Untouched | Observed::Elsewhere) | Err(_) => {
			EffectResult::Unknown
		}
	}
}

/// Tells the checkout's own index the result's entries at the paths the
/// promotion changes, and nothing else.
async fn stage_result(
	root: &Path,
	promotion: &WorkspacePromotionRecord,
) -> Result<(), CoreError> {
	let changes = diff_trees(
		root,
		&promotion.destination_tree,
		&promotion.result_tree,
		promotion_failed,
	)
	.await?;
	let info = changes
		.iter()
		.map(Change::index_info)
		.collect::<String>()
		.into_bytes();
	let staged =
		git_with_input(root, &["update-index", "-z", "--index-info"], info)
			.await?;
	if !staged.status.success() {
		return Err(promotion_failed(staged.stderr));
	}
	Ok(())
}

async fn apply_to_branch(
	root: &Path,
	name: &str,
	promotion: &WorkspacePromotionRecord,
) -> EffectResult {
	let reference = format!("refs/heads/{name}");
	match observe_branch(root, &reference, promotion).await {
		Ok(Observed::Untouched) => {}
		Ok(Observed::Applied) => return EffectResult::Completed,
		Ok(Observed::Elsewhere) | Err(_) => return EffectResult::Failed,
	}
	let message = format!(
		"Promote Workspace {}\n\nJet applied the Workspace's changes to {name} \
		 as previewed.\n",
		promotion.workspace_id
	);
	let committed = git(
		root,
		&[
			"commit-tree",
			"-p",
			&promotion.destination_commit,
			"-m",
			&message,
			"--end-of-options",
			&promotion.result_tree,
		],
	)
	.await;
	// Writing a commit object moves no reference, so a commit that could
	// not be written is a definite failure.
	let commit = match committed {
		Ok(output) if output.status.success() => {
			output.stdout.trim().to_owned()
		}
		Ok(_) | Err(_) => return EffectResult::Failed,
	};
	// The one mutation: move the branch only if it is still at the tip
	// the preview bound.
	let moved = git(
		root,
		&[
			"update-ref",
			"--end-of-options",
			&reference,
			&commit,
			&promotion.destination_commit,
		],
	)
	.await;
	match moved {
		Ok(output) if output.status.success() => EffectResult::Completed,
		Ok(_) | Err(_) => {
			match observe_branch(root, &reference, promotion).await {
				Ok(Observed::Applied) => EffectResult::Completed,
				Ok(Observed::Untouched | Observed::Elsewhere) => {
					EffectResult::Failed
				}
				Err(_) => EffectResult::Unknown,
			}
		}
	}
}

/// Observes what the destination of `promotion` holds, without changing
/// anything.
///
/// # Errors
///
/// Returns what Git reports when it cannot look.
pub(crate) async fn observe(
	root: &Path,
	scratch: &Path,
	promotion: &WorkspacePromotionRecord,
) -> Result<Observed, CoreError> {
	match &promotion.destination {
		PromotionDestinationRecord::LocalCheckout => {
			observe_checkout(root, &scratch.join("observe"), promotion).await
		}
		PromotionDestinationRecord::Branch(name) => {
			observe_branch(root, &format!("refs/heads/{name}"), promotion).await
		}
	}
}

async fn observe_checkout(
	root: &Path,
	index: &Path,
	promotion: &WorkspacePromotionRecord,
) -> Result<Observed, CoreError> {
	let Some(head) = merge::resolve(root, "HEAD").await? else {
		return Ok(Observed::Elsewhere);
	};
	if head != promotion.destination_commit {
		return Ok(Observed::Elsewhere);
	}
	let tree = merge::snapshot(root, index, &head).await?;
	Ok(if tree == promotion.result_tree {
		Observed::Applied
	} else if tree == promotion.destination_tree {
		Observed::Untouched
	} else {
		Observed::Elsewhere
	})
}

/// A branch holds the result when its tip is a commit of the result tree
/// whose first parent is the previewed tip: the commit this promotion
/// writes, whichever attempt wrote it.
async fn observe_branch(
	root: &Path,
	reference: &str,
	promotion: &WorkspacePromotionRecord,
) -> Result<Observed, CoreError> {
	let Some(tip) = merge::resolve(root, reference).await? else {
		return Ok(Observed::Elsewhere);
	};
	if tip == promotion.destination_commit {
		return Ok(Observed::Untouched);
	}
	let parent = format!("{tip}^1");
	let tree = merge::tree_of(root, &tip).await?;
	let first_parent = merge::resolve(root, &parent).await?;
	Ok(
		if tree == promotion.result_tree
			&& first_parent.as_deref() == Some(&*promotion.destination_commit)
		{
			Observed::Applied
		} else {
			Observed::Elsewhere
		},
	)
}
