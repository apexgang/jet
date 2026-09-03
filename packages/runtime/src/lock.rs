//! The per-Plane daemon lifetime lock (ADR-0003).
//!
//! Ownership is established only by the operating-system lock, which the
//! kernel releases when the owning process exits for any reason. The JSON
//! metadata written into the lock file is descriptive: it lets a refused
//! daemon report who owns the Plane, but by itself never proves ownership.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};

use rustix::fs::FlockOperation;
use serde::{Deserialize, Serialize};

use crate::JetHome;

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

impl PartialEq for LockError {
	fn eq(&self, other: &Self) -> bool {
		match (self, other) {
			(Self::Held { owner }, Self::Held { owner: other_owner }) => {
				owner == other_owner
			}
			(Self::Io(a), Self::Io(b)) => a.kind() == b.kind(),
			(Self::Held { .. } | Self::Io(_), _) => false,
		}
	}
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

fn read_owner(file: &mut File) -> Option<DaemonMetadata> {
	let mut contents = String::new();
	file.read_to_string(&mut contents).ok()?;
	serde_json::from_str(&contents).ok()
}

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
