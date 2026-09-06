//! The Git half of a Workspace promotion preview (ADR-0025).
//!
//! Everything here reads. The Workspace and the destination are each
//! captured as a tree through a scratch index, the two trees are merged
//! against the Workspace's base with `git merge-tree`, which writes the
//! result as one more tree and names the paths it could not merge, and
//! the result is compared with the destination to list what promotion
//! would change. No working tree, index, or reference is touched.

use std::path::Path;

use crate::error::CoreError;
use crate::filesystem::blocking;
use crate::repository::git;
use crate::tree_capture::{Change, ScratchIndex, diff_trees, object_name};

/// Longest native Git message kept as local diagnostic detail (ADR-0061).
const MAX_DETAIL_CHARS: usize = 512;

/// The result of merging a Workspace and a destination against the base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Merged {
	/// The merged tree. Where the merge conflicted, the tree holds Git's
	/// conflict markers and is shown but never applied.
	pub(crate) tree: String,
	/// The paths the merge could not settle, as Git names them.
	pub(crate) conflicts: Vec<String>,
}

/// Captures the working tree at `root`, staged and unstaged changes alike
/// and a nested repository dropped, as one tree through a scratch index
/// under `scratch`.
pub(crate) async fn snapshot(
	root: &Path,
	scratch: &Path,
	head: &str,
) -> Result<String, CoreError> {
	let index = ScratchIndex::new(root, scratch, promotion_failed);
	index.copy_from_checkout().await?;
	let (tree, _) = index.capture_everything(head).await?;
	Ok(tree)
}

/// The commit `revision` names at `root` right now, if it names one.
pub(crate) async fn resolve(
	root: &Path,
	revision: &str,
) -> Result<Option<String>, CoreError> {
	let peeled = format!("{revision}^{{commit}}");
	let output = git(
		root,
		&[
			"rev-parse",
			"--verify",
			"--quiet",
			"--end-of-options",
			&peeled,
		],
	)
	.await?;
	if !output.status.success() {
		return Ok(None);
	}
	object_name(output.stdout, promotion_failed).map(Some)
}

/// The tree of `commit`.
pub(crate) async fn tree_of(
	root: &Path,
	commit: &str,
) -> Result<String, CoreError> {
	let peeled = format!("{commit}^{{tree}}");
	let output = git(
		root,
		&[
			"rev-parse",
			"--verify",
			"--quiet",
			"--end-of-options",
			&peeled,
		],
	)
	.await?;
	if !output.status.success() {
		return Err(promotion_failed(output.stderr));
	}
	object_name(output.stdout, promotion_failed)
}

/// Whether some working tree of the repository at `root` has `branch`
/// checked out. Updating such a branch behind its checkout would leave
/// the checkout's index and files describing the reverse of the change.
pub(crate) async fn is_checked_out(
	root: &Path,
	branch: &str,
) -> Result<bool, CoreError> {
	let listed = git(root, &["worktree", "list", "--porcelain"]).await?;
	if !listed.status.success() {
		return Err(promotion_failed(listed.stderr));
	}
	let reference = format!("branch refs/heads/{branch}");
	Ok(listed.stdout.lines().any(|line| line == reference))
}

/// Merges `theirs`, the Workspace's tree, into `ours`, the destination's,
/// with `base` as the common ancestor, writing the result as a tree and
/// naming the conflicts, without touching any working tree.
pub(crate) async fn merge(
	root: &Path,
	base: &str,
	ours: &str,
	theirs: &str,
) -> Result<Merged, CoreError> {
	let merge_base = format!("--merge-base={base}");
	let output = git(
		root,
		&[
			"merge-tree",
			"--write-tree",
			"-z",
			"--name-only",
			"--no-messages",
			&merge_base,
			"--end-of-options",
			ours,
			theirs,
		],
	)
	.await?;
	// Zero means a clean merge and one a conflicted one; anything else is
	// Git failing to merge at all.
	match output.status.code() {
		Some(0 | 1) => {}
		_ => return Err(promotion_failed(output.stderr)),
	}
	parse_merge(&output.stdout)
}

/// Parses `merge-tree --write-tree -z --name-only` output: the merged
/// tree, then each conflicted path, each ended by NUL, then an empty
/// record before the informational messages that were asked to be
/// withheld.
fn parse_merge(printed: &str) -> Result<Merged, CoreError> {
	let mut records = printed.split('\0');
	let tree = object_name(
		records.next().unwrap_or_default().to_owned(),
		promotion_failed,
	)?;
	let conflicts = records
		.take_while(|record| !record.is_empty())
		.map(str::to_owned)
		.collect();
	Ok(Merged { tree, conflicts })
}

/// What `result` changes against `destination`, one record per path.
pub(crate) async fn changes(
	root: &Path,
	destination: &str,
	result: &str,
) -> Result<Vec<Change>, CoreError> {
	diff_trees(root, destination, result, promotion_failed).await
}

/// Which of `paths` the working tree at `root` already holds something
/// at, tracked or not, so a promotion that would create them is known to
/// write over what is there.
pub(crate) async fn occupied(
	root: &Path,
	paths: Vec<String>,
) -> Result<Vec<String>, CoreError> {
	let root = root.to_path_buf();
	blocking(move || {
		paths
			.into_iter()
			.filter(|path| std::fs::symlink_metadata(root.join(path)).is_ok())
			.collect()
	})
	.await
}

/// Git answered, but not with a preview. The native text stays local
/// (ADR-0061, ADR-0068).
pub(crate) fn promotion_failed(detail: String) -> CoreError {
	CoreError::unavailable(
		"workspace.promotion_failed",
		"the Workspace could not be compared with the destination",
		detail.chars().take(MAX_DETAIL_CHARS).collect::<String>(),
	)
}
