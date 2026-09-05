//! What `git` says about a granted root (ADR-0056, ADR-0103).
//!
//! The core invokes the detected `git` through argument arrays and never a
//! shell, and it inspects without changing anything: no lock is taken that
//! Git could do without, no prompt is answered, and every `GIT_*` variable
//! the daemon inherited is dropped so the environment cannot point Git at a
//! repository other than the one at the root.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::capability::{Capability, ExternalTool};
use crate::error::CoreError;
use crate::filesystem::{blocking, canonicalize};
use crate::project::{Checkout, GitLink, Worktree};

/// How long one whole inspection may take before it is reported as having
/// failed. A repository on a slow mount stalls one Command, not the Plane.
const INSPECTION_BUDGET: Duration = Duration::from_secs(15);

/// Longest native Git message kept as local diagnostic detail (ADR-0061).
const MAX_DETAIL_CHARS: usize = 512;

/// Whether a granted root can be a Project (ADR-0103).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
	/// An ordinary non-bare working tree, or a linked worktree.
	Registrable,
	/// Git finds no repository at the root or above it.
	NotARepository,
	/// The root carries a `.git` entry that Git cannot open, such as a
	/// linked worktree whose repository is gone.
	BrokenRepository,
	/// A bare repository, which has no working tree for Runs, diffs, and
	/// Change checkpoints to use.
	BareRepository,
	/// The root lies inside a repository's own `.git` directory.
	InsideGitDir,
	/// The root lies inside a working tree without being its top. The grant
	/// is for the directory named, so it is not widened to the top.
	InsideWorkingTree {
		/// The top of the working tree the root lies in.
		toplevel: PathBuf,
	},
}

/// What `git` reports about a registrable working tree (ADR-0103).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Inspection {
	/// Whether the working tree is the repository's own or a linked one.
	pub(crate) worktree: Worktree,
	/// Whether a sparse checkout narrows it.
	pub(crate) checkout: Checkout,
	/// The Git links its index holds, in index order.
	pub(crate) submodules: Vec<GitLink>,
}

/// The index mode of a Git link: a commit in another repository.
const GIT_LINK_MODE: &str = "160000";

/// What one `git` invocation produced.
struct Output {
	status: ExitStatus,
	stdout: String,
	stderr: String,
}

/// Decides whether `root`, a canonical directory, can be a Project.
///
/// # Errors
///
/// Returns `capability.unavailable` when `git` cannot be run at all, and
/// an `unavailable` `project.inspection_failed` when it answers with
/// something this core cannot interpret or does not answer in time.
pub(crate) async fn verdict(root: &Path) -> Result<Verdict, CoreError> {
	tokio::time::timeout(INSPECTION_BUDGET, registrability(root))
		.await
		.map_err(|_| {
			inspection_failed("the inspection did not finish in time".into())
		})?
}

/// Describes the registrable working tree at `root`.
///
/// The description is read and never written: sparse-checkout
/// configuration is reported as found, and a submodule contributes its
/// Git link alone, so nothing here enters another repository.
///
/// # Errors
///
/// Returns what [`verdict`] returns when `git` cannot answer.
pub(crate) async fn inspect(root: &Path) -> Result<Inspection, CoreError> {
	tokio::time::timeout(INSPECTION_BUDGET, details(root))
		.await
		.map_err(|_| {
			inspection_failed("the inspection did not finish in time".into())
		})?
}

async fn details(root: &Path) -> Result<Inspection, CoreError> {
	Ok(Inspection {
		worktree: worktree(root).await?,
		checkout: checkout(root).await?,
		submodules: git_links(root).await?,
	})
}

/// A linked worktree keeps its own `.git` directory apart from the one it
/// shares; the repository's own working tree keeps the two together.
async fn worktree(root: &Path) -> Result<Worktree, CoreError> {
	let dirs =
		git(root, &["rev-parse", "--git-dir", "--git-common-dir"]).await?;
	if !dirs.status.success() {
		return Err(inspection_failed(dirs.stderr));
	}
	let mut lines = dirs.stdout.lines().map(|line| root.join(line.trim()));
	let (Some(git_dir), Some(common_dir)) = (lines.next(), lines.next()) else {
		return Err(inspection_failed(dirs.stdout));
	};
	let git_dir = canonicalize(git_dir)
		.await
		.map_err(|error| inspection_failed(error.to_string()))?;
	let common_dir = canonicalize(common_dir)
		.await
		.map_err(|error| inspection_failed(error.to_string()))?;
	Ok(if git_dir == common_dir {
		Worktree::Main
	} else {
		Worktree::Linked { common_dir }
	})
}

/// Sparse checkout is configuration, which is unset in most working trees
/// and answered with an exit status of one; only a value Git cannot read
/// is a failure.
async fn checkout(root: &Path) -> Result<Checkout, CoreError> {
	let sparse = git(
		root,
		&["config", "--type=bool", "--get", "core.sparseCheckout"],
	)
	.await?;
	match (sparse.status.code(), sparse.stdout.trim()) {
		(Some(0), "true") => Ok(Checkout::Sparse),
		(Some(0), _) | (Some(1), _) => Ok(Checkout::Full),
		_ => Err(inspection_failed(sparse.stderr)),
	}
}

/// The Git links in the index, read from the index itself: `.gitmodules`
/// is optional metadata, and a nested repository that was added without
/// it is a Git link all the same (ADR-0103).
///
/// The listing is streamed and only the links are kept, so an index of a
/// million files costs time and not memory.
async fn git_links(root: &Path) -> Result<Vec<GitLink>, CoreError> {
	let mut child = command(root)
		.args(["--literal-pathspecs", "ls-files", "--stage", "-z"])
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.map_err(spawn_failure)?;
	let stdout = child.stdout.take().ok_or_else(|| {
		inspection_failed("git ls-files answered without output".into())
	})?;
	let mut records = BufReader::new(stdout);
	let mut record = Vec::new();
	let mut links = Vec::new();
	loop {
		record.clear();
		let read = records
			.read_until(0, &mut record)
			.await
			.map_err(|error| inspection_failed(error.to_string()))?;
		if read == 0 {
			break;
		}
		if record.last() == Some(&0) {
			record.pop();
		}
		if let Some(link) = git_link(&record) {
			links.push(link);
		}
	}
	let finished = child
		.wait_with_output()
		.await
		.map_err(|error| inspection_failed(error.to_string()))?;
	if !finished.status.success() {
		return Err(inspection_failed(
			String::from_utf8_lossy(&finished.stderr).into_owned(),
		));
	}
	Ok(links)
}

/// One `ls-files --stage -z` record, `<mode> <object> <stage>\t<path>`,
/// when it is a Git link.
fn git_link(record: &[u8]) -> Option<GitLink> {
	let record = String::from_utf8_lossy(record);
	let (fields, path) = record.split_once('\t')?;
	let mut fields = fields.split(' ');
	let (mode, object) = (fields.next()?, fields.next()?);
	(mode == GIT_LINK_MODE).then(|| GitLink {
		path: path.into(),
		commit: object.into(),
	})
}

async fn registrability(root: &Path) -> Result<Verdict, CoreError> {
	let flags = git(
		root,
		&[
			"rev-parse",
			"--is-bare-repository",
			"--is-inside-git-dir",
			"--is-inside-work-tree",
		],
	)
	.await?;
	if !flags.status.success() {
		if flags.stderr.contains("not a git repository") {
			return Ok(if has_git_entry(root).await? {
				Verdict::BrokenRepository
			} else {
				Verdict::NotARepository
			});
		}
		return Err(inspection_failed(flags.stderr));
	}
	let answers: Vec<&str> = flags.stdout.lines().map(str::trim).collect();
	match answers.as_slice() {
		["true", _, _] => return Ok(Verdict::BareRepository),
		[_, "true", _] => return Ok(Verdict::InsideGitDir),
		[_, _, "true"] => {}
		_ => return Err(inspection_failed(flags.stdout)),
	}
	let toplevel = git(root, &["rev-parse", "--show-toplevel"]).await?;
	if !toplevel.status.success() {
		return Err(inspection_failed(toplevel.stderr));
	}
	let toplevel = canonicalize(PathBuf::from(toplevel.stdout.trim_end()))
		.await
		.map_err(|error| inspection_failed(error.to_string()))?;
	if toplevel == root {
		Ok(Verdict::Registrable)
	} else {
		Ok(Verdict::InsideWorkingTree { toplevel })
	}
}

/// Runs one `git` command at `root` and collects what it printed.
async fn git(root: &Path, arguments: &[&str]) -> Result<Output, CoreError> {
	let output = command(root)
		.args(arguments)
		.output()
		.await
		.map_err(spawn_failure)?;
	Ok(Output {
		status: output.status,
		stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
		stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
	})
}

/// A `git` invocation at `root` (ASVS 1.2.5, 5.3.8: an argument array,
/// never shell source).
fn command(root: &Path) -> Command {
	let mut command = Command::new(ExternalTool::Git.as_str());
	// Any inherited GIT_* variable can redirect discovery, rewrite
	// configuration, or change how pathspecs are read, so none of them
	// reaches the inspection. HOME and PATH stay, so the user's own
	// configuration and tool lookup still apply.
	for (name, _) in std::env::vars_os() {
		if name.as_encoded_bytes().starts_with(b"GIT_") {
			command.env_remove(name);
		}
	}
	command
		// An inspection takes no optional lock and answers no prompt.
		.env("GIT_OPTIONAL_LOCKS", "0")
		.env("GIT_TERMINAL_PROMPT", "0")
		// The messages this module reads are matched in English.
		.env("LC_ALL", "C")
		.arg("-C")
		.arg(root)
		.stdin(Stdio::null())
		.kill_on_drop(true);
	command
}

/// A `git` that could not be started at all is a Capability the Plane
/// lacks; anything else is an inspection that failed.
fn spawn_failure(error: io::Error) -> CoreError {
	match error.kind() {
		io::ErrorKind::NotFound => CoreError::capability_unavailable(
			Capability::ExternalTool(ExternalTool::Git),
		),
		_ => inspection_failed(error.to_string()),
	}
}

/// Whether `root` holds a `.git` entry of any kind, which is what tells a
/// broken repository from a directory that was never one.
async fn has_git_entry(root: &Path) -> Result<bool, CoreError> {
	let entry = root.join(".git");
	blocking(move || std::fs::symlink_metadata(&entry).is_ok()).await
}

/// Git answered, but not in a way this core can act on. The native text
/// stays local (ADR-0061, ADR-0068).
fn inspection_failed(detail: String) -> CoreError {
	CoreError::unavailable(
		"project.inspection_failed",
		"the repository could not be inspected",
		detail.chars().take(MAX_DETAIL_CHARS).collect::<String>(),
	)
}
