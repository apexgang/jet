//! Removing a registered Project from the Plane and from disk
//! (ADR-0011, ADR-0101, ADR-0102, ADR-0105).
//!
//! Removal is the one Command that destroys a directory the user owns,
//! so it is made in two phases by an interactive user and by nobody
//! else. A preview reads what the removal would meet: the canonical
//! root, its disk use, the live Runs and schedules that refuse it, the
//! uncommitted files and unpushed commits it would lose, and the
//! Workspaces that go with it. The Command carries that preview back as
//! a binding, with the Project's directory name typed out; the Plane
//! computes the preview again and refuses a binding anything has moved
//! past. The directory goes to the system Trash, from where the user can
//! bring it back; where there is none, deleting it for good takes a
//! second acknowledgement.
//!
//! No Harness, Craft, Scheduled task, Utility, or Autodelete rule can
//! reach this Command: it exists only in the client protocol, and only
//! interactive Actors are admitted to it.

mod command;
mod disposal;
mod inspection;

pub(crate) use command::{PreparedRemoval, prepare, remove};
pub(crate) use inspection::preview;

use crate::{ClientId, Core, CoreError, EventSequence, ProjectId, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

impl Core {
	/// Jet's own home, the directory that holds the store, the Crafts,
	/// and every Workspace. It is the parent of the Workspace home, the
	/// same way the Craft home is found at start.
	pub(crate) fn jet_home(&self) -> PathBuf {
		self.workspace_home
			.0
			.parent()
			.expect("Workspace home has a parent")
			.to_path_buf()
	}
}

/// What a preview showed and a removal carries back: the Project as it
/// stood, the work it would lose, and the Actor it was shown to. The
/// removal is refused when any of it has changed since (ADR-0011).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRemovalBinding {
	/// The Project being removed.
	pub project_id: ProjectId,
	/// Its canonical root, the directory the removal takes.
	pub root: PathBuf,
	/// Runs of its Conversations that have not ended. Any refuses the
	/// removal.
	pub live_runs: u64,
	/// Scheduled tasks of its Conversations. Any refuses the removal.
	pub schedules: u64,
	/// Tracked files changed and untracked files present in its checkout,
	/// which the removal loses.
	pub dirty_files: u64,
	/// Commits in its repository that no remote branch holds, which the
	/// removal loses with it.
	pub unpushed_commits: u64,
	/// The Workspaces created from it, which are removed with it.
	pub workspaces: Vec<WorkspaceId>,
	/// The Client identity the preview was shown to.
	pub actor: ClientId,
}

/// One reason a Project cannot be removed as it stands. What keeps it is
/// answered as data, so a GUI acts on it without parsing a message
/// (ADR-0068).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalObstacle {
	/// A Run of one of its Conversations has not ended.
	LiveRuns,
	/// A Scheduled task of one of its Conversations would queue more work.
	Schedules,
	/// Its root is a filesystem root.
	FilesystemRoot,
	/// Its root is the user's home directory.
	UserHome,
	/// Its root is Jet's own home, or holds it, or lies inside it.
	JetHome,
	/// Another registered Project lies inside its root.
	ContainsProject,
}

/// What removing one Project would meet and lose, shown before it is
/// done (ADR-0011). Fenced by the journal position it was read at
/// (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRemovalPreview {
	/// Newest Event sequence visible when the Project was read.
	pub cursor: EventSequence,
	/// What the removal is bound to.
	pub binding: ProjectRemovalBinding,
	/// Bytes the files under the root occupy, symbolic links not followed.
	/// It is disclosed and not bound: a repository's size moves on its own.
	pub disk_use_bytes: u64,
	/// What refuses the removal today, in a fixed order; empty when it may
	/// proceed.
	pub obstacles: Vec<RemovalObstacle>,
	/// The warning a permanent removal acknowledges, word for word, so a
	/// client shows what the Plane expects back.
	pub permanent_removal_warning: &'static str,
}

/// The acknowledgement a permanent removal carries, word for word.
pub const PERMANENT_REMOVAL_WARNING: &str = "This Plane has no system Trash \
	for this directory. Removing the Project deletes its directory, its \
	uncommitted files, and its unpushed commits permanently, and nothing \
	in Jet can bring them back. I authorize this permanent deletion.";

/// The second authorization a permanent removal needs: the warning above,
/// acknowledged word for word (ADR-0011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PermanentRemoval(());

impl PermanentRemoval {
	/// Accepts only the exact warning.
	///
	/// # Errors
	///
	/// Returns an `invalid_input` `project.warning_unacknowledged` when the
	/// acknowledgement is anything else.
	pub fn acknowledging(warning: &str) -> Result<Self, CoreError> {
		if warning != PERMANENT_REMOVAL_WARNING {
			return Err(CoreError::invalid_input(
				"project.warning_unacknowledged",
				"a permanent removal acknowledges the permanent-deletion \
				 warning word for word",
			));
		}
		Ok(Self(()))
	}
}

/// Where the Project's directory goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ProjectDisposal {
	/// To the system Trash, from where the user can restore it. Refused
	/// with `project.trash_unavailable` where the Plane has none for the
	/// directory, and nothing is removed.
	SystemTrash,
	/// Deleted for good, under the acknowledged warning.
	Permanent(PermanentRemoval),
}

/// Where the Project's directory went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
	/// It is in the system Trash.
	Trashed,
	/// It is deleted.
	Deleted,
}

/// The Project as removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRemoved {
	/// The Project that is no longer registered.
	pub project_id: ProjectId,
	/// The root its directory was at.
	pub root: PathBuf,
	/// Where the directory went.
	pub disposition: Disposition,
}
