//! One bounded native PTY exchange, owned by a disposable remote worker.
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use rustix::process::{
	Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid,
};
use std::{
	io::{self, Read},
	path::Path,
	time::Duration,
};

/// Executes a bounded, reviewed input batch in a real controlling PTY.
/// The caller must isolate this worker and authorize its root and input.
/// # Errors
/// Returns native I/O failures, output overflow, or revocation. TERM receives
/// one second of grace before forced cleanup; the outer worker is also bounded.
pub async fn no_visa_terminal(
	root: &Path,
	input: &str,
	rows: u16,
	columns: u16,
) -> io::Result<(i32, String)> {
	if !(1..=1000).contains(&rows)
		|| !(1..=1000).contains(&columns)
		|| input.len() > 16384
	{
		return Err(io::Error::other("invalid terminal bounds"));
	}
	// Install before spawn so revocation cannot abandon a newly created PTY.
	let mut revoked = tokio::signal::unix::signal(
		tokio::signal::unix::SignalKind::terminate(),
	)?;
	let pair = native_pty_system()
		.openpty(PtySize {
			rows,
			cols: columns,
			pixel_width: 0,
			pixel_height: 0,
		})
		.map_err(io::Error::other)?;
	let mut command = CommandBuilder::new("/bin/sh");
	// The reviewed batch uses a controlling PTY without implicit interactive
	// job control, so ordinary background jobs stay in the owned group.
	command.args(["-c", input]);
	command.cwd(root);
	command.env_clear();
	command.env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
	command.env("HOME", root);
	command.env("TERM", "xterm-256color");
	let child = pair
		.slave
		.spawn_command(command)
		.map_err(io::Error::other)?;
	drop(pair.slave);
	let pid = child
		.process_id()
		.and_then(|p| i32::try_from(p).ok())
		.and_then(Pid::from_raw)
		.ok_or_else(|| io::Error::other("missing PTY process identity"))?;
	let mut terminal = OwnedTerminal {
		master: pair.master,
		child,
		pid,
	};
	let fd = terminal
		.master
		.as_raw_fd()
		.ok_or_else(|| io::Error::other("PTY descriptor unavailable"))?;
	let mut reader =
		filedescriptor::FileDescriptor::dup(&fd).map_err(io::Error::other)?;
	reader.set_non_blocking(true).map_err(io::Error::other)?;
	let mut output = Vec::new();
	loop {
		let mut buffer = [0; 4096];
		match reader.read(&mut buffer) {
			Ok(count) => output.extend_from_slice(&buffer[..count]),
			Err(error)
				if error.kind() == io::ErrorKind::WouldBlock
					|| error.raw_os_error() == Some(5) => {}
			Err(error) => return Err(error),
		}
		if output.len() > 65536 {
			return Err(io::Error::other("terminal output exceeded 64 KiB"));
		}
		// Keep the leader unreaped until every owned group has been stopped.
		if waitid(
			WaitId::Pid(pid),
			WaitIdOptions::EXITED
				| WaitIdOptions::NOHANG
				| WaitIdOptions::NOWAIT,
		)?
		.is_some()
		{
			terminal.signal(Signal::KILL);
			let status = terminal.child.wait()?;
			// Drain the kernel's remaining bounded output after exit.
			while let Ok(count) = reader.read(&mut buffer) {
				if count == 0 {
					break;
				}
				output.extend_from_slice(&buffer[..count]);
				if output.len() > 65536 {
					return Err(io::Error::other(
						"terminal output exceeded 64 KiB",
					));
				}
			}
			return Ok((
				i32::try_from(status.exit_code()).unwrap_or(1),
				String::from_utf8_lossy(&output).into_owned(),
			));
		}
		tokio::select! {
			_ = revoked.recv() => {
				terminal.signal(Signal::TERM);
				tokio::time::sleep(Duration::from_secs(1)).await;
				return Err(io::Error::other("terminal operation was revoked"));
			},
			() = tokio::time::sleep(Duration::from_millis(5)) => {},
		}
	}
}
struct OwnedTerminal {
	master: Box<dyn MasterPty + Send>,
	child: Box<dyn portable_pty::Child + Send + Sync>,
	pid: Pid,
}
impl OwnedTerminal {
	fn signal(&self, signal: Signal) {
		if let Some(foreground) =
			self.master.process_group_leader().and_then(Pid::from_raw)
		{
			let _ = kill_process_group(foreground, signal);
		}
		let _ = kill_process_group(self.pid, signal);
	}
}
impl Drop for OwnedTerminal {
	fn drop(&mut self) {
		// Never signal a reused PID after the portable child has been reaped.
		if waitid(
			WaitId::Pid(self.pid),
			WaitIdOptions::EXITED
				| WaitIdOptions::NOHANG
				| WaitIdOptions::NOWAIT,
		)
		.is_ok()
		{
			self.signal(Signal::KILL);
			let _ = self.child.wait();
		}
	}
}
