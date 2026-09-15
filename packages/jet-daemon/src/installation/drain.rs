//! Asking the owning `jetd` to leave the Plane (ADR-0026, ADR-0088).
//!
//! The owner is found through the lifetime lock, told to drain with
//! `SIGTERM`, and considered gone only once the operating-system lock is
//! free again: metadata alone never proves anything about ownership.

use jet_runtime::{
	DaemonMetadata, JetHome, LifetimeLock, LockError, LockProbe,
};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(100);

/// How the Plane came to be free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DrainOutcome {
	/// Nothing owned the Plane.
	Idle,
	/// The named daemon drained and relinquished its lock.
	Drained(DaemonMetadata),
}

/// Why the Plane could not be freed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DrainError {
	/// A live process holds the lock but left no readable metadata, so
	/// there is no process this command may safely signal.
	#[error("a live process owns the Plane but left no readable metadata")]
	OwnerUnknown,
	/// The owner did not relinquish the lock within the deadline.
	#[error("jetd (pid {}) did not relinquish the Plane within {timeout:?}", .owner.pid)]
	Timeout {
		/// The owner that was signaled.
		owner: DaemonMetadata,
		/// How long the caller waited.
		timeout: Duration,
	},
	/// The lock could not be inspected.
	#[error(transparent)]
	Lock(#[from] LockError),
	/// The owner could not be signaled.
	#[error("cannot signal jetd (pid {pid}): {errno}")]
	Signal {
		/// The process that was to be signaled.
		pid: u32,
		/// The failure.
		errno: rustix::io::Errno,
	},
}

/// Looks up the owner of `home` without disturbing the lock.
pub(crate) fn owner(home: &JetHome) -> Result<LockProbe, LockError> {
	LifetimeLock::probe(home)
}

/// Signals the owner of `home` to drain and waits up to `timeout` for the
/// lock to be free. Returns as soon as the Plane is free.
pub(crate) async fn drain(
	home: &JetHome,
	timeout: Duration,
) -> Result<DrainOutcome, DrainError> {
	let owner = match LifetimeLock::probe(home)? {
		LockProbe::Free => return Ok(DrainOutcome::Idle),
		LockProbe::Held(None) => return Err(DrainError::OwnerUnknown),
		LockProbe::Held(Some(owner)) => owner,
	};
	let pid = i32::try_from(owner.pid)
		.ok()
		.and_then(rustix::process::Pid::from_raw)
		.ok_or(DrainError::OwnerUnknown)?;
	match rustix::process::kill_process(pid, rustix::process::Signal::TERM) {
		// A process that exited between the probe and the signal has
		// relinquished the lock already; the poll below confirms it.
		Ok(()) | Err(rustix::io::Errno::SRCH) => {}
		Err(errno) => {
			return Err(DrainError::Signal {
				pid: owner.pid,
				errno,
			});
		}
	}
	let deadline = Instant::now() + timeout;
	loop {
		match LifetimeLock::probe(home)? {
			LockProbe::Free => return Ok(DrainOutcome::Drained(owner)),
			LockProbe::Held(_) if Instant::now() >= deadline => {
				return Err(DrainError::Timeout { owner, timeout });
			}
			LockProbe::Held(_) => tokio::time::sleep(POLL).await,
		}
	}
}
