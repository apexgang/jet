//! Where a removed Project's directory goes: the system Trash, or nowhere
//! (ADR-0011).

use super::{Disposition, ProjectDisposal};
use crate::{CoreError, filesystem::blocking};
use std::path::{Path, PathBuf};

/// Disposes of the directory at `root` as `disposal` asks. Nothing is
/// removed when it fails. A root that is already gone, removed by hand or
/// by an earlier attempt the Plane could not commit, needs nothing and
/// counts as deleted, so its registration can still be removed.
///
/// # Errors
///
/// Returns an `unavailable` `project.trash_unavailable` when the Plane has
/// no system Trash for the directory, so the client can ask for a
/// permanent removal instead, and an `unavailable`
/// `project.disposal_failed` when a permanent removal could not finish.
pub(super) async fn dispose(
	root: &Path,
	disposal: ProjectDisposal,
) -> Result<Disposition, CoreError> {
	let root = root.to_path_buf();
	if !root.exists() {
		return Ok(Disposition::Deleted);
	}
	match disposal {
		ProjectDisposal::SystemTrash => {
			blocking(move || trash::delete(&root))
				.await?
				.map_err(|error| {
					CoreError::unavailable(
						"project.trash_unavailable",
						"this Plane has no system Trash for the directory; \
						 acknowledge the permanent-deletion warning to remove \
						 it for good",
						error.to_string(),
					)
				})?;
			Ok(Disposition::Trashed)
		}
		ProjectDisposal::Permanent(_) => {
			blocking(move || std::fs::remove_dir_all(&root))
				.await?
				.map_err(|error| {
					CoreError::unavailable(
						"project.disposal_failed",
						"the directory could not be deleted",
						error.to_string(),
					)
				})?;
			Ok(Disposition::Deleted)
		}
	}
}

/// Removes the directories of the Workspaces that were created from the
/// removed Project. The repository they were worktrees of is gone, so
/// there is nothing to tell it; a failure leaves a directory nothing
/// refers to, which is what it leaves behind a deleted Conversation too.
pub(super) async fn remove_workspaces(roots: Vec<PathBuf>) {
	for root in roots {
		let _ = tokio::fs::remove_dir_all(&root).await;
	}
}
