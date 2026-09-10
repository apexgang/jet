//! Durable Orphaned executions and interactive resolution requests.
use crate::{ReadTransaction, StoreError, WriteTransaction};
use uuid::Uuid;

impl ReadTransaction {
	/// Reads a bounded keyset page of Orphaned execution metadata.
	///
	/// # Errors
	/// Returns a store error when the page cannot be read.
	pub async fn orphaned_executions(
		&mut self,
		after: &str,
	) -> Result<Vec<String>, StoreError> {
		Ok(sqlx::query_scalar!("SELECT state FROM orphaned_executions WHERE execution_id > ?1 ORDER BY execution_id LIMIT 101", after)
            .fetch_all(self.connection()).await?)
	}
	/// Reads one persisted Orphaned execution.
	///
	/// # Errors
	/// Returns a store error when the query fails.
	pub async fn orphaned_execution(
		&mut self,
		id: Uuid,
	) -> Result<Option<String>, StoreError> {
		let id = id.to_string();
		Ok(sqlx::query_scalar!(
			"SELECT state FROM orphaned_executions WHERE execution_id = ?1",
			id
		)
		.fetch_optional(self.connection())
		.await?)
	}
	/// Reads the immutable interactive request behind a resolution Effect.
	///
	/// # Errors
	/// Returns a store error when the query fails.
	pub async fn execution_resolution(
		&mut self,
		id: Uuid,
	) -> Result<Option<String>, StoreError> {
		let id = id.to_string();
		Ok(sqlx::query_scalar!(
			"SELECT request FROM execution_resolutions WHERE effect_id = ?1",
			id
		)
		.fetch_optional(self.connection())
		.await?)
	}
}
impl WriteTransaction {
	/// Records or refreshes read-only identity while preserving Orphaned status.
	///
	/// # Errors
	/// Returns a store error on invalid metadata or failed writes.
	pub async fn insert_orphaned_execution(
		&mut self,
		id: Uuid,
		state: &str,
	) -> Result<(), StoreError> {
		let id = id.to_string();
		sqlx::query!("INSERT INTO orphaned_executions (execution_id, state) VALUES (?1, ?2) ON CONFLICT(execution_id) DO UPDATE SET state = excluded.state", id, state).execute(self.connection()).await?;
		Ok(())
	}
	/// Clears an Orphaned execution after an explicit resolution succeeded.
	///
	/// # Errors
	/// Returns a store error when the write fails.
	pub async fn remove_orphaned_execution(
		&mut self,
		id: Uuid,
	) -> Result<(), StoreError> {
		let id = id.to_string();
		sqlx::query!(
			"DELETE FROM orphaned_executions WHERE execution_id = ?1",
			id
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
	/// Pins an interactive request beside its Effect and Command receipt.
	///
	/// # Errors
	/// Returns a store error when the request cannot be recorded.
	pub async fn insert_execution_resolution(
		&mut self,
		id: Uuid,
		request: &str,
	) -> Result<(), StoreError> {
		let id = id.to_string();
		sqlx::query!(
			"INSERT INTO execution_resolutions (effect_id, request) VALUES (?1, ?2)",
			id,
			request
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
}
