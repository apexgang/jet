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

use tokio::process::Command;

use crate::capability::{Capability, ExternalTool};
use crate::error::CoreError;

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

/// Runs one `git` command at `root` (ASVS 1.2.5, 5.3.8: an argument array,
/// never shell source).
async fn git(root: &Path, arguments: &[&str]) -> Result<Output, CoreError> {
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
	let output = command
		// An inspection takes no optional lock and answers no prompt.
		.env("GIT_OPTIONAL_LOCKS", "0")
		.env("GIT_TERMINAL_PROMPT", "0")
		// The messages this module reads are matched in English.
		.env("LC_ALL", "C")
		.arg("-C")
		.arg(root)
		.args(arguments)
		.stdin(Stdio::null())
		.kill_on_drop(true)
		.output()
		.await
		.map_err(|error| match error.kind() {
			io::ErrorKind::NotFound => CoreError::capability_unavailable(
				Capability::ExternalTool(ExternalTool::Git),
			),
			_ => inspection_failed(error.to_string()),
		})?;
	Ok(Output {
		status: output.status,
		stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
		stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
	})
}

/// Whether `root` holds a `.git` entry of any kind, which is what tells a
/// broken repository from a directory that was never one.
async fn has_git_entry(root: &Path) -> Result<bool, CoreError> {
	let entry = root.join(".git");
	blocking(move || std::fs::symlink_metadata(&entry).is_ok()).await
}

/// Resolves `path` as the filesystem names it, off the runtime.
pub(crate) async fn canonicalize(path: PathBuf) -> io::Result<PathBuf> {
	blocking(move || std::fs::canonicalize(path))
		.await
		.map_err(|error| io::Error::other(error.to_string()))?
}

/// Runs filesystem work on a blocking thread, so a slow mount stalls that
/// thread and not the runtime every connection shares.
pub(crate) async fn blocking<T: Send + 'static>(
	work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, CoreError> {
	tokio::task::spawn_blocking(work).await.map_err(|error| {
		CoreError::internal("filesystem.task_failed", error.to_string())
	})
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
