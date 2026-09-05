//! Reading the Security audit back (ADR-0105).
//!
//! Pages are read oldest first and fenced by the position the audit had
//! reached, the way every other snapshot is fenced (ADR-0092). Validation
//! reads one epoch at a time, because an epoch is exactly the stretch of
//! audit the Plane still vouches for.

use crate::StoreError;
use crate::audit::{
	AUDIT_PAGE_LIMIT, AuditOutcome, AuditRecord, AuditRisk, AuditTip,
	RetentionAnchor,
};
use crate::audit_chain::{AuditEntryHash, AuditTargetRef};
use crate::audit_epoch::{counter_column, parse_counter};
use crate::records::{ActorRecord, parse_uuid};
use crate::transaction::ReadTransaction;

/// One `security_audit` row as SQLite stores it.
struct Row {
	sequence: i64,
	epoch: i64,
	record_id: String,
	recorded_at_unix_ms: i64,
	plane_id: String,
	actor_kind: String,
	actor_id: String,
	target_kind: String,
	target_reference: Vec<u8>,
	target_id: Option<String>,
	decision: String,
	risk: String,
	outcome: String,
	entry_hash: Vec<u8>,
}

impl ReadTransaction {
	/// Up to `limit` records strictly after `after`, oldest first, beside
	/// the newest position the audit held when the page was read.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn audit_page(
		&mut self,
		after: u64,
		limit: usize,
	) -> Result<(u64, Vec<AuditRecord>), StoreError> {
		let cursor = self.audit_cursor().await?;
		// ASVS 2.2.1/2.2.2: cap allocation-driving input again at the
		// trusted store seam, even when the caller already applies a limit.
		let limit = i64::try_from(limit.min(AUDIT_PAGE_LIMIT)).unwrap_or(1);
		let after = counter_column(after)?;
		let rows = sqlx::query_as!(
			Row,
			r#"SELECT sequence AS "sequence!", epoch, record_id,
				recorded_at_unix_ms, plane_id, actor_kind, actor_id,
				target_kind, target_reference, target_id, decision, risk,
				outcome, entry_hash
			 FROM security_audit
			 WHERE sequence > ?1
			 ORDER BY sequence
			 LIMIT ?2"#,
			after,
			limit
		)
		.fetch_all(self.connection())
		.await?;
		let records =
			rows.into_iter().map(read_row).collect::<Result<_, _>>()?;
		Ok((cursor, records))
	}

	/// Up to `limit` records of `epoch` strictly after `after`, oldest
	/// first. Validation walks one epoch at a time, because an epoch is
	/// exactly the stretch of audit the Plane still vouches for.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub(crate) async fn audit_epoch_page(
		&mut self,
		epoch: u64,
		after: u64,
		limit: usize,
	) -> Result<Vec<AuditRecord>, StoreError> {
		let limit = i64::try_from(limit.min(AUDIT_PAGE_LIMIT)).unwrap_or(1);
		let epoch = counter_column(epoch)?;
		let after = counter_column(after)?;
		let rows = sqlx::query_as!(
			Row,
			r#"SELECT sequence AS "sequence!", epoch, record_id,
				recorded_at_unix_ms, plane_id, actor_kind, actor_id,
				target_kind, target_reference, target_id, decision, risk,
				outcome, entry_hash
			 FROM security_audit
			 WHERE epoch = ?1 AND sequence > ?2
			 ORDER BY sequence
			 LIMIT ?3"#,
			epoch,
			after,
			limit
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter().map(read_row).collect()
	}

	/// The newest position the audit has reached, or zero before its first
	/// record.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the position cannot be read.
	pub async fn audit_cursor(&mut self) -> Result<u64, StoreError> {
		Ok(self.audit_tip().await?.map_or(0, |tip| tip.sequence))
	}

	/// The newest record's epoch, position, and chain link, or `None`
	/// before the audit's first record.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn audit_tip(&mut self) -> Result<Option<AuditTip>, StoreError> {
		let row = sqlx::query!(
			r#"SELECT sequence AS "sequence!", epoch, entry_hash
			 FROM security_audit
			 ORDER BY sequence DESC
			 LIMIT 1"#
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(|row| {
			Ok(AuditTip {
				epoch: parse_counter(row.epoch)?,
				sequence: parse_counter(row.sequence)?,
				entry_hash: AuditEntryHash::from_column(&row.entry_hash)?,
			})
		})
		.transpose()
	}

	/// The record retention last removed, or `None` while the whole chain
	/// is still retained.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub(crate) async fn audit_retention_anchor(
		&mut self,
	) -> Result<Option<RetentionAnchor>, StoreError> {
		let row = sqlx::query!(
			"SELECT retained_after_epoch, retained_after_sequence,
				retained_after_hash
			 FROM audit_state WHERE singleton = 1"
		)
		.fetch_one(self.connection())
		.await?;
		let Some(hash) = row.retained_after_hash else {
			return Ok(None);
		};
		Ok(Some(RetentionAnchor {
			epoch: parse_counter(row.retained_after_epoch)?,
			sequence: parse_counter(row.retained_after_sequence)?,
			entry_hash: AuditEntryHash::from_column(&hash)?,
		}))
	}
}

fn read_row(row: Row) -> Result<AuditRecord, StoreError> {
	let risk = AuditRisk::parse(&row.risk).ok_or_else(|| {
		StoreError::Integrity(format!("unknown audit risk {:?}", row.risk))
	})?;
	let outcome = AuditOutcome::parse(&row.outcome).ok_or_else(|| {
		StoreError::Integrity(format!(
			"unknown audit outcome {:?}",
			row.outcome
		))
	})?;
	Ok(AuditRecord {
		sequence: parse_counter(row.sequence)?,
		epoch: parse_counter(row.epoch)?,
		record_id: parse_uuid("record_id", &row.record_id)?,
		recorded_at_unix_ms: row.recorded_at_unix_ms,
		plane_id: parse_uuid("plane_id", &row.plane_id)?,
		actor: ActorRecord::parse(&row.actor_kind, &row.actor_id)?,
		target_kind: row.target_kind,
		target_reference: AuditTargetRef::from_column(&row.target_reference)?,
		target_id: row.target_id,
		decision: row.decision,
		risk,
		outcome,
		entry_hash: AuditEntryHash::from_column(&row.entry_hash)?,
	})
}
