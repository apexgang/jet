//! Owner-only local IPC listener with peer-identity validation (ADR-0087).

use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::PathBuf;

use tokio::net::{UnixListener, UnixStream};

use crate::JetHome;

/// Failure while binding or accepting local connections.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
	/// The connecting process runs as a different user.
	#[error("rejected local peer with uid {uid}")]
	PeerRejected {
		/// The peer's effective user id.
		uid: u32,
	},
	/// The socket path is occupied by something other than a socket.
	#[error("socket path {0} is not a socket")]
	PathOccupied(PathBuf),
	/// Transport failure.
	#[error("local IPC failure: {0}")]
	Io(#[from] std::io::Error),
}

/// Unix socket listener that admits only the socket owner's processes.
#[derive(Debug)]
pub struct LocalListener {
	listener: UnixListener,
	path: PathBuf,
	owner_uid: u32,
}

impl LocalListener {
	/// Binds the Plane socket under `home`, replacing a stale socket file.
	///
	/// # Errors
	///
	/// Returns [`IpcError::PathOccupied`] when a non-socket file sits at
	/// the socket path, or [`IpcError::Io`] on bind or permission failure.
	pub fn bind(home: &JetHome) -> Result<Self, IpcError> {
		let path = home.socket_path();
		match fs::symlink_metadata(&path) {
			Ok(metadata) if metadata.file_type().is_socket() => {
				fs::remove_file(&path)?;
			}
			Ok(_) => return Err(IpcError::PathOccupied(path)),
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
			Err(error) => return Err(error.into()),
		}
		let listener = UnixListener::bind(&path)?;
		fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
		Ok(Self {
			listener,
			path,
			owner_uid: rustix::process::geteuid().as_raw(),
		})
	}

	/// Path of the bound socket.
	#[must_use]
	pub fn socket_path(&self) -> &PathBuf {
		&self.path
	}

	/// Accepts the next connection whose peer runs as the socket owner.
	///
	/// # Errors
	///
	/// Returns [`IpcError::PeerRejected`] after closing a connection from
	/// another user, or [`IpcError::Io`] on transport failure.
	pub async fn accept(&self) -> Result<UnixStream, IpcError> {
		let (stream, _) = self.listener.accept().await?;
		let uid = stream.peer_cred()?.uid();
		if uid != self.owner_uid {
			return Err(IpcError::PeerRejected { uid });
		}
		Ok(stream)
	}
}

impl Drop for LocalListener {
	fn drop(&mut self) {
		let _ = fs::remove_file(&self.path);
	}
}

#[cfg(test)]
#[path = "ipc_tests.rs"]
mod tests;
