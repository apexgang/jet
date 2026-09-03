//! Forward-only schema migrations tracked in `schema_migrations`.

use rusqlite::Connection;

use crate::StoreError;

const MIGRATIONS: &[&str] = &[
	// 1: Plane identity and daemon lifecycle counters.
	"CREATE TABLE plane (
		singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
		plane_id TEXT NOT NULL,
		daemon_starts INTEGER NOT NULL DEFAULT 0
	)",
];

pub(crate) fn apply(connection: &mut Connection) -> Result<(), StoreError> {
	connection.execute_batch(
		"CREATE TABLE IF NOT EXISTS schema_migrations (
			version INTEGER PRIMARY KEY,
			applied_at_unix_ms INTEGER NOT NULL
		)",
	)?;
	let applied: i64 = connection.query_row(
		"SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
		[],
		|row| row.get(0),
	)?;
	let applied = usize::try_from(applied)
		.map_err(|_| StoreError::Integrity("negative schema version".into()))?;
	for (index, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
		let version = i64::try_from(index + 1).map_err(|_| {
			StoreError::Integrity("schema version overflow".into())
		})?;
		let transaction = connection.transaction()?;
		transaction.execute_batch(sql)?;
		transaction.execute(
			"INSERT INTO schema_migrations (version, applied_at_unix_ms)
			 VALUES (?1, ?2)",
			(version, unix_ms_now()),
		)?;
		transaction.commit()?;
	}
	Ok(())
}

fn unix_ms_now() -> i64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.ok()
		.and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
		.unwrap_or_default()
}
