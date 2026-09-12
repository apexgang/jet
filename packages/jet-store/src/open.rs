//! Opening the store: the connection, the checks that precede any
//! migration, the pre-migration snapshot, and what a store that fails its
//! checks opens as (ADR-0057, ADR-0073, ADR-0077, ADR-0097).

use crate::{
	IntegrityFailure, IntegrityFailureReason, Opened, StoreError,
	StoreIntegrity, deletion, migrations, plane, recovery,
	snapshot::{self, SnapshotReason},
};
use sqlx::{
	Connection as _, SqlitePool,
	sqlite::{
		SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions,
		SqliteSynchronous,
	},
};
use std::{path::Path, time::Duration};
use uuid::Uuid;

/// Opens the database at `path`, checks it, snapshots it when a migration
/// is pending, migrates it, reads its Plane identity, and reapplies the
/// Deletion ledger. A check or migration that fails leaves the result
/// read-only rather than failing the open (ADR-0077).
pub(crate) async fn connect(
	path: &Path,
	snapshots: &snapshot::Tracker,
) -> Result<Opened, StoreError> {
	let pool = connect_pool(path).await?;
	// Damage can surface in the first statement that touches the schema,
	// before the check itself runs; any of them failing on content rather
	// than reachability is the check failing.
	let schema = match inspect(&pool).await {
		Ok(Inspection::Legacy) => return Err(legacy_schema_refusal()),
		Ok(Inspection::Schema(schema)) => schema,
		Err(StoreError::Integrity(detail)) => {
			return Ok(open_read_only(
				pool,
				IntegrityFailureReason::IntegrityCheck,
				detail,
			)
			.await);
		}
		Err(error) => return Err(error),
	};
	if let migrations::SchemaState::Behind { applied_version } = schema {
		snapshot::create(
			&pool,
			path,
			snapshots,
			SnapshotReason::Migration { applied_version },
			snapshot::wall_clock_unix_ms(),
		)
		.await?;
	}
	if let Err(error) = migrations::apply(&pool).await {
		let failure = recovery::migration_failure(error)?;
		return Ok(open_read_only(pool, failure.reason, failure.detail).await);
	}
	plane::ensure_present(&pool).await?;
	let plane_id = plane::read(&pool).await?.plane_id;
	// What the ledger says is gone stays gone, whether the store is a
	// restored snapshot or crashed between the ledger and its commit. A
	// ledger that vouches for nothing is left for the status to report;
	// the store itself is sound (ADR-0102).
	if let deletion::DeletionLedger::Verified(records) =
		deletion::read(path, plane_id)?
		&& deletion::reapply(&pool, &records).await? > 0
	{
		snapshots.mark_dirty();
	}
	Ok(Opened {
		pool,
		plane_id,
		integrity: StoreIntegrity::Verified,
	})
}

/// What the checks before any migration found.
enum Inspection {
	/// A pre-release tracker owns the store; it is refused outright.
	Legacy,
	/// Where the schema stands, with SQLite's own check passed.
	Schema(migrations::SchemaState),
}

/// The checks that precede any migration, in order: durability settings,
/// the schema tracker, and SQLite's own account of the pages. Damage
/// reported by the check is an integrity error like damage met on the way
/// to it.
async fn inspect(pool: &SqlitePool) -> Result<Inspection, StoreError> {
	verify_durability(pool).await?;
	if is_legacy_schema(pool).await? {
		return Ok(Inspection::Legacy);
	}
	let schema = migrations::state(pool).await?;
	let findings = recovery::quick_check(pool).await?;
	if findings.is_empty() {
		Ok(Inspection::Schema(schema))
	} else {
		Err(StoreError::Integrity(findings.join("; ")))
	}
}

/// A store that stays open for reads only. Its Plane identity is read
/// when the damage allows, and nil otherwise.
async fn open_read_only(
	pool: SqlitePool,
	reason: IntegrityFailureReason,
	detail: String,
) -> Opened {
	Opened {
		plane_id: plane::read(&pool)
			.await
			.map_or(Uuid::nil(), |plane| plane.plane_id),
		pool,
		integrity: StoreIntegrity::Failed(IntegrityFailure { reason, detail }),
	}
}

async fn connect_pool(path: &Path) -> Result<SqlitePool, StoreError> {
	Ok(SqlitePoolOptions::new()
		// One connection, so reads and writes serialize exactly as they
		// did behind the single connection this store used to hold.
		.max_connections(1)
		.min_connections(0)
		// A local file cannot go stale the way a socket can, and both
		// `None`s keep the pool from spawning a maintenance task that
		// would wake an idle Plane (ADR-0055).
		.test_before_acquire(false)
		.idle_timeout(None)
		.max_lifetime(None)
		// Long enough for a durable commit, short enough that a
		// re-entrant transaction fails loudly instead of hanging.
		.acquire_timeout(ACQUIRE_TIMEOUT)
		// SQLite ends a transaction by itself when a statement fails on
		// a full disk or an I/O error, which leaves the driver's own
		// transaction counter one ahead of the connection. The rollback
		// that follows then fails and the counter never comes back down,
		// so such a connection would refuse every later transaction.
		// Discard it and let the next caller open a fresh one.
		.after_release(|connection, _| {
			Box::pin(async move { Ok(!connection.is_in_transaction()) })
		})
		.connect_with(connect_options(path))
		.await?)
}

/// How long a caller waits for the store's one connection. A transaction
/// that outlives this is a re-entrant call, not contention, because the
/// Plane has a single authoritative daemon (ADR-0003).
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long SQLite's own busy handler waits for a write lock held by
/// another process before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The durability settings authoritative state requires (ADR-0057).
pub(crate) fn connect_options(path: &Path) -> SqliteConnectOptions {
	SqliteConnectOptions::new()
		.filename(path)
		.create_if_missing(true)
		.journal_mode(SqliteJournalMode::Wal)
		.synchronous(SqliteSynchronous::Full)
		.foreign_keys(true)
		.busy_timeout(BUSY_TIMEOUT)
}

/// SQLite answers a refused `PRAGMA` with the mode it kept rather than an
/// error, and the driver does not inspect that answer, so the durability
/// settings are read back before any acknowledged commit relies on them
/// (ADR-0057, ADR-0071).
async fn verify_durability(pool: &SqlitePool) -> Result<(), StoreError> {
	// Pragmas have no describable result, so they stay on the runtime query
	// API rather than the compile-time checked macros.
	let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
		.fetch_one(pool)
		.await?;
	let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
		.fetch_one(pool)
		.await?;
	if journal_mode.eq_ignore_ascii_case("wal")
		&& synchronous == SYNCHRONOUS_FULL
	{
		Ok(())
	} else {
		Err(StoreError::Unavailable(format!(
			"the store runs with journal_mode {journal_mode} and synchronous {synchronous} instead of wal and full"
		)))
	}
}

/// A store written before the schema tracker moved into the driver keeps
/// its versions in `schema_migrations`, which the migrator knows nothing
/// about. Report that plainly instead of failing on the first `CREATE
/// TABLE`. The store is pre-release, so the answer is to delete the file.
///
/// The driver's own table decides. A `jetd` from before this change creates
/// an empty `schema_migrations` on any store it opens, including one this
/// code wrote, so its mere presence would condemn a healthy store.
async fn is_legacy_schema(pool: &SqlitePool) -> Result<bool, StoreError> {
	// `sqlite_master` is read before the schema the compile-time query
	// cache describes exists, so this runs on the runtime API.
	let legacy: Option<String> = sqlx::query_scalar(
		"SELECT name FROM sqlite_master
		 WHERE type = 'table' AND name = 'schema_migrations'
		   AND NOT EXISTS (
			SELECT 1 FROM sqlite_master
			WHERE type = 'table' AND name = '_sqlx_migrations'
		   )",
	)
	.fetch_optional(pool)
	.await?;
	Ok(legacy.is_some())
}

fn legacy_schema_refusal() -> StoreError {
	StoreError::Integrity(
		"the store was written by a pre-release schema tracker; delete the \
		 store file and let jetd recreate it"
			.into(),
	)
}

/// SQLite's numeric value for `synchronous = FULL`.
const SYNCHRONOUS_FULL: i64 = 2;
