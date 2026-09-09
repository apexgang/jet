//! Core-owned native lifecycle documents stored beside their durable Effects.
use crate::{ReadTransaction, StoreError, WriteTransaction};
use uuid::Uuid;
impl ReadTransaction {
	/// Read one lifecycle document. Returns a store error if the read fails.
	pub async fn extension_change(
		&mut self,
		id: Uuid,
	) -> Result<Option<String>, StoreError> {
		let id = id.to_string();
		Ok(sqlx::query_scalar!(
			"SELECT document FROM extension_changes WHERE change_id = ?1",
			id
		)
		.fetch_optional(self.connection())
		.await?)
	}
}
impl WriteTransaction {
	/// Commit a native lifecycle document. Returns a store error if the write fails.
	pub async fn save_extension_change(
		&mut self,
		id: Uuid,
		document: &str,
	) -> Result<(), StoreError> {
		let id = id.to_string();
		sqlx::query!("INSERT INTO extension_changes (change_id, document) VALUES (?1, ?2) ON CONFLICT(change_id) DO UPDATE SET document = excluded.document", id, document).execute(self.connection()).await?;
		Ok(())
	}
}
