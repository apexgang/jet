//! Seeding a Workspace from its Project's Local checkout (ADR-0025).
//!
//! A Workspace starts detached at its base commit. It may also start with
//! the changes the user has in the Project's Local checkout: none of them,
//! every eligible one, or the paths the user names. The changes are
//! captured first, as one immutable Git tree in the Project's object
//! store, and applied to the Workspace only after its base is verified to
//! be the commit those changes were made against. A capture never follows
//! a symbolic link, never enters a submodule, and treats a repository
//! nested inside the working tree as an opaque directory (ADR-0103).
//!
//! The capture itself lives in [`crate::seed_capture`]; this module holds
//! what the user selects and what the Workspace remembers.

use jet_store::WorkspaceSeedRecord;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::relative_path::RelativePath;

/// Most paths one selection may name. A control frame holds no larger
/// collection either, so a selection that reaches the core through the
/// protocol is already within it; the bound here keeps the core's own
/// contract independent of the wire's.
pub(crate) const MAX_SELECTED_PATHS: usize = 4096;

/// Which Local-checkout changes a new Workspace starts with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub enum SeedSelection {
	/// No changes: the Workspace starts at its base alone.
	#[default]
	None,
	/// Every eligible change: modifications and deletions of tracked paths,
	/// untracked paths that are not ignored, and the commit each submodule
	/// has checked out. Ignored paths are left out, and so is a repository
	/// nested inside the working tree.
	AllEligible,
	/// The named paths and whatever they hold, each taken as the Local
	/// checkout has it. A path that is ignored is included because it was
	/// named; a directory that was named brings its unignored content and
	/// leaves its ignored content behind.
	Paths(Vec<RelativePath>),
}

impl SeedSelection {
	/// Whether the selection asks for nothing, so no capture is needed.
	#[must_use]
	pub fn is_none(&self) -> bool {
		matches!(self, Self::None)
	}

	/// Refuses a selection the core will not stage, before anything
	/// reaches Git.
	///
	/// # Errors
	///
	/// Returns an `invalid_input` `workspace.seed_too_many_paths` when the
	/// selection names more paths than [`MAX_SELECTED_PATHS`], or
	/// `workspace.seed_no_paths` when it names none.
	pub(crate) fn validate(&self) -> Result<(), CoreError> {
		match self {
			Self::None | Self::AllEligible => Ok(()),
			Self::Paths(paths) if paths.is_empty() => {
				Err(CoreError::invalid_input(
					"workspace.seed_no_paths",
					"a selection of paths names at least one; select no \
					 changes to seed nothing",
				))
			}
			Self::Paths(paths) if paths.len() > MAX_SELECTED_PATHS => {
				Err(CoreError::invalid_input(
					"workspace.seed_too_many_paths",
					format!(
						"a selection names at most {MAX_SELECTED_PATHS} paths; \
						 select all eligible changes to bring more"
					),
				))
			}
			Self::Paths(_) => Ok(()),
		}
	}
}

/// What a Workspace was seeded with: the immutable tree its Local-checkout
/// changes were captured as, and how many paths that tree changes against
/// the base commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSeed {
	/// The tree object, as Git spells it. It lives in the Project's object
	/// store beside the base commit and names exactly the content applied.
	pub tree: String,
	/// How many paths the tree changes against the base.
	pub changed_paths: u32,
}

impl From<WorkspaceSeedRecord> for WorkspaceSeed {
	fn from(record: WorkspaceSeedRecord) -> Self {
		Self {
			tree: record.tree,
			changed_paths: record.changed_paths,
		}
	}
}

impl From<&WorkspaceSeed> for WorkspaceSeedRecord {
	fn from(seed: &WorkspaceSeed) -> Self {
		Self {
			tree: seed.tree.clone(),
			changed_paths: seed.changed_paths,
		}
	}
}

#[cfg(test)]
#[path = "seed_tests.rs"]
mod tests;
