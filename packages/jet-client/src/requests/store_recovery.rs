//! Restoring a Recovery snapshot over a damaged Plane store, and purging
//! the snapshots a deletion may survive in.

use super::unexpected;
use crate::connection::{Client, ClientError};
use jet_protocol::{CommandRequest, CommandResponse};
use uuid::Uuid;

/// What a Recovery purge took and removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgedSnapshots {
	/// The snapshot taken after every recorded deletion.
	pub snapshot: String,
	/// The snapshots removed, newest first.
	pub removed: Vec<String>,
}

/// What a restoration replaced and with what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredSnapshot {
	/// The snapshot that is now the store.
	pub snapshot: String,
	/// The file name, beside the store, the previous database was moved to.
	pub replaced: String,
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
			CommandResponse::RecoverySnapshotRestored {
				snapshot,
				replaced,
			} => Ok(RestoredSnapshot { snapshot, replaced }),
			other => Err(unexpected(&other)),
		}
	}

	/// Takes a verified snapshot of the Plane store after every deletion
	/// its Deletion ledger records and removes every older snapshot that
	/// may still hold what was deleted, under the Command identity
	/// `command_id` (ADR-0102). It is not replayed from a receipt: a retry
	/// takes another snapshot and removes nothing.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn purge_recovery_snapshots(
		&self,
		command_id: Uuid,
	) -> Result<PurgedSnapshots, ClientError> {
		self.require_minor(jet_protocol::DELETION_LEDGER_MINOR)?;
		match self
			.execute_command(command_id, CommandRequest::PurgeRecoverySnapshots)
			.await?
		{
			CommandResponse::RecoverySnapshotsPurged { snapshot, removed } => {
				Ok(PurgedSnapshots { snapshot, removed })
			}
			other => Err(unexpected(&other)),
		}
	}
}
