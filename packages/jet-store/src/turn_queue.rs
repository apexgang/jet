//! Conversation-owned queue projections, committed with Commands and Events.
use crate::{ReadTransaction, StoreError, WriteTransaction};
use uuid::Uuid;

impl ReadTransaction {
	/// Reads a bounded page of queues with pending work, without polling idle queues.
	///
	/// # Errors
	/// Returns a store error on query or identity decoding failure.
	pub async fn pending_turn_queues(
		&mut self,
		after: &str,
	) -> Result<Vec<Uuid>, StoreError> {
		let rows = sqlx::query_scalar!(r#"SELECT conversation_id AS "conversation_id!" FROM turn_queues WHERE pending_count > 0 AND conversation_id > ?1 ORDER BY conversation_id LIMIT 100"#, after).fetch_all(self.connection()).await?;
		rows.iter()
			.map(|id| crate::records::parse_uuid("conversation_id", id))
			.collect()
	}
	/// Reads the core-owned queue document in this snapshot.
	///
	/// # Errors
	/// Returns a store error when the query fails.
	pub async fn turn_queue(
		&mut self,
		conversation_id: Uuid,
	) -> Result<Option<String>, StoreError> {
		let id = conversation_id.to_string();
		Ok(sqlx::query_scalar!(
			"SELECT state FROM turn_queues WHERE conversation_id = ?1",
			id
		)
		.fetch_optional(self.connection())
		.await?)
	}
}
impl WriteTransaction {
	/// Stores the bounded queue alongside its admission receipt and Events.
	///
	/// # Errors
	/// Returns a store error for unknown Conversations, invalid JSON or failed writes.
	pub async fn save_turn_queue(
		&mut self,
		conversation_id: Uuid,
		state: &str,
		pending_count: u32,
	) -> Result<(), StoreError> {
		let id = conversation_id.to_string();
		let pending_count = i64::from(pending_count);
		// ASVS 1.2.4: both the identity and content are bound SQL parameters.
		sqlx::query!("INSERT INTO turn_queues (conversation_id, state, pending_count) VALUES (?1, ?2, ?3) ON CONFLICT(conversation_id) DO UPDATE SET state = excluded.state, pending_count = excluded.pending_count", id, state, pending_count)
			.execute(self.connection()).await?;
		Ok(())
	}
}
