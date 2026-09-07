//! Immutable, versioned checkpoint payloads; orchestration belongs to Core.
use crate::{ReadTransaction, StoreError, WriteTransaction};
use uuid::Uuid;

impl ReadTransaction {
	/// Reads one immutable turn checkpoint.
	///
	/// # Errors
	/// Returns a store error if the Query cannot complete.
	pub async fn change_checkpoint(
		&mut self,
		run_id: Uuid,
		turn: u32,
	) -> Result<Option<String>, StoreError> {
		let id = run_id.to_string();
		let turn = i64::from(turn);
		Ok(sqlx::query_scalar!(
			"SELECT payload FROM change_checkpoints WHERE run_id = ?1 AND turn = ?2",
			id,
			turn
		)
		.fetch_optional(self.connection())
		.await?)
	}
}
impl WriteTransaction {
	/// Inserts a turn checkpoint beside its source receipt and lifecycle update.
	///
	/// # Errors
	/// Rejects duplicate turns, missing Runs, or invalid payloads.
	pub async fn insert_change_checkpoint(
		&mut self,
		run_id: Uuid,
		turn: u32,
		payload: &str,
	) -> Result<(), StoreError> {
		let id = run_id.to_string();
		let turn = i64::from(turn);
		// ASVS 1.2.4, 2.3.3: bound parameters inside the caller's transaction.
		sqlx::query!(
			"INSERT INTO change_checkpoints (run_id, turn, payload) VALUES (?1, ?2, ?3)",
			id,
			turn,
			payload
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
}
