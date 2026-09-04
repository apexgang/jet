//! The single Plane row: durable identity plus daemon lifecycle counters.

use sqlx::{SqliteExecutor, SqlitePool};
use uuid::Uuid;

use crate::StoreError;
use crate::records::parse_uuid;
use crate::transaction::ReadTransaction;

/// Durable identity and daemon lifecycle counters of the Plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneRecord {
	/// Identity created when the store was first created.
	pub plane_id: Uuid,
	/// Number of authoritative `jetd` starts recorded so far.
	pub daemon_starts: u64,
}

impl ReadTransaction {
	/// Reads the Plane record inside this transaction's consistent snapshot.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the Plane row cannot be read.
	pub async fn plane(&mut self) -> Result<PlaneRecord, StoreError> {
		read(self.connection()).await
	}
}

pub(crate) async fn ensure_present(
	pool: &SqlitePool,
) -> Result<(), StoreError> {
	let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
	let existing =
		sqlx::query_scalar!("SELECT plane_id FROM plane WHERE singleton = 1")
			.fetch_optional(&mut *transaction)
			.await?;
	if existing.is_none() {
		let plane_id = Uuid::now_v7().to_string();
		sqlx::query!(
			"INSERT INTO plane (singleton, plane_id, daemon_starts)
			 VALUES (1, ?1, 0)",
			plane_id
		)
		.execute(&mut *transaction)
		.await?;
	}
	transaction.commit().await?;
	Ok(())
}

pub(crate) async fn read(
	executor: impl SqliteExecutor<'_>,
) -> Result<PlaneRecord, StoreError> {
	let row = sqlx::query!(
		"SELECT plane_id, daemon_starts FROM plane WHERE singleton = 1"
	)
	.fetch_one(executor)
	.await?;
	Ok(PlaneRecord {
		plane_id: parse_uuid("plane_id", &row.plane_id)?,
		daemon_starts: u64::try_from(row.daemon_starts).map_err(|_| {
			StoreError::Integrity("daemon_starts is negative".into())
		})?,
	})
}

pub(crate) async fn record_daemon_start(
	pool: &SqlitePool,
) -> Result<PlaneRecord, StoreError> {
	let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
	sqlx::query!(
		"UPDATE plane SET daemon_starts = daemon_starts + 1
		 WHERE singleton = 1"
	)
	.execute(&mut *transaction)
	.await?;
	let record = read(&mut *transaction).await?;
	transaction.commit().await?;
	Ok(record)
}
