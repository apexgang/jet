//! Owns exactly one Craft until its daemon closes the private control pipe.
//! The helper/Harness process tree is independently owned by jetfueld.
use std::{
	fs::OpenOptions,
	os::{fd::AsFd, unix::fs::OpenOptionsExt},
	path::Path,
	process::Stdio,
	time::Duration,
};
use tokio::{io::AsyncReadExt, net::unix::pipe::Receiver, process::Command};

pub(crate) async fn run(
	executable: &Path,
	socket: &Path,
	digest: &str,
) -> std::io::Result<()> {
	if !executable.is_absolute()
		|| !socket.is_absolute()
		|| digest.len() != 64
		|| !digest
			.bytes()
			.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
	{
		return Err(std::io::Error::other(
			"invalid Craft supervisor arguments",
		));
	}
	// This pipe read is cancellable; blocking stdin would prevent shutdown
	// when the Craft exits while its daemon is still alive.
	let mut control = Receiver::from_owned_fd(
		std::io::stdin().as_fd().try_clone_to_owned()?,
	)?;
	let lock = OpenOptions::new()
		.read(true)
		.write(true)
		.create(true)
		.truncate(false)
		.mode(0o600)
		.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
		.open(
			socket
				.parent()
				.ok_or_else(|| {
					std::io::Error::other("missing runtime directory")
				})?
				.join(format!("craft-{digest}.lock")),
		)?;
	let acquire = async {
		loop {
			match rustix::fs::flock(
				&lock,
				rustix::fs::FlockOperation::NonBlockingLockExclusive,
			) {
				Ok(()) => return Ok::<_, std::io::Error>(()),
				Err(rustix::io::Errno::WOULDBLOCK) => {
					tokio::time::sleep(Duration::from_millis(20)).await
				}
				Err(error) => return Err(error.into()),
			}
		}
	};
	tokio::select! {
		biased;
		_ = control.read_u8() => return Ok(()),
		result = acquire => result?,
	}
	// Stable per-digest lock files are never unlinked. A replacement daemon
	// cannot start this digest until the predecessor has reaped its Craft.
	let mut child = Command::new(executable)
		.arg("--socket")
		.arg(socket)
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.kill_on_drop(true)
		.spawn()?;
	tokio::select! {
		result = child.wait() => { result?; },
		command = control.read_u8() => {
			if matches!(command, Ok(b'T')) {
				if let Some(pid) = child.id().and_then(|id| i32::try_from(id).ok()).and_then(rustix::process::Pid::from_raw) {
					let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
				}
				if tokio::time::timeout(Duration::from_secs(1), child.wait()).await.is_err() {
					child.kill().await?;
				}
			} else {
				child.kill().await?;
			}
		}
	}
	let _ = std::fs::remove_file(socket);
	// Explicit ordering: reap first, then release the digest lock.
	drop(lock);
	Ok(())
}
