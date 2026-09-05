//! Layout of the per-user Jet home directory (ADR-0014).

use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

const RUNTIME_DIR: &str = "runtime";
const LOCK_FILE: &str = "jetd.lock";
const SOCKET_FILE: &str = "jetd.sock";
const STORE_FILE: &str = "plane.sqlite3";

/// The directory holding everything the Jet core owns for one user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JetHome {
	root: PathBuf,
}

impl JetHome {
	/// A Jet home rooted at `root`.
	#[must_use]
	pub fn at(root: PathBuf) -> Self {
		Self { root }
	}

	/// The default `~/.jet` of the current user, or `None` without `HOME`.
	#[must_use]
	pub fn for_current_user() -> Option<Self> {
		std::env::var_os("HOME")
			.map(|home| Self::at(Path::new(&home).join(".jet")))
	}

	/// Creates the root and owner-only runtime directory if missing.
	///
	/// # Errors
	///
	/// Returns the underlying I/O error when a directory cannot be created
	/// or tightened to owner-only permissions.
	pub fn prepare(&self) -> std::io::Result<()> {
		for dir in [self.root.as_path(), &self.runtime_dir()] {
			fs::DirBuilder::new()
				.recursive(true)
				.mode(0o700)
				.create(dir)?;
			fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
		}
		Ok(())
	}

	/// Root of the Jet home.
	#[must_use]
	pub fn root(&self) -> &Path {
		&self.root
	}

	/// Owner-only directory for the lock and socket.
	pub(crate) fn runtime_dir(&self) -> PathBuf {
		self.root.join(RUNTIME_DIR)
	}

	/// Path of the daemon lifetime lock.
	pub(crate) fn lock_path(&self) -> PathBuf {
		self.runtime_dir().join(LOCK_FILE)
	}

	/// Path of the local Jet protocol socket.
	pub fn socket_path(&self) -> PathBuf {
		self.runtime_dir().join(SOCKET_FILE)
	}

	/// Path of the authoritative Plane store.
	#[must_use]
	pub fn store_path(&self) -> PathBuf {
		self.root.join(STORE_FILE)
	}
}
