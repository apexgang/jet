//! Capturing Local-checkout changes as a Git tree and applying that tree
//! to a fresh Workspace (ADR-0025, ADR-0103).
//!
//! The capture copies the Local checkout's index to a scratch file and
//! stages the selected changes into the copy, so what the user has staged
//! stays exactly as it is and a sparse checkout keeps the paths it left
//! out. A capture of everything starts from the copy as it is, staged
//! changes included; a capture of named paths first reads HEAD back over
//! the copy, keeping its sparse bits, so a change staged elsewhere does not
//! come along uninvited. Writing the copy as a tree gives an immutable,
//! content-addressed capture in the Project's own object store, which the
//! Workspace shares.
//!
//! Git records a symbolic link as the link itself and a submodule as the
//! commit it has checked out, so neither is followed or entered. A
//! repository nested inside the working tree would become a Git link too,
//! and is not: it is dropped from a capture of everything and refused
//! when named. One nested where the base already tracks files is staged as
//! those files, as Git does, and its own `.git` is never read (ADR-0103).
//!
//! Applying reads the tree over the Workspace's own HEAD after checking
//! that HEAD is the commit the changes were made against. The changes
//! arrive staged, because a Git link change has no unstaged form.

use std::path::Path;
use std::time::Duration;

use crate::error::CoreError;
use crate::relative_path::RelativePath;
use crate::repository::{Output, git};
use crate::seed::SeedSelection;
use crate::tree_capture::{ScratchIndex, diff_trees};
use crate::worktree;

/// How long one capture or one application may take. Hashing a large
/// working tree on a slow disk stalls one Command, not the Plane.
const SEED_BUDGET: Duration = Duration::from_secs(300);

/// Longest native Git message kept as local diagnostic detail (ADR-0061).
const MAX_DETAIL_CHARS: usize = 512;

/// Local-checkout changes captured as one tree, with the commit they were
/// made against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapturedSeed {
	/// The commit the Local checkout had checked out when the changes were
	/// captured; the Workspace must be at it for the tree to mean the same.
	pub(crate) head: String,
	/// The tree object the changes were captured as.
	pub(crate) tree: String,
	/// How many paths the tree changes against `head`.
	pub(crate) changed_paths: u32,
}

/// Captures `selection` from the Local checkout at `root` into a tree,
/// staging through a scratch index under `scratch`, an empty directory
/// the caller owns.
///
/// # Errors
///
/// Returns a `conflict` `workspace.seed_base_mismatch` when the checkout
/// has a commit other than `base_commit` checked out; an `invalid_input`
/// `workspace.seed_unsupported` or a `not_found`
/// `workspace.seed_path_not_found` when a named path cannot be taken; and
/// an `unavailable` `workspace.seed_failed` when Git cannot capture or
/// does not finish in time.
pub(crate) async fn capture(
	root: &Path,
	scratch: &Path,
	selection: &SeedSelection,
	base_commit: &str,
) -> Result<CapturedSeed, CoreError> {
	tokio::time::timeout(
		SEED_BUDGET,
		capture_unbounded(root, scratch, selection, base_commit),
	)
	.await
	.map_err(|_| seed_failed("the capture did not finish in time".into()))?
}

async fn capture_unbounded(
	root: &Path,
	scratch: &Path,
	selection: &SeedSelection,
	base_commit: &str,
) -> Result<CapturedSeed, CoreError> {
	if selection.is_none() {
		return Err(CoreError::internal(
			"workspace.seed_unselected",
			"a capture was asked for with nothing selected",
		));
	}
	let head = worktree::resolve_commit(root, "HEAD").await?;
	if head != base_commit {
		return Err(base_mismatch());
	}
	let index_path = scratch.join("index");
	let index = ScratchIndex::new(root, &index_path, seed_failed);
	index.copy_from_checkout().await?;
	let (tree, changed) = match selection {
		SeedSelection::None | SeedSelection::AllEligible => {
			index.capture_everything(&head).await?
		}
		SeedSelection::Paths(paths) => {
			stage_paths(root, &index, paths).await?;
			let tree = index.write_tree().await?;
			let changed = diff_trees(root, &head, &tree, seed_failed).await?;
			if let Some(nested) =
				changed.iter().find(|change| change.is_nested_repository())
			{
				return Err(nested_repository(&nested.path));
			}
			(tree, changed)
		}
	};
	Ok(CapturedSeed {
		head,
		tree,
		changed_paths: u32::try_from(changed.len()).unwrap_or(u32::MAX),
	})
}

/// Applies `captured` to the Workspace at `root`, which must be clean and
/// at the commit the captured was captured against.
///
/// # Errors
///
/// Returns an `unavailable` `workspace.seed_failed` when the Workspace is
/// not at that commit or Git cannot read the tree in. The Workspace is
/// then not one the caller keeps.
pub(crate) async fn apply(
	root: &Path,
	captured: &CapturedSeed,
) -> Result<(), CoreError> {
	tokio::time::timeout(SEED_BUDGET, apply_unbounded(root, captured))
		.await
		.map_err(|_| {
			seed_failed("the application did not finish in time".into())
		})?
}

async fn apply_unbounded(
	root: &Path,
	captured: &CapturedSeed,
) -> Result<(), CoreError> {
	let head = worktree::resolve_commit(root, "HEAD").await?;
	if head != captured.head {
		return Err(seed_failed(format!(
			"the Workspace is at {head}, not at {} as captured",
			captured.head
		)));
	}
	let read =
		git(root, &["read-tree", "-m", "-u", "HEAD", &captured.tree]).await?;
	if !read.status.success() {
		return Err(seed_failed(read.stderr));
	}
	Ok(())
}

/// Stages each named path over an index read back to HEAD, so only what
/// was named comes along. A path that is itself ignored was named on
/// purpose and is forced in; a directory that is not ignored brings only
/// its unignored content, as `git add` does, which may be nothing.
async fn stage_paths(
	root: &Path,
	index: &ScratchIndex<'_>,
	paths: &[RelativePath],
) -> Result<(), CoreError> {
	index.run(&["read-tree", "-m", "HEAD"]).await?;
	for path in paths {
		let standing =
			git(root, &["check-ignore", "--quiet", "--", path.as_str()])
				.await?;
		let ignored = match standing.status.code() {
			Some(0) => true,
			Some(1) => false,
			_ => return Err(path_refusal(path, &standing)),
		};
		let mut arguments = vec![
			"--literal-pathspecs",
			"add",
			"--all",
			"--no-warn-embedded-repo",
		];
		if ignored {
			arguments.push("--force");
		}
		arguments.extend(["--", path.as_str()]);
		let added = index.try_output(&arguments).await?;
		if !added.status.success() {
			return Err(path_refusal(path, &added));
		}
	}
	Ok(())
}

/// Why `git add` would not take a named path, by what it said. The
/// messages are matched in English, which the invocation asks for.
fn path_refusal(path: &RelativePath, output: &Output) -> CoreError {
	let stderr = &output.stderr;
	let path = path.as_str();
	if stderr.contains("did not match any files") {
		return CoreError::not_found(
			"workspace.seed_path_not_found",
			format!(
				"the selected path {path:?} names nothing in the Local checkout"
			),
		);
	}
	if stderr.contains("is in submodule") {
		return CoreError::invalid_input(
			"workspace.seed_unsupported",
			format!(
				"the selected path {path:?} lies inside a submodule, which \
				 contributes only the commit it has checked out; select the \
				 submodule itself"
			),
		);
	}
	if stderr.contains("beyond a symbolic link") {
		return CoreError::invalid_input(
			"workspace.seed_unsupported",
			format!(
				"the selected path {path:?} lies beyond a symbolic link, which \
				 is kept as a link and not followed; select the link itself"
			),
		);
	}
	seed_failed(stderr.clone())
}

fn nested_repository(path: &str) -> CoreError {
	CoreError::invalid_input(
		"workspace.seed_unsupported",
		format!(
			"the selected path {path:?} is a repository nested inside the \
			 working tree, which Jet treats as an opaque directory"
		),
	)
}

fn base_mismatch() -> CoreError {
	CoreError::conflict(
		"workspace.seed_base_mismatch",
		"the Project's Local checkout has a different commit checked out \
		 than the selected base, so its changes cannot seed a Workspace \
		 started there; start from the checked-out commit or seed nothing",
	)
}

/// Git answered, but not with a seeded Workspace. The native text stays
/// local (ADR-0061, ADR-0068).
fn seed_failed(detail: String) -> CoreError {
	CoreError::unavailable(
		"workspace.seed_failed",
		"the Local-checkout changes could not be applied to the Workspace",
		detail.chars().take(MAX_DETAIL_CHARS).collect::<String>(),
	)
}
