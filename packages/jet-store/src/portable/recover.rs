//! Validates an imported schema before creating inert Recovered identities.
use crate::{Store, StoreError};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::path::Path;
use uuid::Uuid;

/// Identity mapping and disabled schedule count in a staged SQLite copy.
#[derive(Debug)]
pub struct RecoveredSnapshot {
	/// Source and fresh Conversation identities, in source creation order.
	pub conversations: Vec<(Uuid, Uuid)>,
	/// Schedules retained only as disabled metadata.
	pub disabled_schedules: u64,
}
impl Store {
	/// Verifies a private imported snapshot against this build's schema and
	/// rewrites it as a non-authoritative Recovered copy. Never opens it as a Store.
	/// Returns an integrity error on incompatible or malformed input.
	pub async fn recover_portable_snapshot(
		&self,
		path: &Path,
	) -> Result<RecoveredSnapshot, StoreError> {
		let mut connection = SqliteConnection::connect_with(
			&SqliteConnectOptions::new()
				.filename(path)
				.foreign_keys(false)
				.pragma("trusted_schema", "OFF"),
		)
		.await?;
		// Bootstrap schema reads cannot use the compiled live-store schema cache.
		const SCHEMA: &str = "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name";
		let expected: Vec<(String, String, String, Option<String>)> =
			sqlx::query_as(SCHEMA).fetch_all(&self.pool()).await?;
		let actual: Vec<(String, String, String, Option<String>)> =
			sqlx::query_as(SCHEMA).fetch_all(&mut connection).await?;
		if actual != expected {
			return Err(StoreError::Integrity(
				"incompatible Recovery schema".into(),
			));
		}
		// Only the exact known schema may execute its FTS maintenance triggers.
		sqlx::query("PRAGMA trusted_schema = ON")
			.execute(&mut connection)
			.await?;
		super::validate::validate(&mut connection).await?;
		super::sanitize::sanitize(&mut connection).await?;
		let sources = sqlx::query_scalar!(r#"SELECT conversation_id AS "conversation_id!" FROM conversations ORDER BY rowid"#)
            .fetch_all(&mut connection).await?;
		let mut conversations = Vec::new();
		for source in sources {
			let source_id = Uuid::parse_str(&source).map_err(|_| {
				StoreError::Integrity("invalid Conversation identity".into())
			})?;
			let recovered = Uuid::new_v4();
			let target = recovered.to_string();
			sqlx::query!(
				"UPDATE conversations SET conversation_id = ?1 WHERE conversation_id = ?2",
				target,
				source
			)
			.execute(&mut connection)
			.await?;
			sqlx::query!(
				"UPDATE runs SET conversation_id = ?1 WHERE conversation_id = ?2",
				target,
				source
			)
			.execute(&mut connection)
			.await?;
			sqlx::query!(
				"UPDATE events SET conversation_id = ?1 WHERE conversation_id = ?2",
				target,
				source
			)
			.execute(&mut connection)
			.await?;
			sqlx::query!("UPDATE settings SET scope_id = ?1 WHERE scope = 'conversation' AND scope_id = ?2", target, source).execute(&mut connection).await?;
			sqlx::query!("UPDATE scheduled_tasks SET conversation_id = ?1, state = json_set(state, '$.conversation_id', ?1) WHERE conversation_id = ?2", target, source).execute(&mut connection).await?;
			conversations.push((source_id, recovered));
		}
		// There is no runnable execution or authorized schedule in a Recovered copy.
		sqlx::query!("UPDATE runs SET lifecycle = 'lost', ended_at_unix_ms = COALESCE(ended_at_unix_ms, created_at_unix_ms) WHERE lifecycle IN ('created', 'starting', 'active', 'stopping')").execute(&mut connection).await?;
		let count = sqlx::query_scalar!("SELECT COUNT(*) FROM scheduled_tasks")
			.fetch_one(&mut connection)
			.await?;
		// ASVS 8.3.1: Store::open refuses this marker, so even an accidental path
		// mix-up cannot turn the recovered file into a Home Plane.
		sqlx::query("PRAGMA application_id = 1246057554")
			.execute(&mut connection)
			.await?;
		sqlx::query("VACUUM").execute(&mut connection).await?;
		super::validate::validate(&mut connection).await?;
		connection.close().await?;
		crate::snapshot::verify(path).await?;
		Ok(RecoveredSnapshot {
			conversations,
			disabled_schedules: count as u64,
		})
	}
}
