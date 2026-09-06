//! Promoting a Workspace's changes to a permanent checkout or branch of
//! its Project (ADR-0025).
//!
//! A promotion is previewed before it is made. The preview captures the
//! Workspace's whole working tree as one Git tree, reads the destination
//! as it stands, and merges the two against the Workspace's base with a
//! three-way tree merge that touches nothing. What the preview shows is
//! bound: the base, the two trees, the destination's commit, the proposed
//! result, and the Actor it was shown to travel back with the promotion,
//! which is refused when the Workspace or the destination has moved on.
//! A conflict is never resolved by Jet: the preview names it, and nothing
//! is written over the destination's work.

use std::path::Path;

use jet_store::WorkspaceRecord;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::event::EventSequence;
use crate::query::QueryResult;
use crate::tree_capture::Change;
use crate::workspace::{self, WorkspaceHome, WorkspaceId};
use crate::{Actor, ClientId, Core, promotion_merge as merge};

/// Longest branch name the core accepts, as text.
const MAX_BRANCH_CHARS: usize = 1024;

/// Most changes one preview lists. The count is always complete; a
/// promotion that touches more paths than a control frame carries is
/// listed up to here.
pub(crate) const MAX_PREVIEW_CHANGES: usize = 4096;

/// How long a preview may take. Hashing two large working trees on a
/// slow disk stalls one Query, not the Plane.
const PREVIEW_BUDGET: std::time::Duration = std::time::Duration::from_secs(300);

/// Where a promotion applies a Workspace's changes: a permanent place in
/// its Project, chosen by the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromotionDestination {
	/// The Project's own Local checkout. The changes arrive staged in its
	/// index and written to its files, merged over whatever it holds.
	LocalCheckout,
	/// A branch of the Project that no working tree has checked out. The
	/// changes arrive as one commit on top of the branch.
	Branch(String),
}

/// What a preview showed and a promotion carries back: the Workspace and
/// destination as they stood, the result the user looked at, and the
/// Actor it was shown to. A promotion is refused when any of it has
/// changed since (ADR-0025).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionBinding {
	/// The Workspace being promoted.
	pub workspace_id: WorkspaceId,
	/// Where its changes go.
	pub destination: PromotionDestination,
	/// The commit the Workspace started from, which the merge is against.
	pub base_commit: String,
	/// The Workspace's working tree, captured as one tree.
	pub workspace_tree: String,
	/// The commit the destination had checked out or pointed at.
	pub destination_commit: String,
	/// The destination's content as one tree: its working tree for the
	/// Local checkout, the tip's tree for a branch.
	pub destination_tree: String,
	/// The tree the three-way merge produced.
	pub result_tree: String,
	/// The Client identity the preview was shown to.
	pub actor: ClientId,
}

/// What promoting a Workspace would do, shown before it is done
/// (ADR-0025). Fenced by the journal position the Workspace was read at
/// (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionPreview {
	/// Newest Event sequence visible when the Workspace was read.
	pub cursor: EventSequence,
	/// What the promotion is bound to.
	pub binding: PromotionBinding,
	/// Whether the destination holds uncommitted changes of its own, which
	/// the promotion is merged over and never discards.
	pub destination_dirty: bool,
	/// How many paths the promotion changes in the destination.
	pub changed_paths: u32,
	/// The changes, up to [`MAX_PREVIEW_CHANGES`] of them, in Git's order.
	pub changes: Vec<PromotedChange>,
	/// The paths the promotion cannot settle. A preview with any is shown
	/// and never applied.
	pub conflicts: Vec<PromotionConflict>,
}

/// One path a promotion changes in the destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotedChange {
	/// The path, as Git spells it.
	pub path: String,
	/// What happens to it.
	pub kind: ChangeKind,
}

/// What a promotion does to one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
	/// The destination gains the path.
	Added,
	/// The destination's content or mode at the path changes.
	Modified,
	/// The destination loses the path.
	Deleted,
}

/// One path a promotion cannot settle without a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionConflict {
	/// The path, as Git spells it.
	pub path: String,
	/// Why it cannot be settled.
	pub kind: ConflictKind,
}

/// Why a path cannot be promoted as it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictKind {
	/// The Workspace and the destination both changed the path since the
	/// base, in ways Git cannot combine.
	Diverged,
	/// The Workspace adds the path and the destination already holds
	/// something there that Git ignores, so the merge never saw it. An
	/// untracked file Git does not ignore is part of what is merged, and
	/// collides as a divergence instead.
	Untracked,
}

/// A preview as computed, before the journal fence is added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Computed {
	pub(crate) binding: PromotionBinding,
	pub(crate) destination_dirty: bool,
	pub(crate) changed_paths: u32,
	pub(crate) changes: Vec<PromotedChange>,
	pub(crate) conflicts: Vec<PromotionConflict>,
}

impl PromotionDestination {
	/// Refuses a destination Git could read as something other than one
	/// branch name, before it reaches a subprocess.
	pub(crate) fn validate(&self) -> Result<(), CoreError> {
		match self {
			Self::LocalCheckout => Ok(()),
			Self::Branch(name) => {
				let malformed = name.is_empty()
					|| name.chars().count() > MAX_BRANCH_CHARS
					|| name.starts_with('-')
					|| name
						.chars()
						.any(|c| c.is_control() || c.is_whitespace());
				if malformed {
					return Err(destination_invalid());
				}
				Ok(())
			}
		}
	}
}

/// Shows what promoting `workspace_id` to `destination` would do, without
/// changing anything.
///
/// # Errors
///
/// Returns `workspace.not_found` or `project.not_found` when either is
/// gone, `workspace.promotion_destination_invalid`,
/// `workspace.promotion_branch_not_found`, or
/// `workspace.promotion_branch_checked_out` when the destination cannot
/// be promoted to, and an `unavailable` `workspace.promotion_failed` when
/// Git cannot compare the two.
pub(crate) async fn preview(
	core: &Core,
	actor: &Actor,
	workspace_id: WorkspaceId,
	destination: PromotionDestination,
) -> Result<QueryResult, CoreError> {
	destination.validate()?;
	let (cursor, workspace, project_root) = core
		.store
		.read(async |tx| {
			let cursor = EventSequence(tx.event_cursor().await?);
			let Some(workspace) = tx.workspace(workspace_id.0).await? else {
				return Err(workspace_not_found());
			};
			let Some(project) = tx.project(workspace.project_id).await? else {
				return Err(CoreError::not_found(
					"project.not_found",
					"the Project is not registered",
				));
			};
			Ok((cursor, workspace, std::path::PathBuf::from(project.root)))
		})
		.await?;
	let Computed {
		binding,
		destination_dirty,
		changed_paths,
		changes,
		conflicts,
	} = compute(
		&core.workspace_home,
		actor,
		&workspace,
		&project_root,
		destination,
	)
	.await?;
	Ok(QueryResult::PromotionPreview(PromotionPreview {
		cursor,
		binding,
		destination_dirty,
		changed_paths,
		changes,
		conflicts,
	}))
}

/// Computes the preview from the repository as it is right now, outside
/// any store lock.
pub(crate) async fn compute(
	home: &WorkspaceHome,
	actor: &Actor,
	workspace: &WorkspaceRecord,
	project_root: &Path,
	destination: PromotionDestination,
) -> Result<Computed, CoreError> {
	tokio::time::timeout(
		PREVIEW_BUDGET,
		workspace::with_scratch(home, "promotion", async |scratch| {
			compute_in(scratch, actor, workspace, project_root, destination)
				.await
		}),
	)
	.await
	.map_err(|_| {
		merge::promotion_failed("the preview did not finish in time".into())
	})?
}

async fn compute_in(
	scratch: &Path,
	actor: &Actor,
	workspace: &WorkspaceRecord,
	project_root: &Path,
	destination: PromotionDestination,
) -> Result<Computed, CoreError> {
	let workspace_root = Path::new(&workspace.root);
	let Some(workspace_head) = merge::resolve(workspace_root, "HEAD").await?
	else {
		return Err(merge::promotion_failed(
			"the Workspace has no commit checked out".into(),
		));
	};
	let workspace_tree = merge::snapshot(
		workspace_root,
		&scratch.join("workspace"),
		&workspace_head,
	)
	.await?;
	let DestinationState {
		commit: destination_commit,
		tree: destination_tree,
		dirty: destination_dirty,
	} = destination_state(project_root, &destination, scratch).await?;
	let merged = merge::merge(
		project_root,
		&workspace.base_commit,
		&destination_tree,
		&workspace_tree,
	)
	.await?;
	let changed =
		merge::changes(project_root, &destination_tree, &merged.tree).await?;
	let mut conflicts: Vec<PromotionConflict> = merged
		.conflicts
		.into_iter()
		.map(|path| PromotionConflict {
			path,
			kind: ConflictKind::Diverged,
		})
		.collect();
	if destination == PromotionDestination::LocalCheckout {
		let added = changed
			.iter()
			.filter(|change| change.is_addition())
			.map(|change| change.path.clone())
			.collect();
		conflicts.extend(
			merge::occupied(project_root, added).await?.into_iter().map(
				|path| PromotionConflict {
					path,
					kind: ConflictKind::Untracked,
				},
			),
		);
	}
	Ok(Computed {
		binding: PromotionBinding {
			workspace_id: WorkspaceId(workspace.workspace_id),
			destination,
			base_commit: workspace.base_commit.clone(),
			workspace_tree,
			destination_commit,
			destination_tree,
			result_tree: merged.tree,
			actor: actor.client_id(),
		},
		destination_dirty,
		changed_paths: u32::try_from(changed.len()).unwrap_or(u32::MAX),
		changes: changed
			.into_iter()
			.take(MAX_PREVIEW_CHANGES)
			.map(PromotedChange::from)
			.collect(),
		conflicts,
	})
}

/// The destination as it stands: the commit it is at, its content as one
/// tree, and whether the two differ.
struct DestinationState {
	commit: String,
	tree: String,
	dirty: bool,
}

async fn destination_state(
	project_root: &Path,
	destination: &PromotionDestination,
	scratch: &Path,
) -> Result<DestinationState, CoreError> {
	match destination {
		PromotionDestination::LocalCheckout => {
			let Some(commit) = merge::resolve(project_root, "HEAD").await?
			else {
				return Err(merge::promotion_failed(
					"the Local checkout has no commit checked out".into(),
				));
			};
			let tree = merge::snapshot(
				project_root,
				&scratch.join("destination"),
				&commit,
			)
			.await?;
			let dirty = merge::tree_of(project_root, &commit).await? != tree;
			Ok(DestinationState {
				commit,
				tree,
				dirty,
			})
		}
		PromotionDestination::Branch(name) => {
			let reference = format!("refs/heads/{name}");
			let Some(commit) = merge::resolve(project_root, &reference).await?
			else {
				return Err(CoreError::not_found(
					"workspace.promotion_branch_not_found",
					"the selected branch does not exist in the Project",
				));
			};
			if merge::is_checked_out(project_root, name).await? {
				return Err(CoreError::conflict(
					"workspace.promotion_branch_checked_out",
					"the selected branch is checked out in a working tree; \
					 promote to the Local checkout instead, or select a branch \
					 no working tree has checked out",
				));
			}
			let tree = merge::tree_of(project_root, &commit).await?;
			Ok(DestinationState {
				commit,
				tree,
				dirty: false,
			})
		}
	}
}

impl From<Change> for PromotedChange {
	fn from(change: Change) -> Self {
		Self {
			kind: if change.is_addition() {
				ChangeKind::Added
			} else if change.is_deletion() {
				ChangeKind::Deleted
			} else {
				ChangeKind::Modified
			},
			path: change.path,
		}
	}
}

fn destination_invalid() -> CoreError {
	CoreError::invalid_input(
		"workspace.promotion_destination_invalid",
		"a branch is one name without whitespace or control characters",
	)
}

pub(crate) fn workspace_not_found() -> CoreError {
	CoreError::not_found("workspace.not_found", "the Workspace does not exist")
}

#[cfg(test)]
#[path = "promotion_tests.rs"]
mod tests;
