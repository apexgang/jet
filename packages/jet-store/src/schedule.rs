//! Indexed deadlines with core-owned schedule documents.
use crate::{ReadTransaction, StoreError, WriteTransaction};
use uuid::Uuid;
impl ReadTransaction {
	/// The earliest indexed schedule deadline, without scanning Conversations.
	/// # Errors
	/// Returns a store error if the deadline cannot be read.
	pub async fn next_schedule_deadline(
		&mut self,
	) -> Result<Option<i64>, StoreError> {
		Ok(sqlx::query_scalar!(
			"SELECT MIN(next_due_unix_ms) FROM scheduled_tasks"
		)
		.fetch_one(self.connection())
		.await?)
	}
	/// Read enabled schedules in a Conversation. Creation bounds this to 32.
	/// # Errors
	/// Returns a store error if the query fails.
	pub async fn scheduled_tasks(
		&mut self,
		conversation_id: Uuid,
	) -> Result<Vec<String>, StoreError> {
		let id = conversation_id.to_string();
		Ok(sqlx::query_scalar!("SELECT state FROM scheduled_tasks WHERE conversation_id = ?1 ORDER BY schedule_id", id).fetch_all(self.connection()).await?)
	}
	/// Read a schedule by its immutable identity.
	/// # Errors
	/// Returns a store error if the query fails.
	pub async fn scheduled_task(
		&mut self,
		schedule_id: Uuid,
	) -> Result<Option<String>, StoreError> {
		let id = schedule_id.to_string();
		Ok(sqlx::query_scalar!(
			"SELECT state FROM scheduled_tasks WHERE schedule_id = ?1",
			id
		)
		.fetch_optional(self.connection())
		.await?)
	}
	/// Read a bounded deadline page. Idle ticks read only the due index.
	/// # Errors
	/// Returns a store error if the query fails.
	pub async fn due_schedules(
		&mut self,
		now: i64,
		after: &str,
	) -> Result<Vec<String>, StoreError> {
		Ok(sqlx::query_scalar!("SELECT state FROM scheduled_tasks WHERE next_due_unix_ms <= ?1 AND schedule_id > ?2 ORDER BY schedule_id LIMIT 100", now, after).fetch_all(self.connection()).await?)
	}
}
impl WriteTransaction {
	/// Commit the next selected instant alongside firing Events and Turn admission.
	/// # Errors
	/// Returns a store error if the write fails.
	pub async fn save_schedule(
		&mut self,
		schedule_id: Uuid,
		conversation_id: Uuid,
		next_due: i64,
		state: &str,
	) -> Result<(), StoreError> {
		let id = schedule_id.to_string();
		let conversation_id = conversation_id.to_string();
		sqlx::query!("INSERT INTO scheduled_tasks (schedule_id, conversation_id, next_due_unix_ms, state) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(schedule_id) DO UPDATE SET next_due_unix_ms = excluded.next_due_unix_ms, state = excluded.state", id, conversation_id, next_due, state).execute(self.connection()).await?;
		Ok(())
	}
	/// Remove an enabled schedule, releasing its cleanup protection.
	/// # Errors
	/// Returns a store error if the write fails.
	pub async fn delete_schedule(
		&mut self,
		schedule_id: Uuid,
	) -> Result<(), StoreError> {
		let id = schedule_id.to_string();
		sqlx::query!("DELETE FROM scheduled_tasks WHERE schedule_id = ?1", id)
			.execute(self.connection())
			.await?;
		Ok(())
	}
}
