//! Isolated execution of one validated destination operation.
use crate::{
	Core, CoreError, RemoteSession, RemoteToolAction, RemoteToolResult,
	remote::tool::{codec, unavailable},
};
use serde::{Deserialize, Serialize};
use std::{
	path::{Path, PathBuf},
	process::Stdio,
	time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// Private worker input, sent only after durable destination admission.
#[derive(Debug, Serialize, Deserialize)]
pub struct RemoteWork {
	/// Root selected from the destination registry, never from the peer.
	pub root: PathBuf,
	/// Exact admitted action.
	pub action: RemoteToolAction,
}

impl RemoteWork {
	/// Performs file I/O or replaces this dedicated worker with the approved
	/// process. Call only in a disposable worker process, never in `jetd serve`.
	///
	/// # Errors
	/// Refuses changed roots, traversal, excessive content and native failures.
	pub async fn execute_in_worker(
		self,
	) -> Result<RemoteToolResult, CoreError> {
		use std::io::Read;
		use std::os::unix::fs::PermissionsExt;
		self.action.validate()?;
		let root = self.root;
		match self.action {
			RemoteToolAction::Terminal {
				directory,
				input,
				rows,
				columns,
			} => {
				let directory = directory_in(&root, &directory)?;
				let (code, stdout) = jet_runtime::no_visa_terminal(
					&directory, &input, rows, columns,
				)
				.await
				.map_err(unavailable)?;
				Ok(RemoteToolResult::Process {
					exit_code: Some(code),
					stdout,
					stderr: String::new(),
				})
			}
			RemoteToolAction::Git { operation } => {
				use std::os::unix::process::CommandExt;
				let mut command = std::process::Command::new("/usr/bin/git");
				command
					.arg("--work-tree")
					.arg(directory_in(&root, "")?)
					.arg("--no-pager")
					.current_dir(directory_in(&root, "")?)
					.env_clear()
					.env("PATH", "/usr/bin:/bin")
					.env("HOME", &root)
					.env("GIT_CONFIG_NOSYSTEM", "1")
					.env("GIT_CONFIG_GLOBAL", "/dev/null")
					.env("GIT_OPTIONAL_LOCKS", "0")
					.args([
						"-c",
						"core.fsmonitor=false",
						"-c",
						"core.hooksPath=/dev/null",
						"-c",
						"diff.external=",
					]);
				match operation {
					crate::RemoteGitOperation::Status => {
						command.args([
							"status",
							"--porcelain=v1",
							"--untracked-files=normal",
						]);
					}
					crate::RemoteGitOperation::Diff => {
						command.args([
							"diff",
							"--no-ext-diff",
							"--no-textconv",
							"--no-color",
							"--",
							".",
						]);
					}
				}
				Err(unavailable(command.exec()))
			}
			RemoteToolAction::Process {
				arguments,
				directory,
				environment,
			} => {
				use std::os::unix::process::CommandExt;
				let mut command = std::process::Command::new(&arguments[0]);
				command
					.args(&arguments[1..])
					.current_dir(directory_in(&root, &directory)?);
				environment_for(&mut command, &root, environment);
				Err(unavailable(command.exec()))
			}
			RemoteToolAction::Shell {
				directory,
				environment,
				script,
			} => {
				use std::os::unix::process::CommandExt;
				let directory = directory_in(&root, &directory)?;
				let mut command = std::process::Command::new("/bin/sh");
				command.args(["-c", &script]).current_dir(directory);
				environment_for(&mut command, &root, environment);
				Err(unavailable(command.exec()))
			}
			RemoteToolAction::ReadFile { path } => {
				let granted =
					crate::filesystem::relative_path::GrantedRoot::verify(
						&root,
					)?;
				let path = crate::RelativePath::parse(&path)?
					.resolve_within(&granted)?;
				let file = std::fs::File::open(path).map_err(unavailable)?;
				if !file.metadata().map_err(unavailable)?.is_file() {
					return Err(CoreError::invalid_input(
						"remote.not_file",
						"the path must name a regular file",
					));
				}
				let mut content = String::new();
				file.take(65537)
					.read_to_string(&mut content)
					.map_err(unavailable)?;
				if content.len() > 65536 {
					return Err(CoreError::invalid_input(
						"remote.too_large",
						"remote file content exceeds 64 KiB",
					));
				}
				Ok(RemoteToolResult::File { content })
			}
			RemoteToolAction::WriteFile { path, content } => {
				let path = crate::RelativePath::parse(&path)?;
				let granted =
					crate::filesystem::relative_path::GrantedRoot::verify(
						&root,
					)?;
				let destination = path.resolve_within(&granted)?;
				let mode = match std::fs::metadata(destination) {
					Ok(metadata) if metadata.is_file() => {
						Some(metadata.permissions().mode())
					}
					Ok(_) => {
						return Err(CoreError::invalid_input(
							"remote.not_file",
							"the destination must name a regular file",
						));
					}
					Err(error)
						if error.kind() == std::io::ErrorKind::NotFound =>
					{
						None
					}
					Err(error) => return Err(unavailable(error)),
				};
				crate::user_input::files::replace_atomic(
					root, path, content, mode,
				)?;
				Ok(RemoteToolResult::Written)
			}
		}
	}
}

pub(crate) fn directory_in(
	root: &Path,
	directory: &str,
) -> Result<PathBuf, CoreError> {
	let granted = crate::filesystem::relative_path::GrantedRoot::verify(root)?;
	let directory = if directory.is_empty() {
		root.to_path_buf()
	} else {
		crate::RelativePath::parse(directory)?.resolve_within(&granted)?
	};
	if !directory.is_dir() {
		return Err(CoreError::invalid_input(
			"remote.invalid_directory",
			"the working directory must exist inside the Workspace",
		));
	}
	Ok(directory)
}

impl Core {
	pub(crate) async fn perform_remote_work(
		&self,
		session: &RemoteSession,
		work: RemoteWork,
	) -> Result<RemoteToolResult, CoreError> {
		let worker = self.remote_worker.as_ref().ok_or_else(|| {
			CoreError::conflict(
				"remote.worker_unavailable",
				"no remote operation worker was configured",
			)
		})?;
		let process_output = matches!(
			work.action,
			RemoteToolAction::Shell { .. }
				| RemoteToolAction::Process { .. }
				| RemoteToolAction::Git { .. }
		);
		let input = serde_json::to_vec(&work).map_err(codec)?;
		let mut command = tokio::process::Command::new(worker);
		command
			.arg("remote-worker")
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::piped());
		let mut operation = self.spawn_no_visa(session, &mut command).await?;
		let mut stdin = operation.take_stdin().expect("piped worker stdin");
		let stdout = operation.take_stdout().expect("piped worker stdout");
		let stderr = operation.take_stderr().expect("piped worker stderr");
		let limit = if process_output { 65536 } else { 512 * 1024 };
		let output = tokio::time::timeout(Duration::from_secs(60), async {
			stdin.write_all(&input).await.map_err(unavailable)?;
			stdin.shutdown().await.map_err(unavailable)?;
			drop(stdin);
			tokio::try_join!(bounded(stdout, limit), bounded(stderr, 65536))
		})
		.await;
		let output = match output {
			Ok(Ok(output)) => Ok(output),
			Ok(Err(error)) => {
				operation.stop();
				Err(error)
			}
			Err(_) => {
				operation.stop();
				Err(CoreError::conflict(
					"remote.outcome_unknown",
					"the operation exceeded its bound; its effects must be inspected before retrying",
				))
			}
		};
		let status = operation.wait().await.map_err(unavailable)?;
		let (stdout, stderr) = output?;
		if session.authorize().is_err() || status.code().is_none() {
			return Err(CoreError::conflict(
				"remote.outcome_unknown",
				"the operation stopped before its effects could be established",
			));
		}
		if process_output {
			Ok(RemoteToolResult::Process {
				exit_code: status.code(),
				stdout: String::from_utf8_lossy(&stdout).into_owned(),
				stderr: String::from_utf8_lossy(&stderr).into_owned(),
			})
		} else {
			serde_json::from_slice(&stdout).map_err(codec)?
		}
	}
}

async fn bounded(
	reader: impl AsyncRead + Unpin,
	limit: usize,
) -> Result<Vec<u8>, CoreError> {
	let mut bytes = Vec::new();
	reader
		.take(limit as u64 + 1)
		.read_to_end(&mut bytes)
		.await
		.map_err(unavailable)?;
	if bytes.len() > limit {
		return Err(CoreError::conflict(
			"remote.output_limit",
			"the operation exceeded its output bound and was stopped",
		));
	}
	Ok(bytes)
}

fn environment_for(
	command: &mut std::process::Command,
	root: &Path,
	changes: Vec<crate::RemoteEnvironment>,
) {
	command
		.env_clear()
		.env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
		.env("HOME", root);
	for change in changes {
		match change.value {
			Some(value) => {
				command.env(change.name, value);
			}
			None => {
				command.env_remove(change.name);
			}
		}
	}
}
