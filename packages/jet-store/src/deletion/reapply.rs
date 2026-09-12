//! Removing from the store every identity the ledger says is gone. It runs
//! at every open of an authoritative store, which is how a restored
//! snapshot and a store that crashed between the ledger and its own
//! commit both catch up (ADR-0102).

use crate::{
	StoreError,
	deletion::{DeletedIdentityKind, DeletionRecord},
	plane,
};
use sqlx::SqlitePool;

/// Removes the identity each record names and records the ledger as
/// applied, in one transaction, and returns how many rows that changed:
/// zero when the store already agreed with the ledger. Only the
/// identity's own row is removed; a restoration brings back the old
/// journal and queues regardless.
pub(crate) async fn reapply(
	pool: &SqlitePool,
	records: &[DeletionRecord],
) -> Result<u64, StoreError> {
	let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
	let mut changed = 0;
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
		changed += result.rows_affected();
	}
	let vouched = u64::try_from(records.len()).unwrap_or(u64::MAX);
	// A store behind the ledger catches up; one ahead of it is left
	// saying so, which is how a lost ledger is noticed.
	if plane::deletions_applied(&mut *transaction).await? < vouched {
		plane::record_deletions_applied(&mut *transaction, vouched).await?;
		changed += 1;
	}
	transaction.commit().await?;
	Ok(changed)
}
