//! Keeping the Security audit no longer than it is meant to be kept, and
//! forgetting what a deleted target was called (ADR-0105).
//!
//! Both operations remove information from an append-only chain without
//! breaking it, in the two ways that are possible.
//!
//! Retention removes a prefix. The link the removed records folded to is
//! kept as the anchor the remaining chain continues from, so what is left
//! still folds through the durable head. The record that head names is
//! never removed, because nothing would be left to validate against.
//!
//! Anonymization removes an identity. A target enters the chain as an
//! opaque reference derived from it, never as its own identity, so the
//! identity beside that reference can be cleared and every link stays
//! exactly what it was. What remains says that some Conversation was
//! deleted and which records were about the same one, and nothing else.

use crate::audit_chain::target_reference;
use crate::audit_epoch::{parse_entry_hash, parse_sequence, sequence_column};
use crate::audit_head::{self, AuditHead};
use crate::transaction::WriteTransaction;
use crate::{Store, StoreError};

/// Most records one retention transaction removes. Retention repeats until
/// nothing is left to remove, so a long-idle Plane catches up without
/// holding the write lock for the whole backlog.
pub const AUDIT_RETENTION_BATCH_LIMIT: usize = 256;

impl Store {
	/// Removes Security audit records recorded before `cutoff_unix_ms`,
	/// repeating in bounded transactions until none are left.
	///
	/// The record the durable head names always survives: the head is what
	/// the remaining chain is validated against, so removing it would turn
	/// retention into the tampering it is supposed to make visible.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the head cannot be read or a batch
	/// cannot be committed.
	pub async fn prune_audit_before(
		&self,
		cutoff_unix_ms: i64,
	) -> Result<usize, StoreError> {
		let Some(head) = audit_head::read(&self.database, self.plane_id)?
		else {
			return Ok(0);
		};
		let mut removed = 0;
		loop {
			let batch = self
				.write(async |tx| tx.prune_audit(head, cutoff_unix_ms).await)
				.await?;
			if batch == 0 {
				return Ok(removed);
			}
			removed += batch;
		}
	}
}

impl WriteTransaction {
	/// Forgets what the target named by `kind` and `id` was called,
	/// wherever the Security audit recorded a decision about it, and
	/// returns how many records that was.
	///
	/// The count is what a deletion preview discloses: the audit keeps
	/// content-free metadata about a deleted Conversation until the records
	/// themselves expire, and the person deleting it is told so (ADR-0105).
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read or updated.
	pub async fn anonymize_audit_target(
		&mut self,
		kind: &str,
		id: &str,
	) -> Result<usize, StoreError> {
		let plane_id = self.plane().await?.plane_id;
		let reference = target_reference(plane_id, kind, Some(id)).0.to_vec();
		// ASVS 1.2.4: the reference is a parameter, never assembled SQL.
		let anonymized = sqlx::query!(
			"UPDATE security_audit SET target_id = NULL
			 WHERE target_reference = ?1 AND target_id IS NOT NULL",
			reference
		)
		.execute(self.connection())
		.await?
		.rows_affected();
		Ok(usize::try_from(anonymized).unwrap_or(usize::MAX))
	}

	/// Removes one bounded batch of expired records and moves the anchor
	/// the remaining chain is validated from.
	async fn prune_audit(
		&mut self,
		head: AuditHead,
		cutoff_unix_ms: i64,
	) -> Result<usize, StoreError> {
		let keep_from = sequence_column(head.sequence)?;
		let batch =
			i64::try_from(AUDIT_RETENTION_BATCH_LIMIT).unwrap_or(i64::MAX);
		// The eligible records are the unbroken run of expired ones from
		// the oldest onwards. A record with a later stamp sitting among
		// them stops the run, because removing around it would leave a hole
		// the chain could not be folded across.
		let last_removed = sqlx::query_scalar!(
			r#"SELECT MAX(sequence) AS "last_removed?: i64" FROM (
				SELECT sequence FROM security_audit
				WHERE sequence < ?1 AND recorded_at_unix_ms < ?2
					AND sequence < COALESCE((
						SELECT MIN(sequence) FROM security_audit
						WHERE sequence < ?1
							AND recorded_at_unix_ms >= ?2
					), ?1)
				ORDER BY sequence
				LIMIT ?3
			)"#,
			keep_from,
			cutoff_unix_ms,
			batch
		)
		.fetch_one(self.connection())
		.await?;
		let Some(last_removed) = last_removed else {
			return Ok(0);
		};
		let anchor = sqlx::query!(
			"SELECT epoch, entry_hash FROM security_audit WHERE sequence = ?1",
			last_removed
		)
		.fetch_one(self.connection())
		.await?;
		let epoch = parse_sequence(anchor.epoch)?;
		let entry_hash = parse_entry_hash(&anchor.entry_hash)?;
		let removed = sqlx::query!(
			"DELETE FROM security_audit WHERE sequence <= ?1",
			last_removed
		)
		.execute(self.connection())
		.await?
		.rows_affected();
		let epoch_column = sequence_column(epoch)?;
		let hash_column = entry_hash.0.to_vec();
		sqlx::query!(
			"UPDATE audit_state
			 SET retained_after_epoch = ?1, retained_after_sequence = ?2,
				retained_after_hash = ?3
			 WHERE singleton = 1",
			epoch_column,
			last_removed,
			hash_column
		)
		.execute(self.connection())
		.await?;
		Ok(usize::try_from(removed).unwrap_or(usize::MAX))
	}
}
