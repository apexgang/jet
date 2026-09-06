//! Concrete transactional SQLite store for authoritative Plane state
//! (ADR-0048).
//!
//! This is the only crate that links SQLite. It links one pinned bundled
//! build so every supported Plane shares identical transaction, migration,
//! and corruption behavior (ADR-0057). Authoritative state runs in WAL mode
//! with `synchronous=FULL`, so an acknowledged commit survives operating
//! system crash or power loss. SQL and migrations stay private; callers see
//! typed records and stable errors.

mod account;
mod audit;
mod audit_chain;
mod audit_epoch;
mod audit_head;
mod audit_integrity;
mod audit_read;
mod audit_retention;
mod command;
mod conversation;
mod effect;
mod journal;
mod migrations;
mod paired_client;
mod pairing;
mod pairing_offer;
mod plane;
mod project;
mod promotion;
mod records;
mod run;
mod run_execution;
mod search;
mod setting;
mod transaction;
mod workspace;

use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::sqlite::{
	SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions,
	SqliteSynchronous,
};
use sqlx::{Connection, SqlitePool};
use uuid::Uuid;

pub use account::{
	AccountBindingRecord, CredentialSourceRecord, NewAccountBinding,
};
pub use audit::{
	AUDIT_PAGE_LIMIT, AuditOutcome, AuditRecord, AuditRisk, AuditTip,
	NewAuditRecord,
};
pub use audit_chain::{AuditEntryHash, AuditTargetRef};
pub use audit_epoch::AuditGap;
pub use audit_head::{AuditHead, audit_head_path};
pub use audit_integrity::{AuditBreach, AuditIntegrity, AuditIntegrityFailure};
pub use conversation::CONVERSATION_PAGE_LIMIT;
pub use journal::EVENT_COMPACTION_BATCH_LIMIT;
pub use paired_client::{
	NewPairedClient, PairedClientAccess, PairedClientRecord,
};
pub use pairing::PairingGate;
pub use pairing_offer::{
	NewPairingClaim, NewPairingOffer, PairingInvalidation, PairingKeyAlgorithm,
	PairingMethod, PairingOfferRecord, PairingOfferState,
};
pub use plane::PlaneRecord;
pub use project::{NewProject, ProjectRecord};
pub use promotion::{
	NewWorkspacePromotion, PromotionConflictKindRecord,
	PromotionConflictRecord, PromotionDestinationRecord, PromotionStateRecord,
	WorkspacePromotionRecord,
};
pub use records::{
	ActorRecord, CommandReceiptRecord, ConversationPageKey,
	ConversationPageStart, ConversationRecord, EffectKindRecord, EffectRecord,
	EffectSafetyRecord, EffectStateRecord, EventClass, EventRecord,
	NewCommandReceipt, NewConversation, NewEffect, NewEvent, NewRun,
	RetentionPolicy, RunLifecycle, RunRecord, SettingRecord,
	SettingScopeRecord, VerifiedSnapshotCoverage, WorkingTreeRecord,
};
pub use run_execution::RunExecutionRecord;
pub use search::{
	NewSearchDocument, SEARCH_DOCUMENT_BODY_LIMIT, SEARCH_HIT_LIMIT,
	SEARCH_INDEX_BATCH_LIMIT, SearchHitRecord,
};
pub use transaction::{ReadTransaction, WriteTransaction};
pub use workspace::{NewWorkspace, WorkspaceRecord, WorkspaceSeedRecord};

/// Failure inside the store, without native SQLite strings in the category.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
	/// The database could not be opened or reached.
	#[error("store unavailable: {0}")]
	Unavailable(String),
	/// The database is reachable but a statement or its data is broken.
	#[error("store integrity failure: {0}")]
	Integrity(String),
	/// Required Event replay is no longer retained.
	#[error(
		"Event cursor expired before {minimum_available_cursor}; current snapshot revision is {current_snapshot_revision}"
	)]
	CursorExpired {
		/// Oldest cursor from which continuous replay remains possible.
		minimum_available_cursor: u64,
		/// Current Event high-water cursor for a replacement snapshot.
		current_snapshot_revision: u64,
	},
	/// The requested Event cursor is ahead of this Plane's journal.
	#[error(
		"Event cursor is ahead of the current snapshot revision {current_snapshot_revision}"
	)]
	CursorAhead {
		/// Current Event high-water cursor for a replacement snapshot.
		current_snapshot_revision: u64,
	},
}

impl From<sqlx::Error> for StoreError {
	fn from(error: sqlx::Error) -> Self {
		// Native SQLite text reaches the message, never the category.
		if is_unavailable(&error) {
			Self::Unavailable(error.to_string())
		} else {
			Self::Integrity(error.to_string())
		}
	}
}

/// One open Plane store. Current state and the Event journal are read and
/// written through [`Store::read`] and [`Store::write`].
#[derive(Debug)]
pub struct Store {
	pool: SqlitePool,
	/// The database file, which also names the Security audit head kept
	/// beside it (ADR-0105).
	database: PathBuf,
	/// This Plane's durable identity, read once at open. It binds the audit
	/// head to the store it describes.
	plane_id: Uuid,
}

impl Store {
	/// Opens or creates the store at `path` and applies pending migrations.
	///
	/// The Security audit head lives beside the database rather than in it,
	/// at [`audit_head_path`], so a database restored from a snapshot
	/// cannot silently shorten the audit (ADR-0105).
	///
	/// # Errors
	///
	/// Returns [`StoreError::Unavailable`] when the file cannot be opened
	/// and [`StoreError::Integrity`] when its schema cannot be prepared.
	pub async fn open(path: &Path) -> Result<Self, StoreError> {
		let pool = SqlitePoolOptions::new()
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
			.await?;
		verify_durability(&pool).await?;
		reject_legacy_schema(&pool).await?;
		migrations::apply(&pool).await?;
		plane::ensure_present(&pool).await?;
		let plane_id = plane::read(&pool).await?.plane_id;
		Ok(Self {
			pool,
			database: path.to_owned(),
			plane_id,
		})
	}

	/// Current Plane identity and daemon start count.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the Plane row cannot be read.
	pub async fn plane(&self) -> Result<PlaneRecord, StoreError> {
		plane::read(&self.pool).await
	}

	/// Durably records that an authoritative `jetd` started on this Plane and
	/// returns the updated record.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the increment cannot be committed.
	pub async fn record_daemon_start(&self) -> Result<PlaneRecord, StoreError> {
		plane::record_daemon_start(&self.pool).await
	}

	/// Closes the store, letting SQLite finish its write-ahead log checkpoint
	/// before the process exits. Every later call reports the store as
	/// unavailable.
	///
	/// Skipping this loses no data, because WAL with `synchronous=FULL`
	/// already survives abrupt termination (ADR-0071); it only leaves the log
	/// for the next open to replay.
	pub async fn close(&self) {
		self.pool.close().await;
	}
}

/// How long a caller waits for the store's one connection. A transaction
/// that outlives this is a re-entrant call, not contention, because the
/// Plane has a single authoritative daemon (ADR-0003).
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long SQLite's own busy handler waits for a write lock held by
/// another process before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The durability settings authoritative state requires (ADR-0057).
fn connect_options(path: &Path) -> SqliteConnectOptions {
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
async fn reject_legacy_schema(pool: &SqlitePool) -> Result<(), StoreError> {
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
	match legacy {
		None => Ok(()),
		Some(_) => Err(StoreError::Integrity(
			"the store was written by a pre-release schema tracker; delete \
			 the store file and let jetd recreate it"
				.into(),
		)),
	}
}

/// SQLite's numeric value for `synchronous = FULL`.
const SYNCHRONOUS_FULL: i64 = 2;

/// Primary SQLite result codes that mean the store is not reachable.
const SQLITE_BUSY: i32 = 5;
const SQLITE_LOCKED: i32 = 6;
const SQLITE_IOERR: i32 = 10;
const SQLITE_CANTOPEN: i32 = 14;

/// The driver reports SQLite's *extended* result code, whose low byte is
/// the primary code and whose upper bytes are detail. Masking keeps 517
/// (`SQLITE_BUSY_SNAPSHOT`) reading as a busy database rather than as
/// broken data.
fn is_unavailable_code(extended: i32) -> bool {
	matches!(
		extended & 0xff,
		SQLITE_BUSY | SQLITE_LOCKED | SQLITE_IOERR | SQLITE_CANTOPEN
	)
}

/// Whether the store could not be reached at all, as opposed to answering
/// with something broken.
///
/// Both `sqlx::Error` and `sqlx::migrate::MigrateError` are
/// `#[non_exhaustive]`, so the wildcard arms are required by the types and
/// cannot be made exhaustive. A variant a future driver release adds counts
/// as an integrity failure until it is classified here by hand.
fn is_unavailable(error: &sqlx::Error) -> bool {
	match error {
		// Each SQLite connection runs on its own worker thread; losing that
		// thread is an availability failure, not a data one.
		sqlx::Error::Io(_)
		| sqlx::Error::PoolTimedOut
		| sqlx::Error::PoolClosed
		| sqlx::Error::WorkerCrashed => true,
		sqlx::Error::Database(database) => database
			.code()
			.and_then(|code| code.parse::<i32>().ok())
			.is_some_and(is_unavailable_code),
		// A migration failure reports no database error of its own, so an
		// unreachable store met while migrating has to be unwrapped or it
		// is misfiled as an integrity failure.
		sqlx::Error::Migrate(migrate) => match &**migrate {
			sqlx::migrate::MigrateError::Execute(inner)
			| sqlx::migrate::MigrateError::ExecuteMigration(inner, _) => {
				is_unavailable(inner)
			}
			_ => false,
		},
		_ => false,
	}
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
