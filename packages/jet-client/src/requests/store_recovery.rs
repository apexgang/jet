//! Restoring a Recovery snapshot over a damaged Plane store.

use super::unexpected;
use crate::connection::{Client, ClientError};
use jet_protocol::{CommandRequest, CommandResponse};
use uuid::Uuid;

/// What a restoration replaced and with what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredSnapshot {
	/// The snapshot that is now the store.
	pub snapshot: String,
	/// The file name, beside the store, the damaged database was moved to.
	pub damaged: String,
}

impl Client {
	/// Restores the verified Recovery snapshot called `snapshot` over the
	/// store of a Plane in read-only Recovery mode, under the Command
	/// identity `command_id` (ADR-0077). It is refused while the Plane
	/// serves, and it is not replayed from a receipt: a retry after success
	/// is refused too.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn restore_recovery_snapshot(
		&self,
		command_id: Uuid,
		snapshot: String,
	) -> Result<RestoredSnapshot, ClientError> {
		match self
			.execute_command(
				command_id,
				CommandRequest::RestoreRecoverySnapshot { snapshot },
			)
			.await?
		{
			CommandResponse::RecoverySnapshotRestored { snapshot, damaged } => {
				Ok(RestoredSnapshot { snapshot, damaged })
			}
			other => Err(unexpected(&other)),
		}
	}
}
