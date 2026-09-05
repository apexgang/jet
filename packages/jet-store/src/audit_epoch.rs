//! Authority epochs of the Security audit chain (ADR-0105).
//!
//! An epoch is a stretch of audit the Plane can vouch for as one unbroken
//! chain. The first begins with the first record ever written. Every later
//! one exists because validation failed and the owner chose to carry on:
//! it records the head the previous epoch was last known to have and the
//! reason the chain restarts, so the gap is part of the audit rather than
//! something the audit is silent about.

use uuid::Uuid;

use crate::StoreError;
use crate::audit_chain::{AuditEntryHash, EpochGenesis, genesis_hash};
use crate::transaction::{ReadTransaction, WriteTransaction};

/// One recorded epoch of the audit chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EpochRow {
	pub(crate) epoch: u64,
	pub(crate) started_at_unix_ms: i64,
	/// The head the preceding epoch was last known to have, and the reason
	/// this one succeeds it. Absent for the first epoch.
	pub(crate) preceding: Option<PrecedingEpoch>,
}

/// What an epoch records about the one it replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrecedingEpoch {
	pub(crate) sequence: u64,
	pub(crate) entry_hash: AuditEntryHash,
	pub(crate) gap_reason: String,
}

impl EpochRow {
	/// The chain link the first record of this epoch follows.
	pub(crate) fn genesis(&self, plane_id: Uuid) -> AuditEntryHash {
		genesis_hash(&EpochGenesis {
			epoch: self.epoch,
			plane_id,
			started_at_unix_ms: self.started_at_unix_ms,
			preceding: self
				.preceding
				.as_ref()
				.map(|preceding| (preceding.sequence, preceding.entry_hash)),
			gap_reason: self
				.preceding
				.as_ref()
				.map(|preceding| preceding.gap_reason.as_str()),
		})
	}
}

impl ReadTransaction {
	/// The newest epoch, or `None` before the audit has ever been written.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub(crate) async fn newest_audit_epoch(
		&mut self,
	) -> Result<Option<EpochRow>, StoreError> {
		let row = sqlx::query!(
			r#"SELECT epoch AS "epoch!", started_at_unix_ms,
				preceding_sequence, preceding_entry_hash, gap_reason
			 FROM audit_epochs
			 ORDER BY epoch DESC
			 LIMIT 1"#
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(|row| {
			read_epoch(
				row.epoch,
				row.started_at_unix_ms,
				row.preceding_sequence,
				row.preceding_entry_hash,
				row.gap_reason,
			)
		})
		.transpose()
	}
}

impl WriteTransaction {
	/// The epoch new records belong to, creating the first one when this
	/// Plane has never written an audit record.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the epoch cannot be read or created.
	pub(crate) async fn current_audit_epoch(
		&mut self,
		now_unix_ms: i64,
	) -> Result<EpochRow, StoreError> {
		match self.newest_audit_epoch().await? {
			Some(epoch) => Ok(epoch),
			None => self.insert_audit_epoch(1, now_unix_ms, None).await,
		}
	}

	pub(crate) async fn insert_audit_epoch(
		&mut self,
		epoch: u64,
		started_at_unix_ms: i64,
		preceding: Option<PrecedingEpoch>,
	) -> Result<EpochRow, StoreError> {
		let number = epoch_column(epoch)?;
		let sequence = preceding
			.as_ref()
			.map(|preceding| sequence_column(preceding.sequence))
			.transpose()?;
		let hash = preceding
			.as_ref()
			.map(|preceding| preceding.entry_hash.0.to_vec());
		let reason = preceding
			.as_ref()
			.map(|preceding| preceding.gap_reason.clone());
		sqlx::query!(
			"INSERT INTO audit_epochs (epoch, started_at_unix_ms,
				preceding_sequence, preceding_entry_hash, gap_reason)
			 VALUES (?1, ?2, ?3, ?4, ?5)",
			number,
			started_at_unix_ms,
			sequence,
			hash,
			reason
		)
		.execute(self.connection())
		.await?;
		Ok(EpochRow {
			epoch,
			started_at_unix_ms,
			preceding,
		})
	}
}

fn read_epoch(
	epoch: i64,
	started_at_unix_ms: i64,
	preceding_sequence: Option<i64>,
	preceding_entry_hash: Option<Vec<u8>>,
	gap_reason: Option<String>,
) -> Result<EpochRow, StoreError> {
	let preceding = match (preceding_sequence, preceding_entry_hash, gap_reason)
	{
		(None, None, None) => None,
		(Some(sequence), Some(hash), Some(gap_reason)) => {
			Some(PrecedingEpoch {
				sequence: parse_sequence(sequence)?,
				entry_hash: parse_entry_hash(&hash)?,
				gap_reason,
			})
		}
		_ => {
			return Err(StoreError::Integrity(format!(
				"audit epoch {epoch} records an incomplete predecessor"
			)));
		}
	};
	Ok(EpochRow {
		epoch: parse_sequence(epoch)?,
		started_at_unix_ms,
		preceding,
	})
}

pub(crate) fn parse_entry_hash(
	bytes: &[u8],
) -> Result<AuditEntryHash, StoreError> {
	<[u8; 32]>::try_from(bytes)
		.map(AuditEntryHash)
		.map_err(|_| {
			StoreError::Integrity(format!(
				"an audit chain hash is 32 bytes, not {}",
				bytes.len()
			))
		})
}

pub(crate) fn parse_sequence(value: i64) -> Result<u64, StoreError> {
	u64::try_from(value).map_err(|_| {
		StoreError::Integrity(format!("audit position {value} is negative"))
	})
}

pub(crate) fn sequence_column(sequence: u64) -> Result<i64, StoreError> {
	i64::try_from(sequence).map_err(|_| {
		StoreError::Integrity(format!("audit position {sequence} overflows"))
	})
}

fn epoch_column(epoch: u64) -> Result<i64, StoreError> {
	i64::try_from(epoch).map_err(|_| {
		StoreError::Integrity(format!("audit epoch {epoch} overflows"))
	})
}
