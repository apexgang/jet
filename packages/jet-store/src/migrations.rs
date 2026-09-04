//! Forward-only schema migrations tracked in `schema_migrations`
//! (ADR-0073).
//!
//! Each migration commits in its own transaction, so a failure leaves the
//! store at the previous version, and an older `jetd` opens a newer store
//! by skipping versions it does not know. Schema changes are expand-only
//! until the rollback window of the release that introduced them has
//! passed. The verified Recovery snapshot that precedes a migration
//! (ADR-0097) arrives with the recovery work; until then a pre-existing
//! store is migrated in place.

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
	// 3: Durable Command receipts for Actor-scoped idempotency (ADR-0093).
	"ALTER TABLE runs ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
	CREATE TRIGGER runs_revision_after_lifecycle_update
	AFTER UPDATE OF lifecycle ON runs
	WHEN OLD.lifecycle <> NEW.lifecycle
	BEGIN
		UPDATE runs SET revision = revision + 1 WHERE run_id = NEW.run_id;
	END;
	CREATE TABLE command_receipts (
		actor_kind TEXT NOT NULL,
		actor_id TEXT NOT NULL,
		command_id TEXT NOT NULL,
		request_digest BLOB CHECK (length(request_digest) = 32),
		recorded_at_unix_ms INTEGER NOT NULL,
		outcome_version INTEGER,
		outcome TEXT CHECK (length(outcome) <= 65536),
		PRIMARY KEY (actor_kind, actor_id, command_id)
	)",
	// 4: Durable external-work Effect outbox (ADR-0064, ADR-0067).
	// ASVS 2.2.1/2.2.3: CHECK constraints allow only valid state, safety,
	// key, and retry-bound combinations at the trusted storage layer.
	"CREATE TABLE effects (
		effect_id TEXT PRIMARY KEY,
		command_id TEXT NOT NULL,
		run_id TEXT REFERENCES runs (run_id),
		kind TEXT NOT NULL,
		safety TEXT NOT NULL CHECK (
			safety IN ('read_only', 'idempotent', 'ambiguous')
		),
		external_key TEXT,
		max_attempts INTEGER NOT NULL CHECK (max_attempts >= 1),
		state TEXT NOT NULL CHECK (
			state IN ('pending', 'in_flight', 'completed', 'failed', 'outcome_unknown')
		),
		attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
		CHECK (
			(safety = 'idempotent' AND external_key IS NOT NULL)
			OR (safety <> 'idempotent' AND external_key IS NULL)
		),
		CHECK (safety <> 'ambiguous' OR max_attempts = 1)
	);
	CREATE INDEX unresolved_effects ON effects (state);",
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
