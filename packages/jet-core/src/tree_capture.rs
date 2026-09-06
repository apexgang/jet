//! Capturing a working tree as a Git tree through a scratch index
//! (ADR-0025, ADR-0103).
//!
//! A capture never touches the index the user works with: the checkout's
//! index is copied to a scratch file, changes are staged into the copy,
//! and the copy is written as a tree. The tree is immutable and lives in
//! the repository's own object store, so a Workspace of the same Project
//! can read it and a later comparison can name it. Seeding a Workspace
//! and promoting one both capture this way; each names the refusal it
//! answers with when Git cannot capture.

use std::io;
use std::path::{Path, PathBuf};

use crate::error::CoreError;
use crate::filesystem::blocking;
use crate::repository::{Output, git, git_with_index};

/// The index mode of a Git link: a commit in another repository.
const GIT_LINK_MODE: &str = "160000";

/// The mode `diff-tree` gives a path one of its trees lacks.
const ABSENT_MODE: &str = "000000";

/// One path that differs between two trees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Change {
	pub(crate) path: String,
	source_mode: String,
	destination_mode: String,
}

impl Change {
	/// Whether the change adds a Git link where the source had nothing: a
	/// repository nested inside the working tree, which Jet treats as an
	/// opaque directory (ADR-0103).
	pub(crate) fn is_nested_repository(&self) -> bool {
		self.source_mode == ABSENT_MODE
			&& self.destination_mode == GIT_LINK_MODE
	}

	/// Whether the destination tree has the path at all.
	pub(crate) fn is_addition(&self) -> bool {
		self.source_mode == ABSENT_MODE
	}

	/// Whether the destination tree lacks the path.
	pub(crate) fn is_deletion(&self) -> bool {
		self.destination_mode == ABSENT_MODE
	}
}

/// A scratch index of the repository at one root, and the refusal every
/// Git failure through it becomes.
pub(crate) struct ScratchIndex<'a> {
	root: &'a Path,
	index: &'a Path,
	failure: fn(String) -> CoreError,
}

impl<'a> ScratchIndex<'a> {
	pub(crate) fn new(
		root: &'a Path,
		index: &'a Path,
		failure: fn(String) -> CoreError,
	) -> Self {
		Self {
			root,
			index,
			failure,
		}
	}

	/// Copies the checkout's own index here, keeping the index file's own
	/// modification time. Git decides whether an entry's recorded stat
	/// data can be trusted by comparing the entry's time with the index
	/// file's; a copy stamped with the time of copying would vouch for
	/// every entry, and a file changed in the same second as the last
	/// index write, to content of the same size, would be captured as it
	/// was rather than as it is. A working tree that was never checked
	/// out has no index yet, and the copy then starts from HEAD.
	pub(crate) async fn copy_from_checkout(&self) -> Result<(), CoreError> {
		let located = git(
			self.root,
			&["rev-parse", "--path-format=absolute", "--git-path", "index"],
		)
		.await?;
		if !located.status.success() {
			return Err((self.failure)(located.stderr));
		}
		let source = PathBuf::from(located.stdout.trim_end());
		let target = self.index.to_path_buf();
		match blocking(move || copy_with_times(&source, &target)).await? {
			Ok(()) => Ok(()),
			Err(error) if error.kind() == io::ErrorKind::NotFound => {
				self.run(&["read-tree", "HEAD"]).await
			}
			Err(error) => Err((self.failure)(error.to_string())),
		}
	}

	/// Stages every eligible change: `git add --all` respects ignore rules,
	/// records a submodule as its checked-out commit, and records a
	/// symbolic link as itself.
	pub(crate) async fn stage_everything(&self) -> Result<(), CoreError> {
		self.run(&[
			"--literal-pathspecs",
			"add",
			"--all",
			"--no-warn-embedded-repo",
			"--",
			".",
		])
		.await
	}

	/// Writes the index as a tree and returns the tree's name.
	pub(crate) async fn write_tree(&self) -> Result<String, CoreError> {
		let written =
			git_with_index(self.root, self.index, &["write-tree"]).await?;
		if !written.status.success() {
			return Err((self.failure)(written.stderr));
		}
		object_name(written.stdout, self.failure)
	}

	/// Captures the whole working tree, staged and unstaged changes alike,
	/// as one tree, and reports what it changes against `head`. A
	/// repository nested inside the working tree is dropped from the tree
	/// rather than recorded as a Git link (ADR-0103).
	pub(crate) async fn capture_everything(
		&self,
		head: &str,
	) -> Result<(String, Vec<Change>), CoreError> {
		self.stage_everything().await?;
		let tree = self.write_tree().await?;
		let changed = diff_trees(self.root, head, &tree, self.failure).await?;
		let nested: Vec<&str> = changed
			.iter()
			.filter(|change| change.is_nested_repository())
			.map(|change| change.path.as_str())
			.collect();
		if nested.is_empty() {
			return Ok((tree, changed));
		}
		let mut arguments = vec!["update-index", "--force-remove", "--"];
		arguments.extend(nested);
		self.run(&arguments).await?;
		let tree = self.write_tree().await?;
		let changed = diff_trees(self.root, head, &tree, self.failure).await?;
		Ok((tree, changed))
	}

	/// Runs one Git command against this index and keeps only whether it
	/// succeeded.
	pub(crate) async fn run(
		&self,
		arguments: &[&str],
	) -> Result<(), CoreError> {
		let output = git_with_index(self.root, self.index, arguments).await?;
		if !output.status.success() {
			return Err((self.failure)(output.stderr));
		}
		Ok(())
	}

	/// Runs one Git command against this index and returns what it wrote,
	/// whether or not it succeeded, so the caller can read the refusal.
	pub(crate) async fn try_output(
		&self,
		arguments: &[&str],
	) -> Result<Output, CoreError> {
		git_with_index(self.root, self.index, arguments).await
	}
}

/// Copies `source` to `target` and gives the copy the source's
/// modification time.
fn copy_with_times(source: &Path, target: &Path) -> io::Result<()> {
	let modified = std::fs::metadata(source)?.modified()?;
	std::fs::copy(source, target)?;
	std::fs::File::options()
		.write(true)
		.open(target)?
		.set_times(std::fs::FileTimes::new().set_modified(modified))
}

/// What `destination` changes against `source`, one record per path.
pub(crate) async fn diff_trees(
	root: &Path,
	source: &str,
	destination: &str,
	failure: fn(String) -> CoreError,
) -> Result<Vec<Change>, CoreError> {
	let diff = git(
		root,
		&[
			"diff-tree",
			"-r",
			"-z",
			"--end-of-options",
			source,
			destination,
		],
	)
	.await?;
	if !diff.status.success() {
		return Err(failure(diff.stderr));
	}
	parse_changes(&diff.stdout, failure)
}

/// Parses `diff-tree -r -z` output: for each path, a record
/// `:<source mode> <destination mode> <source> <destination> <status>`
/// followed by the path, each ended by NUL.
fn parse_changes(
	records: &str,
	failure: fn(String) -> CoreError,
) -> Result<Vec<Change>, CoreError> {
	let mut fields = records.split('\0');
	let mut changes = Vec::new();
	while let Some(record) = fields.next() {
		if record.is_empty() {
			break;
		}
		let (Some(modes), Some(path)) =
			(record.strip_prefix(':'), fields.next())
		else {
			return Err(failure(format!(
				"diff-tree answered with an unreadable record {record:?}"
			)));
		};
		let mut modes = modes.split(' ');
		let (Some(source), Some(destination)) = (modes.next(), modes.next())
		else {
			return Err(failure(format!(
				"diff-tree answered with an unreadable record {record:?}"
			)));
		};
		changes.push(Change {
			path: path.to_owned(),
			source_mode: source.to_owned(),
			destination_mode: destination.to_owned(),
		});
	}
	Ok(changes)
}

/// The object name Git printed, checked to be one.
pub(crate) fn object_name(
	printed: String,
	failure: fn(String) -> CoreError,
) -> Result<String, CoreError> {
	let name = printed.trim().to_owned();
	if name.len() < 40 || !name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
		return Err(failure(printed));
	}
	Ok(name)
}
