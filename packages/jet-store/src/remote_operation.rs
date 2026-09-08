//! Durable identities for external No-Visa work, including uncertain outcomes.
use crate::{ReadTransaction, StoreError, WriteTransaction};

/// One reserved operation, scoped to its paired installation.
#[derive(Debug, Clone)]
pub struct RemoteOperationRecord {
	/// Origin metadata retained without action content after payload expiry.
	pub origin_plane_id: String,
	/// Origin-owned Conversation identity.
	pub origin_conversation_id: String,
	/// Origin Harness execution identity.
	pub origin_run_id: String,
	/// Registered destination scope.
	pub workspace_id: String,
	/// Authenticated installation identity.
	pub client_id: String,
	/// Stable operation identity.
	pub operation_id: String,
	/// Digest that prevents changing a reserved request.
	pub request_digest: Vec<u8>,
	/// Admission time, used for payload retention.
	pub recorded_at_unix_ms: i64,
	/// Bounded request retained for exact-action review, cleared after expiry.
	pub request: Option<String>,
	/// Domain-owned state spelling.
	pub state: String,
	/// Completed bounded result, absent while the outcome is uncertain.
	pub result: Option<String>,
}

impl ReadTransaction {
	/// Looks up an operation without executing it. Returns query errors.
	pub async fn remote_operation(
		&mut self,
		client_id: &str,
		operation_id: &str,
	) -> Result<Option<RemoteOperationRecord>, StoreError> {
		Ok(sqlx::query_as!(RemoteOperationRecord,
            "SELECT client_id, operation_id, request_digest, recorded_at_unix_ms, origin_plane_id, origin_conversation_id, origin_run_id, workspace_id, request, state, result FROM remote_operations WHERE client_id = ?1 AND operation_id = ?2", client_id, operation_id)
            .fetch_optional(self.connection()).await?)
	}
}

impl WriteTransaction {
	/// Discards stale approval payloads after ten minutes. No effect was started.
	/// Returns query errors.
	pub async fn expire_remote_reviews(
		&mut self,
		before: i64,
	) -> Result<(), StoreError> {
		sqlx::query!("UPDATE remote_operations SET state = 'denied', request = NULL WHERE recorded_at_unix_ms < ?1 AND state IN ('pending', 'approved')", before).execute(self.connection()).await?;
		Ok(())
	}
	/// Revokes unused approvals and pending requests in the Pairing transaction.
	/// Returns query errors; running effects stop through their live session.
	pub async fn invalidate_remote_operations(
		&mut self,
		client_id: &str,
	) -> Result<(), StoreError> {
		sqlx::query!("UPDATE remote_operations SET state = 'denied', request = NULL WHERE client_id = ?1 AND state IN ('pending', 'approved')", client_id)
            .execute(self.connection()).await?;
		Ok(())
	}
	/// Changes exactly one expected operation state. Returns whether it changed.
	pub async fn transition_remote_operation(
		&mut self,
		client_id: &str,
		operation_id: &str,
		from: &str,
		to: &str,
	) -> Result<bool, StoreError> {
		Ok(sqlx::query!("UPDATE remote_operations SET state = ?4 WHERE client_id = ?1 AND operation_id = ?2 AND state = ?3", client_id, operation_id, from, to)
            .execute(self.connection()).await?.rows_affected() == 1)
	}
	/// Reserves an identity before any external effect. Returns constraint or query errors.
	pub async fn insert_remote_operation(
		&mut self,
		record: &RemoteOperationRecord,
	) -> Result<(), StoreError> {
		// ASVS 1.2.4: all peer data are bound parameters.
		sqlx::query!("INSERT INTO remote_operations (client_id, operation_id, request_digest, recorded_at_unix_ms, request, state, result, origin_plane_id, origin_conversation_id, origin_run_id, workspace_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            record.client_id, record.operation_id, record.request_digest, record.recorded_at_unix_ms, record.request, record.state, record.result, record.origin_plane_id, record.origin_conversation_id, record.origin_run_id, record.workspace_id)
            .execute(self.connection()).await?;
		Ok(())
	}
	/// Records completion after external work. Returns query errors.
	pub async fn finish_remote_operation(
		&mut self,
		client_id: &str,
		operation_id: &str,
		result: &str,
	) -> Result<(), StoreError> {
		sqlx::query!("UPDATE remote_operations SET state = 'finished', result = ?3, request = NULL WHERE client_id = ?1 AND operation_id = ?2 AND state = 'running'", client_id, operation_id, result)
            .execute(self.connection()).await?;
		Ok(())
	}
	/// Expires payloads after thirty days while retaining the identity barrier.
	/// Returns query errors.
	pub async fn expire_remote_operations(
		&mut self,
		before: i64,
	) -> Result<(), StoreError> {
		sqlx::query!("UPDATE remote_operations SET state = 'expired', request = NULL, result = NULL WHERE recorded_at_unix_ms < ?1 AND state <> 'expired'", before)
            .execute(self.connection()).await?;
		Ok(())
	}
}
