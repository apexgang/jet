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
	// 2: Conversations, their Runs, and the Plane-local Event journal
	// (ADR-0001, ADR-0020, ADR-0096).
	"CREATE TABLE conversations (
		conversation_id TEXT PRIMARY KEY,
		retention TEXT NOT NULL,
		created_at_unix_ms INTEGER NOT NULL
	);
	CREATE TABLE runs (
		run_id TEXT PRIMARY KEY,
		conversation_id TEXT NOT NULL REFERENCES conversations (conversation_id),
		lifecycle TEXT NOT NULL,
		created_at_unix_ms INTEGER NOT NULL,
		ended_at_unix_ms INTEGER
	);
	CREATE INDEX runs_by_conversation ON runs (conversation_id);
	CREATE TABLE events (
		sequence INTEGER PRIMARY KEY AUTOINCREMENT,
		event_id TEXT NOT NULL UNIQUE,
		actor_kind TEXT NOT NULL,
		actor_id TEXT,
		recorded_at_unix_ms INTEGER NOT NULL,
		conversation_id TEXT,
		run_id TEXT,
		kind TEXT NOT NULL,
		payload_version INTEGER NOT NULL,
		payload TEXT NOT NULL CHECK (length(payload) <= 65536)
	);
	CREATE INDEX events_by_conversation ON events (conversation_id, sequence);",
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
			(version, crate::clock::unix_ms_now()),
		)?;
		transaction.commit()?;
	}
	Ok(())
}
