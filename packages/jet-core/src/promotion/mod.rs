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

mod encoding;

pub(crate) mod apply;
pub(crate) mod command;
pub(crate) mod effect;
pub(crate) mod merge;

use crate::{
	Actor, ClientId, Core,
	error::CoreError,
	event::EventSequence,
	query::QueryResult,
	workspace::{self, WorkspaceHome, WorkspaceId, tree_capture::diff_trees},
};
use jet_store::{ReadTransaction, WorkspaceRecord};
use serde::{Deserialize, Serialize};
use std::{
	path::{Path, PathBuf},
	time::SystemTime,
};
use uuid::Uuid;

/// Durable identity of one promotion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PromotionId(pub Uuid);

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
/// destination as they stood, the result and the risk the user looked
/// at, and the Actor it was shown to. A promotion is refused when any of
/// it has changed since (ADR-0025).
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
	/// Whether the destination holds uncommitted changes of its own, which
	/// the promotion is merged over and never discards.
	pub destination_dirty: bool,
	/// The paths the promotion cannot settle. A preview with any is shown
	/// and never applied.
	pub conflicts: Vec<PromotionConflict>,
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
	/// What the promotion is bound to, risk included.
	pub binding: PromotionBinding,
	/// How many paths the promotion changes in the destination.
	pub changed_paths: u32,
	/// The changes, up to [`MAX_PREVIEW_CHANGES`] of them, in Git's order.
	pub changes: Vec<PromotedChange>,
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
	/// The destination's index holds a version of the path that differs
	/// from its file and from HEAD alike. The merge saw the file alone,
	/// and applying it would replace the staged version unseen.
	Staged,
}

/// Where a promotion stands (ADR-0025, ADR-0067).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromotionState {
	/// Recorded, with the Effect that applies it not yet settled.
	Applying,
	/// Applied, and the destination verified to hold the result.
	Promoted,
	/// Never applied: the preview could not settle every path, and the
	/// paths are kept with the promotion for the user to resolve in the
	/// Workspace.
	Conflicted,
	/// Its Effect reported a definite failure before changing anything;
	/// the destination is as it was.
	Failed,
	/// Its Effect's outcome could not be established. Jet neither repeats
	/// it nor calls it failed; the destination is the user's to inspect.
	OutcomeUnknown,
}

/// One recorded promotion of a Workspace: what the user confirmed and
/// where it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePromotion {
	/// Durable identity.
	pub promotion_id: PromotionId,
	/// Exactly what the preview bound and the user confirmed.
	pub binding: PromotionBinding,
	/// How many paths the result changes in the destination.
	pub changed_paths: u32,
	/// Where the promotion stands.
	pub state: PromotionState,
	/// When it was recorded.
	pub recorded_at: SystemTime,
	/// When it reached a settled state, if it has.
	pub settled_at: Option<SystemTime>,
}

/// The Workspace and destination compared as they are right now: a
/// preview before the journal fence is added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Compared {
	pub(crate) binding: PromotionBinding,
	pub(crate) changed_paths: u32,
	pub(crate) changes: Vec<PromotedChange>,
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
/// `workspace.promotion_branch_not_found`,
/// `workspace.promotion_branch_checked_out`, or
/// `workspace.promotion_identity_missing` when the destination cannot be
/// promoted to, and an `unavailable` `workspace.promotion_failed` when
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
			let (workspace, project_root) =
				workspace_and_project(tx, workspace_id).await?;
			Ok::<_, CoreError>((cursor, workspace, project_root))
		})
		.await?;
	let Compared {
		binding,
		changed_paths,
		changes,
	} = compare(
		&core.workspace_home,
		actor,
		&workspace,
		&project_root,
		destination,
	)
	.await?;
	Ok(QueryResult::PromotionPreview(Box::new(PromotionPreview {
		cursor,
		binding,
		changed_paths,
		changes,
	})))
}

/// The Workspace and the root of its Project, as the store has them.
///
/// # Errors
///
/// Returns `workspace.not_found` or `project.not_found`.
pub(crate) async fn workspace_and_project(
	tx: &mut ReadTransaction,
	workspace_id: WorkspaceId,
) -> Result<(WorkspaceRecord, PathBuf), CoreError> {
	let Some(workspace) = tx.workspace(workspace_id.0).await? else {
		return Err(workspace_not_found());
	};
	let Some(project) = tx.project(workspace.project_id).await? else {
		return Err(CoreError::not_found(
			"project.not_found",
			"the Project is not registered",
		));
	};
	Ok((workspace, PathBuf::from(project.root)))
}

/// Compares the Workspace with the destination as the repository is
/// right now, outside any store lock.
pub(crate) async fn compare(
	home: &WorkspaceHome,
	actor: &Actor,
	workspace: &WorkspaceRecord,
	project_root: &Path,
	destination: PromotionDestination,
) -> Result<Compared, CoreError> {
	tokio::time::timeout(
		PREVIEW_BUDGET,
		workspace::with_scratch(home, "promotion", async |scratch| {
			compare_in(scratch, actor, workspace, project_root, destination)
				.await
		}),
	)
	.await
	.map_err(|_| {
		merge::promotion_failed("the preview did not finish in time".into())
	})?
}

async fn compare_in(
	scratch: &Path,
	actor: &Actor,
	workspace: &WorkspaceRecord,
	project_root: &Path,
	destination: PromotionDestination,
) -> Result<Compared, CoreError> {
	let workspace_root = Path::new(&workspace.root);
	let Some(workspace_head) = merge::resolve(workspace_root, "HEAD").await?
	else {
		return Err(merge::promotion_failed(
			"the Workspace has no commit checked out".into(),
		));
	};
	let workspace_tree = merge::capture(
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
	let changed = diff_trees(
		project_root,
		&destination_tree,
		&merged.tree,
		merge::promotion_failed,
	)
	.await?;
	let mut conflicts: Vec<PromotionConflict> = merged
		.conflicts
		.into_iter()
		.map(|path| PromotionConflict {
			path,
			kind: ConflictKind::Diverged,
		})
		.collect();
	if destination == PromotionDestination::LocalCheckout {
		conflicts.extend(merge::collisions(project_root, &changed).await?);
	}
	Ok(Compared {
		binding: PromotionBinding {
			workspace_id: WorkspaceId(workspace.workspace_id),
			destination,
			base_commit: workspace.base_commit.clone(),
			workspace_tree,
			destination_commit,
			destination_tree,
			result_tree: merged.tree,
			destination_dirty,
			conflicts,
			actor: actor.client_id(),
		},
		changed_paths: u32::try_from(changed.len()).unwrap_or(u32::MAX),
		changes: changed
			.into_iter()
			.take(MAX_PREVIEW_CHANGES)
			.map(PromotedChange::from)
			.collect(),
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
			let tree = merge::capture(
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
			if !merge::has_identity(project_root).await? {
				return Err(CoreError::unavailable(
					"workspace.promotion_identity_missing",
					"Git on this Plane has no identity to commit as; configure \
					 user.name and user.email for the Project, or promote to \
					 the Local checkout instead",
					"git var GIT_COMMITTER_IDENT failed",
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
mod tests {
	use std::path::Path;

	use pretty_assertions::assert_eq;

	use crate::test_support::{
		Diverged, diverged, git, preview_promotion as preview, status,
	};
	use crate::{
		ChangeKind, ClientId, ConflictKind, ErrorCategory, EventSequence,
		PromotedChange, PromotionBinding, PromotionConflict,
		PromotionDestination, PromotionPreview, WorkspaceId,
	};

	/// What `tree` holds at `path`, or nothing.
	fn content(root: &Path, tree: &str, path: &str) -> Option<String> {
		let output = std::process::Command::new("git")
			.arg("-C")
			.arg(root)
			.args(["show", &format!("{tree}:{path}")])
			.output()
			.unwrap();
		output
			.status
			.success()
			.then(|| String::from_utf8(output.stdout).unwrap())
	}

	fn change(path: &str, kind: ChangeKind) -> PromotedChange {
		PromotedChange {
			path: path.into(),
			kind,
		}
	}

	/// A preview merges the Workspace's changes over the Local checkout's own
	/// against the Workspace base, lists exactly what would change, binds
	/// what it compared and whom it was shown to, and leaves both working
	/// trees exactly as they were. Changes the Workspace was seeded with are
	/// on both sides and change nothing (ADR-0025).
	#[tokio::test]
	async fn a_preview_merges_the_workspace_over_the_checkout_without_touching_it()
	 {
		let dir = tempfile::tempdir().unwrap();
		let Diverged {
			core,
			repository,
			base,
			workspace,
		} = diverged(dir.path()).await;
		let checkout_before = status(&repository);
		let workspace_before = status(&workspace.root);

		let previewed = preview(
			&core,
			workspace.workspace_id,
			PromotionDestination::LocalCheckout,
		)
		.await
		.unwrap();

		let binding = previewed.binding.clone();
		assert_eq!(
			(
				&previewed,
				content(&repository, &binding.result_tree, "f.txt"),
				content(&repository, &binding.result_tree, "new.txt"),
				content(&repository, &binding.result_tree, "k.txt"),
				content(&repository, &binding.result_tree, "o.txt"),
				content(&repository, &binding.result_tree, "notes.txt"),
				content(&repository, &binding.workspace_tree, "notes.txt"),
				content(&repository, &binding.destination_tree, "f.txt"),
				status(&repository),
				status(&workspace.root),
			),
			(
				&PromotionPreview {
					cursor: previewed.cursor,
					binding: PromotionBinding {
						workspace_id: workspace.workspace_id,
						destination: PromotionDestination::LocalCheckout,
						base_commit: base.clone(),
						workspace_tree: binding.workspace_tree.clone(),
						destination_commit: base.clone(),
						destination_tree: binding.destination_tree.clone(),
						result_tree: binding.result_tree.clone(),
						destination_dirty: true,
						conflicts: vec![],
						actor: ClientId(uuid::Uuid::nil()),
					},
					changed_paths: 3,
					changes: vec![
						change("f.txt", ChangeKind::Modified),
						change("k.txt", ChangeKind::Deleted),
						change("new.txt", ChangeKind::Added),
					],
				},
				Some("A\nb\nC\n".into()),
				Some("new\n".into()),
				None,
				Some("other\n".into()),
				Some("draft\n".into()),
				Some("draft\n".into()),
				Some("a\nb\nC\n".into()),
				checkout_before,
				workspace_before,
			)
		);
	}

	/// A path both sides changed in ways Git cannot combine, a path both
	/// sides added with different content, a path the Workspace adds where
	/// the checkout holds an ignored file, and a changed path whose staged
	/// version differs from its file are named as conflicts rather than
	/// settled (ADR-0025).
	#[tokio::test]
	async fn a_preview_names_what_it_cannot_settle() {
		let dir = tempfile::tempdir().unwrap();
		let Diverged {
			core,
			repository,
			workspace,
			..
		} = diverged(dir.path()).await;
		std::fs::write(repository.join("f.txt"), "X\nb\nC\n").unwrap();
		std::fs::write(repository.join("new.txt"), "mine\n").unwrap();
		std::fs::write(repository.join(".gitignore"), "local.txt\n").unwrap();
		std::fs::write(repository.join("local.txt"), "mine\n").unwrap();
		std::fs::write(workspace.root.join("local.txt"), "theirs\n").unwrap();
		std::fs::write(repository.join("f.txt"), "staged\n").unwrap();
		git(&repository, &["add", "f.txt"]);
		std::fs::write(repository.join("f.txt"), "X\nb\nC\n").unwrap();

		let previewed = preview(
			&core,
			workspace.workspace_id,
			PromotionDestination::LocalCheckout,
		)
		.await
		.unwrap();

		assert_eq!(
			(previewed.binding.conflicts, previewed.changed_paths),
			(
				vec![
					PromotionConflict {
						path: "f.txt".into(),
						kind: ConflictKind::Diverged,
					},
					PromotionConflict {
						path: "new.txt".into(),
						kind: ConflictKind::Diverged,
					},
					PromotionConflict {
						path: "local.txt".into(),
						kind: ConflictKind::Untracked,
					},
					PromotionConflict {
						path: "f.txt".into(),
						kind: ConflictKind::Staged,
					},
				],
				4,
			)
		);
	}

	/// A branch no working tree has checked out is previewed against its
	/// tip; the checked-out branch, a missing one, a malformed name, a
	/// Plane whose Git has no identity to commit as, and an unknown
	/// Workspace are refused with stable errors (ADR-0025).
	#[tokio::test]
	async fn a_preview_targets_a_branch_no_working_tree_has_checked_out() {
		let dir = tempfile::tempdir().unwrap();
		let Diverged {
			core,
			repository,
			base,
			workspace,
		} = diverged(dir.path()).await;
		git(&repository, &["branch", "release", &base]);
		git(&repository, &["config", "user.name", ""]);
		let anonymous = preview(
			&core,
			workspace.workspace_id,
			PromotionDestination::Branch("release".into()),
		)
		.await
		.unwrap_err();
		git(&repository, &["config", "user.name", "Jet"]);
		let mut refusals = vec![(anonymous.category, anonymous.code)];
		for (workspace_id, destination) in [
			(
				workspace.workspace_id,
				PromotionDestination::Branch("main".into()),
			),
			(
				workspace.workspace_id,
				PromotionDestination::Branch("nowhere".into()),
			),
			(
				workspace.workspace_id,
				PromotionDestination::Branch("-bad".into()),
			),
			(
				WorkspaceId(uuid::Uuid::nil()),
				PromotionDestination::LocalCheckout,
			),
		] {
			let refused =
				preview(&core, workspace_id, destination).await.unwrap_err();
			refusals.push((refused.category, refused.code));
		}

		let previewed = preview(
			&core,
			workspace.workspace_id,
			PromotionDestination::Branch("release".into()),
		)
		.await
		.unwrap();

		let binding = previewed.binding.clone();
		assert_eq!(
			(
				&previewed,
				content(&repository, &binding.result_tree, "f.txt"),
				content(&repository, &binding.result_tree, "notes.txt"),
				refusals,
			),
			(
				&PromotionPreview {
					cursor: EventSequence(previewed.cursor.0),
					binding: PromotionBinding {
						workspace_id: workspace.workspace_id,
						destination: PromotionDestination::Branch(
							"release".into()
						),
						base_commit: base.clone(),
						workspace_tree: binding.workspace_tree.clone(),
						destination_commit: base.clone(),
						destination_tree: git(
							&repository,
							&["rev-parse", "HEAD^{tree}"]
						)
						.trim()
						.to_owned(),
						result_tree: binding.result_tree.clone(),
						destination_dirty: false,
						conflicts: vec![],
						actor: ClientId(uuid::Uuid::nil()),
					},
					changed_paths: 4,
					changes: vec![
						change("f.txt", ChangeKind::Modified),
						change("k.txt", ChangeKind::Deleted),
						change("new.txt", ChangeKind::Added),
						change("notes.txt", ChangeKind::Added),
					],
				},
				Some("A\nb\nc\n".into()),
				Some("draft\n".into()),
				vec![
					(
						ErrorCategory::Unavailable,
						"workspace.promotion_identity_missing".into()
					),
					(
						ErrorCategory::Conflict,
						"workspace.promotion_branch_checked_out".into()
					),
					(
						ErrorCategory::NotFound,
						"workspace.promotion_branch_not_found".into()
					),
					(
						ErrorCategory::InvalidInput,
						"workspace.promotion_destination_invalid".into()
					),
					(ErrorCategory::NotFound, "workspace.not_found".into()),
				],
			)
		);
	}

	/// A file changed to content of the same size in the same second as the
	/// checkout's index was last written is captured as it is: the scratch
	/// copy of the index keeps the index file's own time, so Git still
	/// distrusts the entry's recorded stat data and reads the file
	/// (ADR-0025). The second is staged by hand: the index and the file are
	/// given one past time, and the repository is told not to trust change
	/// times, which a write moves and a test cannot set.
	#[tokio::test]
	async fn a_change_made_as_the_index_was_written_is_still_seen() {
		let dir = tempfile::tempdir().unwrap();
		let Diverged {
			core,
			repository,
			workspace,
			..
		} = diverged(dir.path()).await;
		git(&repository, &["config", "core.trustctime", "false"]);
		let file = repository.join("f.txt");
		let index = repository.join(".git").join("index");
		let past =
			std::time::SystemTime::now() - std::time::Duration::from_secs(600);
		let stamp = |path: &Path| {
			std::fs::File::options()
				.write(true)
				.open(path)
				.unwrap()
				.set_times(std::fs::FileTimes::new().set_modified(past))
				.unwrap();
		};
		stamp(&file);
		git(&repository, &["add", "f.txt"]);
		std::fs::write(&file, "a\nb\nX\n").unwrap();
		stamp(&file);
		stamp(&index);

		let previewed = preview(
			&core,
			workspace.workspace_id,
			PromotionDestination::LocalCheckout,
		)
		.await
		.unwrap();

		assert_eq!(
			content(&repository, &previewed.binding.destination_tree, "f.txt"),
			Some("a\nb\nX\n".into())
		);
	}
}
