//! What a managed Workspace's working tree holds that forgetting would
//! lose: uncommitted changes and commits no remote has (ADR-0001).

use crate::{CoreError, project::repository::git};
use std::{path::Path, time::Duration};

/// How long one inspection may take. A stalled Git stalls one decision,
/// not the Plane.
const INSPECTION_BUDGET: Duration = Duration::from_secs(30);

/// What the working tree of a Workspace holds beyond its base.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceState {
	/// Tracked files changed or untracked files present.
	pub(crate) dirty: bool,
	/// Commits past the base that no remote branch contains.
	pub(crate) unpushed: bool,
}

impl WorkspaceState {
	/// What the Conversation's Workspace holds, or nothing when it has no
	/// managed Workspace: a Local checkout is the user's, not Jet's.
	///
	/// # Errors
	///
	/// As [`WorkspaceState::inspect`].
	pub(crate) async fn of(
		workspace: Option<&jet_store::WorkspaceRecord>,
	) -> Result<Self, CoreError> {
		match workspace {
			Some(workspace) => {
				Self::inspect(
					Path::new(&workspace.root),
					&workspace.base_commit,
				)
				.await
			}
			None => Ok(Self::default()),
		}
	}

	/// Inspects the worktree at `root`, created at `base_commit`. A root
	/// that no longer exists holds nothing to lose and is clean.
	///
	/// # Errors
	///
	/// Returns an `unavailable` `retention.workspace_unreadable` when Git
	/// cannot answer or does not answer in time; the caller treats that
	/// as protected rather than guessing.
	pub(crate) async fn inspect(
		root: &Path,
		base_commit: &str,
	) -> Result<Self, CoreError> {
		if !root.is_dir() {
			return Ok(Self::default());
		}
		let status = run(
			root,
			&["status", "--porcelain=v1", "--untracked-files=normal"],
		)
		.await?;
		let range = format!("{base_commit}..HEAD");
		let ahead = run(root, &["rev-list", "--count", &range]).await?;
		let unpushed = ahead.trim() != "0" && {
			let holders =
				run(root, &["branch", "--remotes", "--contains", "HEAD"])
					.await?;
			holders.trim().is_empty()
		};
		Ok(Self {
			dirty: !status.trim().is_empty(),
			unpushed,
		})
	}
}

async fn run(root: &Path, arguments: &[&str]) -> Result<String, CoreError> {
	let output = tokio::time::timeout(INSPECTION_BUDGET, git(root, arguments))
		.await
		.map_err(|_| unreadable("git did not finish in time".into()))??;
	if !output.status.success() {
		return Err(unreadable(output.stderr));
	}
	Ok(output.stdout)
}

fn unreadable(detail: String) -> CoreError {
	CoreError::unavailable(
		"retention.workspace_unreadable",
		"the Workspace could not be inspected",
		detail.chars().take(512).collect::<String>(),
	)
}
