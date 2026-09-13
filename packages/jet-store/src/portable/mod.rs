//! A consistent, sanitized SQLite copy for portable Recovery (ADR-0074).
mod recover;
mod sanitize;
mod validate;
use crate::{Store, StoreError};
pub use recover::RecoveredSnapshot;
use sqlx::{
	Connection, SqliteConnection,
	sqlite::{SqliteConnectOptions, SqliteJournalMode},
};
use std::path::Path;

impl Store {
	/// Writes a sanitized consistent snapshot to a private, nonexistent path.
	/// The caller must keep its parent owner-only and discard it on failure.
	/// Returns a store error if copying, sanitizing, or verifying fails.
	pub async fn portable_snapshot(
		&self,
		path: &Path,
	) -> Result<(), StoreError> {
		let target = path.to_str().ok_or_else(|| {
			StoreError::Unavailable("invalid snapshot path".into())
		})?;
		// VACUUM INTO cannot be described by SQLx's compile-time macros.
		sqlx::query("VACUUM INTO ?1")
			.bind(target)
			.execute(&self.pool())
			.await?;
		let mut connection = SqliteConnection::connect_with(
			&SqliteConnectOptions::new()
				.filename(path)
				.foreign_keys(false)
				.journal_mode(SqliteJournalMode::Delete),
		)
		.await?;
		sanitize::sanitize(&mut connection).await?;
		validate::validate(&mut connection).await?;
		// VACUUM removes deleted authentication bytes from free pages. No raw
		// snapshot bytes are returned before it completes (ASVS 14.2.4).
		sqlx::query("VACUUM").execute(&mut connection).await?;
		connection.close().await?;
		crate::snapshot::verify(path).await
	}
}

impl Store {
	/// Reads the complete Artifact set from a previously verified private copy.
	/// Returns an integrity error for conflicting declared sizes or failed reads.
	pub async fn portable_artifacts(
		path: &Path,
	) -> Result<Vec<(String, u64)>, StoreError> {
		let mut connection = SqliteConnection::connect_with(
			&SqliteConnectOptions::new().filename(path).read_only(true),
		)
		.await?;
		let rows = sqlx::query!(r#"SELECT sha256 AS "sha256!", MIN(size) AS "minimum!", MAX(size) AS "maximum!" FROM artifact_references GROUP BY sha256 ORDER BY sha256"#).fetch_all(&mut connection).await?;
		connection.close().await?;
		rows.into_iter()
			.map(|row| {
				if row.minimum != row.maximum || row.minimum < 0 {
					return Err(StoreError::Integrity(
						"conflicting Artifact sizes".into(),
					));
				}
				Ok((row.sha256, row.minimum as u64))
			})
			.collect()
	}
}
