//! A single client-authorized process group with bounded revocation (ADR-0017).

use rustix::process::{
	Pid, Signal, WaitId, WaitIdOptions, kill_process_group,
	test_kill_process_group, waitid,
};
use std::future::Future;
use std::process::ExitStatus;
use std::time::Duration;
use tokio::process::{Child, ChildStdout, Command};
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// One supervised No-Visa operation. Dropping this handle leaves supervision
/// running until completion or revocation; it never detaches the process.
#[derive(Debug)]
pub struct NoVisaOperation {
	task: JoinHandle<std::io::Result<ExitStatus>>,
	stdout: Option<ChildStdout>,
}

impl NoVisaOperation {
	/// Starts a process in its own group under a revocation signal. The caller
	/// must admit authority and validate the operation's roots and permissions.
	///
	/// # Errors
	/// Returns the OS spawn error without attempting another command.
	pub fn spawn(
		command: &mut Command,
		revoked: impl Future<Output = ()> + Send + 'static,
	) -> std::io::Result<Self> {
		let exited = tokio::signal::unix::signal(
			tokio::signal::unix::SignalKind::child(),
		)?;
		let mut child = command.process_group(0).kill_on_drop(true).spawn()?;
		let stdout = child.stdout.take();
		let group = Pid::from_raw(
			i32::try_from(child.id().expect("new child has a pid"))
				.expect("pid fits i32"),
		);
		let mut process = ProcessGroup { child, group };
		let task = tokio::spawn(async move {
			tokio::select! {
				biased;
				() = revoked => {
					// A failed graceful signal must still reach forced cleanup.
					// In particular, Darwin can report EPERM for exited groups.
					let _ = process.signal(Signal::TERM);
					// Preserve every descendant's full grace, even if the leader exits.
					tokio::time::sleep(Duration::from_secs(2)).await;
				},
				result = observe_exit(group.expect("new process has a group"), exited) => { result?; },
			}
			// ASVS 8.3.2: signal before reaping, while the leader reserves the
			// group id. Normal completion also cleans up remaining descendants.
			let stopped = process.signal(Signal::KILL);
			process.disarm();
			let status = timeout(Duration::from_secs(1), process.child.wait())
				.await
				.map_err(|_| {
					std::io::Error::new(
						std::io::ErrorKind::TimedOut,
						"No-Visa process did not exit after forced stop",
					)
				})??;
			if let Err(error) = stopped {
				// Darwin excludes zombies from killpg and can report EPERM for an
				// already exited group. Accept that only if reaping removed it.
				// Signal 0 cannot affect a new group if the id has been reused.
				if error.kind() != std::io::ErrorKind::PermissionDenied
					|| test_kill_process_group(
						group.expect("owned process has a group"),
					) != Err(rustix::io::Errno::SRCH)
				{
					return Err(error);
				}
			}
			Ok(status)
		});
		Ok(Self { task, stdout })
	}

	/// Takes the output pipe when the launch requested piped stdout.
	pub fn take_stdout(&mut self) -> Option<ChildStdout> {
		self.stdout.take()
	}

	/// Waits for completion or the bounded revocation stop.
	///
	/// # Errors
	/// Returns an OS error or a failed-supervisor error.
	pub async fn wait(self) -> std::io::Result<ExitStatus> {
		self.task.await.map_err(std::io::Error::other)?
	}
}

async fn observe_exit(
	pid: Pid,
	mut exited: tokio::signal::unix::Signal,
) -> std::io::Result<()> {
	loop {
		// WNOWAIT leaves reaping to Child::wait, after final group cleanup.
		if waitid(
			WaitId::Pid(pid),
			WaitIdOptions::EXITED
				| WaitIdOptions::NOHANG
				| WaitIdOptions::NOWAIT,
		)?
		.is_some()
		{
			return Ok(());
		}
		exited.recv().await.ok_or_else(|| {
			std::io::Error::other("child signal stream closed")
		})?;
	}
}

struct ProcessGroup {
	child: Child,
	group: Option<Pid>,
}

impl ProcessGroup {
	fn disarm(&mut self) {
		self.group = None;
	}
	fn signal(&self, signal: Signal) -> std::io::Result<()> {
		if let Some(group) = self.group {
			match kill_process_group(group, signal) {
				Ok(()) | Err(rustix::io::Errno::SRCH) => {}
				Err(error) => return Err(error.into()),
			}
		}
		Ok(())
	}
}

impl Drop for ProcessGroup {
	fn drop(&mut self) {
		// Cancellation before normal reaping stops the owned process group.
		let _ = self.signal(Signal::KILL);
	}
}
