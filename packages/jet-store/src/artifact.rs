//! Transactional references to verified payloads outside SQLite.
use crate::{ReadTransaction, StoreError, WriteTransaction};
use uuid::Uuid;

impl ReadTransaction {
	/// Finds the declared size of a published Artifact.
	/// Returns a store error when the read fails.
	pub async fn artifact_size(
		&mut self,
		sha256: &str,
	) -> Result<Option<i64>, StoreError> {
		Ok(sqlx::query_scalar!(
			"SELECT size FROM artifact_references WHERE sha256 = ?1 LIMIT 1",
			sha256
		)
		.fetch_optional(self.connection())
		.await?)
	}

	/// Tests a collection candidate against every durable reference.
	/// Returns a store error when the read fails.
	pub async fn artifact_referenced(
		&mut self,
		sha256: &str,
	) -> Result<bool, StoreError> {
		Ok(self.artifact_size(sha256).await?.is_some())
	}
}

impl WriteTransaction {
	/// Adds a verified Run reference beside its Event; returns whether it is new.
	/// Returns a store error for an invalid reference or failed write.
	pub async fn reference_artifact(
		&mut self,
		run_id: Uuid,
		sha256: &str,
		size: i64,
	) -> Result<bool, StoreError> {
		let run_id = run_id.to_string();
		// ASVS 1.2.4, 2.3.3: bound values and the caller's durable transaction.
		Ok(sqlx::query!("INSERT INTO artifact_references (run_id, sha256, size) VALUES (?1, ?2, ?3) ON CONFLICT (run_id, sha256) DO NOTHING", run_id, sha256, size)
            .execute(self.connection()).await?.rows_affected() != 0)
	}
}
