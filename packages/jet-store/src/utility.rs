//! Bounded, core-owned Utility documents, committed alongside their Effects.
use crate::{ReadTransaction, StoreError, WriteTransaction};
use uuid::Uuid;
impl ReadTransaction {
	/// Read one retained Utility job. Returns a store error on failure.
	pub async fn utility_job(
		&mut self,
		id: Uuid,
	) -> Result<Option<String>, StoreError> {
		let id = id.to_string();
		Ok(sqlx::query_scalar!(
			"SELECT state FROM utility_jobs WHERE job_id = ?1",
			id
		)
		.fetch_optional(self.connection())
		.await?)
	}
}
impl WriteTransaction {
	/// Store a bounded Utility document in the initiating or settling transaction.
	/// Returns a store error on failure.
	pub async fn save_utility_job(
		&mut self,
		id: Uuid,
		state: &str,
	) -> Result<(), StoreError> {
		let id = id.to_string();
		sqlx::query!("INSERT INTO utility_jobs (job_id, state) VALUES (?1, ?2) ON CONFLICT(job_id) DO UPDATE SET state = excluded.state", id, state).execute(self.connection()).await?;
		Ok(())
	}
}
