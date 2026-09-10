//! The per-Plane daemon lifetime lock (ADR-0003).
//!
//! Ownership is established only by the operating-system lock, which the
//! kernel releases when the owning process exits for any reason. The JSON
//! metadata written into the lock file is descriptive: it lets a refused
//! daemon report who owns the Plane, but by itself never proves ownership.

use crate::JetHome;
use rustix::{fs::FlockOperation, process::Pid};
use serde::{Deserialize, Serialize};
use std::{
	fs::{File, OpenOptions},
	io::{Read, Seek, Write},
};

/// How the running daemon was installed and is managed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallationChannel {
	/// Run from a development checkout.
	Development,
	/// Bundled with and managed by a desktop GUI.
	Gui,
	/// Installed and managed by Homebrew.
	Homebrew,
}

/// Descriptive metadata about the daemon holding the lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonMetadata {
	/// Process identifier of the owner.
	pub pid: u32,
	/// Version of the owner.
	pub version: String,
	/// Installation channel of the owner.
	pub channel: InstallationChannel,
}

/// Why the lock could not be acquired.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
	/// Another live process holds the lock.
	#[error("another jetd owns this Plane: {owner:?}")]
	Held {
		/// Metadata the owner left, when readable.
		owner: Option<DaemonMetadata>,
	},
	/// The lock file could not be opened or written.
	#[error("lifetime lock I/O failure: {0}")]
	Io(#[from] std::io::Error),
}

/// An exclusive claim on one Plane, released when dropped or when the
/// process exits.
#[derive(Debug)]
pub struct LifetimeLock {
	file: File,
}

impl LifetimeLock {
	/// Claims the Plane under `home` for the daemon described by `metadata`.
	///
	/// # Errors
	///
	/// Returns [`LockError::Held`] with the current owner's metadata when a
	/// live process already holds the lock, or [`LockError::Io`] otherwise.
	pub fn acquire(
		home: &JetHome,
		metadata: &DaemonMetadata,
	) -> Result<Self, LockError> {
		let mut file = OpenOptions::new()
			.read(true)
			.write(true)
			.create(true)
			.truncate(false)
			.open(home.lock_path())?;
		match rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive)
		{
			Ok(()) => {}
			Err(rustix::io::Errno::WOULDBLOCK) => {
				return Err(LockError::Held {
					owner: read_owner(&mut file),
				});
			}
			Err(errno) => return Err(LockError::Io(errno.into())),
		}
		file.set_len(0)?;
		file.rewind()?;
		file.write_all(
			serde_json::to_string(metadata)
				.map_err(std::io::Error::other)?
				.as_bytes(),
		)?;
		file.sync_all()?;
		Ok(Self { file })
	}
}

impl Drop for LifetimeLock {
	fn drop(&mut self) {
		// Closing the file releases the lock as well; unlocking explicitly
		// keeps the release independent of drop order.
		let _ = rustix::fs::flock(&self.file, FlockOperation::Unlock);
	}
}

/// Reads the owner's metadata and validates it against the live system:
/// the process it names must exist and the version must be present.
/// Anything else is reported as unreadable rather than trusted.
fn read_owner(file: &mut File) -> Option<DaemonMetadata> {
	let mut contents = String::new();
	file.read_to_string(&mut contents).ok()?;
	let metadata: DaemonMetadata = serde_json::from_str(&contents).ok()?;
	if metadata.version.is_empty() || !process_exists(metadata.pid) {
		return None;
	}
	Some(metadata)
}

fn process_exists(pid: u32) -> bool {
	let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
		return false;
	};
	match rustix::process::test_kill_process(pid) {
		Ok(()) | Err(rustix::io::Errno::PERM) => true,
		Err(_) => false,
	}
}

#[cfg(test)]
mod tests {
	use pretty_assertions::assert_eq;

	use super::{DaemonMetadata, InstallationChannel, LifetimeLock, LockError};
	use crate::JetHome;

	/// The owner a refused acquisition reported.
	fn reported_owner(error: LockError) -> Option<DaemonMetadata> {
		let LockError::Held { owner } = error else {
			panic!("expected the lock to be held, got {error:?}");
		};
		owner
	}

	fn metadata(pid: u32) -> DaemonMetadata {
		DaemonMetadata {
			pid,
			version: "0.1.0".into(),
			channel: InstallationChannel::Development,
		}
	}

	#[test]
	fn a_second_daemon_is_refused_and_told_who_owns_the_plane() {
		let dir = tempfile::tempdir().unwrap();
		let home = JetHome::at(dir.path().join(".jet"));
		home.prepare().unwrap();

		let owner = metadata(std::process::id());

		let _first = LifetimeLock::acquire(&home, &owner).unwrap();
		let error = LifetimeLock::acquire(&home, &metadata(42)).unwrap_err();

		assert_eq!(reported_owner(error), Some(owner));
	}

	#[test]
	fn metadata_naming_a_dead_process_is_not_reported_as_the_owner() {
		let dir = tempfile::tempdir().unwrap();
		let home = JetHome::at(dir.path().join(".jet"));
		home.prepare().unwrap();
		let mut exited = std::process::Command::new("true").spawn().unwrap();
		exited.wait().unwrap();

		let _first =
			LifetimeLock::acquire(&home, &metadata(exited.id())).unwrap();
		let error = LifetimeLock::acquire(&home, &metadata(42)).unwrap_err();

		assert_eq!(reported_owner(error), None);
	}

	#[test]
	fn stale_metadata_from_a_released_lock_does_not_establish_ownership() {
		let dir = tempfile::tempdir().unwrap();
		let home = JetHome::at(dir.path().join(".jet"));
		home.prepare().unwrap();

		let first = LifetimeLock::acquire(&home, &metadata(41)).unwrap();
		drop(first);
		let second = LifetimeLock::acquire(&home, &metadata(42));

		assert!(second.is_ok(), "{second:?}");
	}
}
