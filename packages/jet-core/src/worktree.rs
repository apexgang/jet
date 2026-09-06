//! The `git worktree` half of a Workspace (ADR-0025, ADR-0056).
//!
//! Everything here runs the detected `git` the way repository inspection
//! does: argument arrays, no shell, no inherited `GIT_*` variable, no
//! prompt. Resolving a base reads the repository; adding a worktree writes
//! a directory under Jet's own home and one entry in the repository's
//! `.git/worktrees`, and nothing in the Project's own working tree.

use std::path::Path;
use std::time::Duration;

use crate::error::CoreError;
use crate::repository::{Output, git};

/// How long resolving a base or adding a worktree may take. A checkout of
/// a large repository on a slow disk stalls one Command, not the Plane.
const WORKTREE_BUDGET: Duration = Duration::from_secs(120);

/// Longest native Git message kept as local diagnostic detail (ADR-0061).
const MAX_DETAIL_CHARS: usize = 512;

/// Resolves `revision` at `root` to the commit it names right now.
///
/// # Errors
///
/// Returns `workspace.base_not_found` when Git finds no commit by that
/// name, and an `unavailable` `workspace.git_failed` when Git cannot
/// answer.
pub(crate) async fn resolve_commit(
	root: &Path,
	revision: &str,
) -> Result<String, CoreError> {
	let peeled = format!("{revision}^{{commit}}");
	let output = bounded(git(
		root,
		&[
			"rev-parse",
			"--verify",
			"--quiet",
			"--end-of-options",
			&peeled,
		],
	))
	.await?;
	if !output.status.success() {
		return Err(CoreError::not_found(
			"workspace.base_not_found",
			"the selected base names no commit in the Project",
		));
	}
	let commit = output.stdout.trim().to_owned();
	if commit.len() < 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
	{
		return Err(git_failed(output.stdout));
	}
	Ok(commit)
}

/// Adds a worktree of the repository at `root` at `path`, checked out at
/// `commit` with a detached HEAD, so no branch is claimed or moved by the
/// Workspace's existence (ADR-0025).
///
/// # Errors
///
/// Returns an `unavailable` `workspace.git_failed` when Git refuses or
/// does not finish in time.
pub(crate) async fn add_detached(
	root: &Path,
	path: &str,
	commit: &str,
) -> Result<(), CoreError> {
	let output = bounded(git(
		root,
		&[
			"worktree",
			"add",
			"--detach",
			"--end-of-options",
			path,
			commit,
		],
	))
	.await?;
	if !output.status.success() {
		return Err(git_failed(output.stderr));
	}
	Ok(())
}

/// Removes the worktree at `path` from the repository at `root`, whatever
/// it holds. It undoes an [`add_detached`] whose Workspace cannot be
/// finished, so the answer is not needed: a directory it leaves behind is
/// named by no row and collides with no later Conversation.
pub(crate) async fn remove_forced(root: &Path, path: &str) {
	let _ = bounded(git(
		root,
		&["worktree", "remove", "--force", "--end-of-options", path],
	))
	.await;
}

async fn bounded(
	invocation: impl Future<Output = Result<Output, CoreError>>,
) -> Result<Output, CoreError> {
	tokio::time::timeout(WORKTREE_BUDGET, invocation)
		.await
		.map_err(|_| git_failed("git did not finish in time".into()))?
}

/// Git answered, but not with a Workspace. The native text stays local
/// (ADR-0061, ADR-0068).
fn git_failed(detail: String) -> CoreError {
	CoreError::unavailable(
		"workspace.git_failed",
		"the Workspace could not be created",
		detail.chars().take(MAX_DETAIL_CHARS).collect::<String>(),
	)
}
