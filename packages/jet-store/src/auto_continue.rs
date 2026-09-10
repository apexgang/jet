//! Account-binding defaults for Auto-continue; retry state belongs to the queue.
use crate::{ReadTransaction, StoreError, WriteTransaction};
use uuid::Uuid;

impl ReadTransaction {
	/// Read the core-owned policy document for one Account binding.
	/// # Errors
	/// Returns a store error if the query fails.
	pub async fn auto_continue_policy(
		&mut self,
		binding_id: Uuid,
	) -> Result<Option<String>, StoreError> {
		let id = binding_id.to_string();
		Ok(sqlx::query_scalar!(
			"SELECT policy FROM auto_continue_policies WHERE binding_id = ?1",
			id
		)
		.fetch_optional(self.connection())
		.await?)
	}
}
impl WriteTransaction {
	/// Replace a binding's policy in the Command transaction.
	/// # Errors
	/// Returns a store error for an unknown binding or a failed write.
	pub async fn save_auto_continue_policy(
		&mut self,
		binding_id: Uuid,
		policy: &str,
	) -> Result<(), StoreError> {
		let id = binding_id.to_string();
		// ASVS 1.2.4: policy content is bound data, never SQL source.
		sqlx::query!("INSERT INTO auto_continue_policies (binding_id, policy) VALUES (?1, ?2) ON CONFLICT(binding_id) DO UPDATE SET policy = excluded.policy", id, policy).execute(self.connection()).await?;
		Ok(())
	}
}
