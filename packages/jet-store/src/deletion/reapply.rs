//! Removing from the store every identity the ledger says is gone. It runs
//! at every open of an authoritative store, which is how a restored
//! snapshot and a store that crashed between the ledger and its own
//! commit both catch up (ADR-0102).

use crate::{
	StoreError,
	deletion::{DeletedIdentityKind, DeletionRecord},
};
use sqlx::SqlitePool;

/// Removes the identity each record names, in one transaction, and
/// returns how many rows that took: zero when the store already agreed
/// with the ledger. Only the identity's own row is removed; a restoration
/// brings back the old journal and queues regardless, and the workers
/// that read them find the identity gone.
pub(crate) async fn reapply(
	pool: &SqlitePool,
	records: &[DeletionRecord],
) -> Result<u64, StoreError> {
	if records.is_empty() {
		return Ok(0);
	}
	let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
	let mut removed = 0;
	for record in records {
		let identity = record.identity.to_string();
		let result = match record.kind {
			DeletedIdentityKind::AccountBinding => {
				sqlx::query!(
					"DELETE FROM account_bindings WHERE binding_id = ?1",
					identity
				)
				.execute(&mut *transaction)
				.await?
			}
			DeletedIdentityKind::PairedClient => {
				sqlx::query!(
					"DELETE FROM paired_clients WHERE client_id = ?1",
					identity
				)
				.execute(&mut *transaction)
				.await?
			}
			DeletedIdentityKind::Schedule => {
				sqlx::query!(
					"DELETE FROM scheduled_tasks WHERE schedule_id = ?1",
					identity
				)
				.execute(&mut *transaction)
				.await?
			}
		};
		removed += result.rows_affected();
	}
	transaction.commit().await?;
	Ok(removed)
}
