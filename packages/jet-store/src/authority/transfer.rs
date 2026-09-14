//! Plane transfers this Plane took part in, on either side, and the
//! authority column each phase moves (ADR-0070).

use super::{AuthorityRecord, AuthorityState, PendingFence};
use crate::{
	StoreError,
	conversation::trash::TrashReasonRecord,
	records::{column_error, parse_uuid},
	transaction::{ReadTransaction, WriteTransaction},
};
use uuid::Uuid;

/// Which side of a transfer this Plane is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneTransferRole {
	/// This Plane prepared the bundle and will relinquish.
	Source,
	/// This Plane imported the bundle and will commit.
	Target,
}

impl PlaneTransferRole {
	/// The durable spelling.
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Source => "source",
			Self::Target => "target",
		}
	}

	fn parse(text: &str) -> Result<Self, StoreError> {
		[Self::Source, Self::Target]
			.into_iter()
			.find(|role| role.as_str() == text)
			.ok_or_else(|| {
				column_error("role", format!("unknown transfer role {text:?}"))
			})
	}
}

/// How far a transfer has come on this Plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneTransferPhase {
	/// The source has prepared the bundle, or the target has imported it.
	Prepared,
	/// The source has fenced its authority.
	Relinquished,
	/// The target has validated the fence and taken authority.
	Committed,
}

impl PlaneTransferPhase {
	/// The durable spelling.
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Prepared => "prepared",
			Self::Relinquished => "relinquished",
			Self::Committed => "committed",
		}
	}

	fn parse(text: &str) -> Result<Self, StoreError> {
		[Self::Prepared, Self::Relinquished, Self::Committed]
			.into_iter()
			.find(|phase| phase.as_str() == text)
			.ok_or_else(|| {
				column_error(
					"phase",
					format!("unknown transfer phase {text:?}"),
				)
			})
	}
}

/// One Plane transfer as this Plane recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneTransferRecord {
	/// The transfer's identity, shared by both Planes.
	pub transfer_id: Uuid,
	/// The Conversation whose authority moves.
	pub conversation_id: Uuid,
	/// Which side this Plane is.
	pub role: PlaneTransferRole,
	/// How far it has come here.
	pub phase: PlaneTransferPhase,
	/// The source's epoch, which the transfer retires; the target holds
	/// the epoch after it.
	pub retired_epoch: u64,
	/// The other Plane.
	pub peer_plane_id: Uuid,
	/// SHA-256 of the bundle, in lowercase hex, which every retry must
	/// present.
	pub bundle_sha256: String,
	/// When this Plane recorded the transfer.
	pub prepared_at_unix_ms: i64,
	/// When it was relinquished or committed here.
	pub settled_at_unix_ms: Option<i64>,
}

/// A transfer to record in its prepared phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPlaneTransfer {
	/// The transfer's identity, shared by both Planes.
	pub transfer_id: Uuid,
	/// The Conversation whose authority moves.
	pub conversation_id: Uuid,
	/// Which side this Plane is.
	pub role: PlaneTransferRole,
	/// The source's epoch, which the transfer retires.
	pub retired_epoch: u64,
	/// The other Plane.
	pub peer_plane_id: Uuid,
	/// SHA-256 of the bundle, in lowercase hex.
	pub bundle_sha256: String,
	/// When this Plane recorded the transfer.
	pub prepared_at_unix_ms: i64,
}

struct Row {
	transfer_id: String,
	conversation_id: String,
	role: String,
	phase: String,
	retired_epoch: i64,
	peer_plane_id: String,
	bundle_sha256: String,
	prepared_at_unix_ms: i64,
	settled_at_unix_ms: Option<i64>,
}

fn read_row(row: Row) -> Result<PlaneTransferRecord, StoreError> {
	Ok(PlaneTransferRecord {
		transfer_id: parse_uuid("transfer_id", &row.transfer_id)?,
		conversation_id: parse_uuid("conversation_id", &row.conversation_id)?,
		role: PlaneTransferRole::parse(&row.role)?,
		phase: PlaneTransferPhase::parse(&row.phase)?,
		retired_epoch: u64::try_from(row.retired_epoch).map_err(|_| {
			column_error("retired_epoch", "retired epoch is negative".into())
		})?,
		peer_plane_id: parse_uuid("peer_plane_id", &row.peer_plane_id)?,
		bundle_sha256: row.bundle_sha256,
		prepared_at_unix_ms: row.prepared_at_unix_ms,
		settled_at_unix_ms: row.settled_at_unix_ms,
	})
}

impl ReadTransaction {
	/// The transfer called `transfer_id`, if this Plane recorded it.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn plane_transfer(
		&mut self,
		transfer_id: Uuid,
	) -> Result<Option<PlaneTransferRecord>, StoreError> {
		let transfer_id = transfer_id.to_string();
		sqlx::query_as!(
			Row,
			r#"SELECT transfer_id AS "transfer_id!", conversation_id, role,
				phase, retired_epoch, peer_plane_id, bundle_sha256,
				prepared_at_unix_ms, settled_at_unix_ms
			 FROM plane_transfers WHERE transfer_id = ?1"#,
			transfer_id
		)
		.fetch_optional(self.connection())
		.await?
		.map(read_row)
		.transpose()
	}

	/// The transfer of `conversation_id` that is still under way here: a
	/// source's prepared transfer or a target's, until it settles.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn open_plane_transfer(
		&mut self,
		conversation_id: Uuid,
	) -> Result<Option<PlaneTransferRecord>, StoreError> {
		let conversation_id = conversation_id.to_string();
		sqlx::query_as!(
			Row,
			r#"SELECT transfer_id AS "transfer_id!", conversation_id, role,
				phase, retired_epoch, peer_plane_id, bundle_sha256,
				prepared_at_unix_ms, settled_at_unix_ms
			 FROM plane_transfers
			 WHERE conversation_id = ?1 AND phase = 'prepared'
			 ORDER BY prepared_at_unix_ms DESC LIMIT 1"#,
			conversation_id
		)
		.fetch_optional(self.connection())
		.await?
		.map(read_row)
		.transpose()
	}
}

impl WriteTransaction {
	/// Records a transfer in its prepared phase.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written, including
	/// when the identity is already taken.
	pub async fn insert_plane_transfer(
		&mut self,
		transfer: NewPlaneTransfer,
	) -> Result<PlaneTransferRecord, StoreError> {
		let record = PlaneTransferRecord {
			transfer_id: transfer.transfer_id,
			conversation_id: transfer.conversation_id,
			role: transfer.role,
			phase: PlaneTransferPhase::Prepared,
			retired_epoch: transfer.retired_epoch,
			peer_plane_id: transfer.peer_plane_id,
			bundle_sha256: transfer.bundle_sha256,
			prepared_at_unix_ms: transfer.prepared_at_unix_ms,
			settled_at_unix_ms: None,
		};
		let transfer_id = record.transfer_id.to_string();
		let conversation_id = record.conversation_id.to_string();
		let role = record.role.as_str();
		let phase = record.phase.as_str();
		let epoch = i64::try_from(record.retired_epoch).unwrap_or(i64::MAX);
		let peer = record.peer_plane_id.to_string();
		sqlx::query!(
			"INSERT INTO plane_transfers
				(transfer_id, conversation_id, role, phase, retired_epoch,
					peer_plane_id, bundle_sha256, prepared_at_unix_ms)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
			transfer_id,
			conversation_id,
			role,
			phase,
			epoch,
			peer,
			record.bundle_sha256,
			record.prepared_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(record)
	}

	/// Sets the authority of `conversation_id` and advances its Revision.
	/// The target's import marks its copy Prepared with the epoch after
	/// the retired one; the commit marks it Home.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written.
	pub async fn set_conversation_authority(
		&mut self,
		conversation_id: Uuid,
		authority: AuthorityRecord,
	) -> Result<(), StoreError> {
		let conversation_id = conversation_id.to_string();
		let state = authority.state.as_str();
		let epoch = i64::try_from(authority.epoch).unwrap_or(i64::MAX);
		sqlx::query!(
			"UPDATE conversations SET authority = ?2, authority_epoch = ?3,
				revision = revision + 1
			 WHERE conversation_id = ?1",
			conversation_id,
			state,
			epoch
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Moves a transfer to `phase`, stamped `settled_at_unix_ms`.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written.
	pub async fn settle_plane_transfer(
		&mut self,
		transfer_id: Uuid,
		phase: PlaneTransferPhase,
		settled_at_unix_ms: i64,
	) -> Result<(), StoreError> {
		let transfer_id = transfer_id.to_string();
		let phase = phase.as_str();
		sqlx::query!(
			"UPDATE plane_transfers SET phase = ?2, settled_at_unix_ms = ?3
			 WHERE transfer_id = ?1",
			transfer_id,
			phase,
			settled_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Removes the Transfer tombstone of a relinquished Conversation, rows
	/// and Trash entry, so the Conversation can come back in a later epoch.
	/// The identity is not deleted, so the Deletion ledger is not told: a
	/// restored snapshot meets the fence instead (ADR-0070).
	///
	/// # Errors
	///
	/// Returns [`StoreError::Integrity`] when the Conversation is not
	/// relinquished, or another [`StoreError`] when a row cannot be removed.
	pub async fn discard_transfer_tombstone(
		&mut self,
		conversation_id: Uuid,
	) -> Result<(), StoreError> {
		let relinquished = self
			.conversation(conversation_id)
			.await?
			.is_some_and(|conversation| {
				conversation.authority.state == AuthorityState::Relinquished
			});
		if !relinquished {
			return Err(StoreError::Integrity(format!(
				"conversation {conversation_id} is not a Transfer tombstone"
			)));
		}
		let id = conversation_id.to_string();
		crate::conversation::purge::purge_rows(self.connection(), &id).await?;
		Ok(())
	}

	/// Forgets a transfer that was prepared and then abandoned before the
	/// source relinquished.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be removed.
	pub async fn delete_plane_transfer(
		&mut self,
		transfer_id: Uuid,
	) -> Result<(), StoreError> {
		let transfer_id = transfer_id.to_string();
		sqlx::query!(
			"DELETE FROM plane_transfers WHERE transfer_id = ?1",
			transfer_id
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Relinquishes the authority of a source transfer: raises the fence
	/// for the ledger the transaction writes before it commits, marks the
	/// Conversation relinquished, settles the transfer, and leaves the
	/// content as a Transfer tombstone in Jet Trash until
	/// `tombstone_expires_at_unix_ms` (ADR-0070).
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when a row cannot be written.
	pub async fn relinquish_plane_transfer(
		&mut self,
		transfer: &PlaneTransferRecord,
		now_unix_ms: i64,
		tombstone_expires_at_unix_ms: i64,
	) -> Result<(), StoreError> {
		// A fence the ledger already holds, reapplied at open after a crash
		// between it and its commit, is not raised twice: this transaction
		// finishes what that one did not.
		let already_fenced = self
			.conversation(transfer.conversation_id)
			.await?
			.is_some_and(|conversation| {
				conversation.authority
					== AuthorityRecord {
						state: AuthorityState::Relinquished,
						epoch: transfer.retired_epoch,
					}
			});
		self.set_conversation_authority(
			transfer.conversation_id,
			AuthorityRecord {
				state: AuthorityState::Relinquished,
				epoch: transfer.retired_epoch,
			},
		)
		.await?;
		self.settle_plane_transfer(
			transfer.transfer_id,
			PlaneTransferPhase::Relinquished,
			now_unix_ms,
		)
		.await?;
		self.insert_trash(crate::TrashRecord {
			conversation_id: transfer.conversation_id,
			reason: TrashReasonRecord::Transferred,
			trashed_at_unix_ms: now_unix_ms,
			expires_at_unix_ms: tombstone_expires_at_unix_ms,
		})
		.await?;
		if !already_fenced {
			self.record_fence(PendingFence {
				fenced_at_unix_ms: now_unix_ms,
				conversation_id: transfer.conversation_id,
				retired_epoch: transfer.retired_epoch,
				transfer_id: transfer.transfer_id,
				target_plane_id: transfer.peer_plane_id,
			});
		}
		Ok(())
	}
}
