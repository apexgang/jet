//! Core-owned Git delivery documents and per-Conversation ordering.
use crate::{ReadTransaction, StoreError, WriteTransaction};
use uuid::Uuid;
impl ReadTransaction {
	/// Read the branch explicitly created for this Conversation.
	pub async fn git_branch(
		&mut self,
		conversation_id: Uuid,
	) -> Result<Option<String>, StoreError> {
		let id = conversation_id.to_string();
		Ok(sqlx::query_scalar!(
			"SELECT branch FROM git_branches WHERE conversation_id = ?1",
			id
		)
		.fetch_optional(self.connection())
		.await?)
	}
	/// Read the Conversation's hosted draft binding across every Run.
	pub async fn git_draft(
		&mut self,
		conversation_id: Uuid,
	) -> Result<Option<String>, StoreError> {
		let id = conversation_id.to_string();
		Ok(sqlx::query_scalar!(
			"SELECT url FROM git_drafts WHERE conversation_id = ?1",
			id
		)
		.fetch_optional(self.connection())
		.await?)
	}
	/// Read one delivery document. Returns a store error on failure.
	pub async fn git_delivery(
		&mut self,
		id: Uuid,
	) -> Result<Option<String>, StoreError> {
		let id = id.to_string();
		Ok(sqlx::query_scalar!(
			"SELECT state FROM git_deliveries WHERE delivery_id = ?1",
			id
		)
		.fetch_optional(self.connection())
		.await?)
	}
	/// Read the newest bounded delivery history for one Conversation.
	pub async fn git_deliveries(
		&mut self,
		conversation_id: Uuid,
	) -> Result<Vec<String>, StoreError> {
		let id = conversation_id.to_string();
		Ok(sqlx::query_scalar!("SELECT state FROM git_deliveries WHERE conversation_id = ?1 ORDER BY delivery_id DESC LIMIT 100", id).fetch_all(self.connection()).await?)
	}
	/// Pending and uncertain delivery blocks new mutations and native turns.
	pub async fn git_delivery_blocks(
		&mut self,
		conversation_id: Uuid,
	) -> Result<bool, StoreError> {
		let id = conversation_id.to_string();
		Ok(sqlx::query_scalar!("SELECT EXISTS(SELECT 1 FROM git_deliveries g JOIN effects e ON e.effect_id = g.delivery_id JOIN conversations owner ON owner.conversation_id = g.conversation_id JOIN conversations target ON target.conversation_id = ?1 WHERE (owner.conversation_id = target.conversation_id OR (owner.working_tree = 'local_checkout' AND target.working_tree = 'local_checkout' AND owner.project_id = target.project_id)) AND (e.state IN ('pending', 'in_flight') OR (e.state = 'outcome_unknown' AND g.acknowledged = 0)))", id).fetch_one(self.connection()).await? != 0)
	}
}
impl WriteTransaction {
	/// Bind successful branch creation independently of bounded delivery history.
	pub async fn save_git_branch(
		&mut self,
		conversation_id: Uuid,
		branch: &str,
	) -> Result<(), StoreError> {
		let id = conversation_id.to_string();
		sqlx::query!("INSERT INTO git_branches (conversation_id, branch) VALUES (?1, ?2) ON CONFLICT(conversation_id) DO UPDATE SET branch = excluded.branch", id, branch).execute(self.connection()).await?;
		Ok(())
	}
	/// Acknowledge uncertainty without changing or retrying its Git Effect.
	pub async fn acknowledge_git_delivery(
		&mut self,
		id: Uuid,
	) -> Result<(), StoreError> {
		let id = id.to_string();
		sqlx::query!(
			"UPDATE git_deliveries SET acknowledged = 1 WHERE delivery_id = ?1",
			id
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
	/// Retain the observed draft binding with its completed Effect.
	pub async fn save_git_draft(
		&mut self,
		conversation_id: Uuid,
		url: &str,
	) -> Result<(), StoreError> {
		let id = conversation_id.to_string();
		sqlx::query!("INSERT INTO git_drafts (conversation_id, url) VALUES (?1, ?2) ON CONFLICT(conversation_id) DO UPDATE SET url = excluded.url", id, url).execute(self.connection()).await?;
		Ok(())
	}
	/// Persist a bounded operation in the admission or settlement transaction.
	pub async fn save_git_delivery(
		&mut self,
		id: Uuid,
		conversation_id: Uuid,
		state: &str,
	) -> Result<(), StoreError> {
		let id = id.to_string();
		let conversation_id = conversation_id.to_string();
		sqlx::query!("INSERT INTO git_deliveries (delivery_id, conversation_id, state) VALUES (?1, ?2, ?3) ON CONFLICT(delivery_id) DO UPDATE SET state = excluded.state", id, conversation_id, state).execute(self.connection()).await?;
		Ok(())
	}
}
