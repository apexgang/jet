//! Native PTY hosting for terminal-role helpers (ADR-0051).
use portable_pty::{
	Child, CommandBuilder, MasterPty, PtySize, native_pty_system,
};
use std::{
	io::{self, Read, Write},
	path::Path,
};

/// Owned PTY process and its resize/termination handle.
pub struct TerminalPty {
	master: Box<dyn MasterPty + Send>,
	child: Box<dyn Child + Send + Sync>,
}
/// Independently owned byte pipes; reads and writes may return `WouldBlock`.
pub struct TerminalPipes {
	/// PTY output, with stderr merged by the kernel.
	pub reader: Box<dyn Read + Send>,
	/// PTY input.
	pub writer: Box<dyn Write + Send>,
}
impl TerminalPty {
	/// Opens a controlling PTY and starts the host-selected shell.
	/// Returns native launch/IO errors without substituting pipes for a PTY.
	pub fn open(
		root: &Path,
		shell: &str,
		rows: u16,
		columns: u16,
	) -> io::Result<(Self, TerminalPipes)> {
		let pair = native_pty_system()
			.openpty(size(rows, columns)?)
			.map_err(io::Error::other)?;
		// Duplicate through a safe platform wrapper while the master owns the fd.
		let fd = pair
			.master
			.as_raw_fd()
			.ok_or_else(|| io::Error::other("PTY fd unavailable"))?;
		let mut pipe = filedescriptor::FileDescriptor::dup(&fd)
			.map_err(io::Error::other)?;
		pipe.set_non_blocking(true).map_err(io::Error::other)?;
		let reader = Box::new(pipe);
		let writer = pair.master.take_writer().map_err(io::Error::other)?;
		// ASVS 1.2.5: shell and working root are fixed by authenticated admission;
		// terminal bytes arrive solely through its PTY, never a command-line string.
		let mut command = CommandBuilder::new(shell);
		command.arg("-i");
		command.cwd(root);
		command.env("TERM", "xterm-256color");
		let child = pair
			.slave
			.spawn_command(command)
			.map_err(io::Error::other)?;
		drop(pair.slave);
		Ok((
			Self {
				master: pair.master,
				child,
			},
			TerminalPipes { reader, writer },
		))
	}
	/// Resizes the native PTY, propagating the native window-change notification.
	/// Returns invalid dimensions or native IO errors.
	pub fn resize(&self, rows: u16, columns: u16) -> io::Result<()> {
		self.master
			.resize(size(rows, columns)?)
			.map_err(io::Error::other)
	}
	/// Terminates the owned child and reaps it. Never targets a descriptor PID.
	/// Returns an error unless termination is established.
	pub fn close(&mut self) -> io::Result<()> {
		if self.child.try_wait()?.is_none() {
			// The foreground process group comes from this owned controlling PTY,
			// never from a request or descriptor. Stop a job that ignores SIGHUP.
			if let Some(group) = self
				.master
				.process_group_leader()
				.and_then(rustix::process::Pid::from_raw)
			{
				match rustix::process::kill_process_group(
					group,
					rustix::process::Signal::KILL,
				) {
					Ok(()) | Err(rustix::io::Errno::SRCH) => {}
					Err(error) => return Err(error.into()),
				}
			}
			if self.child.try_wait()?.is_none() {
				self.child.kill()?;
			}
		}
		self.child.wait()?;
		Ok(())
	}
	/// Reaps a completed shell without blocking a running one.
	/// Returns native wait errors.
	pub fn exited(&mut self) -> io::Result<bool> {
		Ok(self.child.try_wait()?.is_some())
	}
}
impl Drop for TerminalPty {
	fn drop(&mut self) {
		let _ = self.close();
	}
}
fn size(rows: u16, cols: u16) -> io::Result<PtySize> {
	if !(1..=1000).contains(&rows) || !(1..=1000).contains(&cols) {
		return Err(io::Error::other("invalid terminal dimensions"));
	}
	Ok(PtySize {
		rows,
		cols,
		pixel_width: 0,
		pixel_height: 0,
	})
}
