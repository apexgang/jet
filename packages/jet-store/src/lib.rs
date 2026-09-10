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
mod artifact;
mod audit;
pub use audit::actor::AuditActorRecord;
mod checkpoint;
mod command;
mod conversation;
pub use craft::lifecycle::CraftDisableMode;
mod effect;
mod journal;
mod migrations;
mod pairing;
mod plane;
mod project;
mod records;
mod remote_operation;
pub use remote_operation::RemoteOperationRecord;
mod run;
mod schedule;
mod search;
mod setting;
mod terminal;
mod transaction;
mod turn_queue;
mod usage;
mod workspace;
pub use terminal::TerminalRecord;

use sqlx::{
	Connection, SqlitePool,
	sqlite::{
		SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions,
		SqliteSynchronous,
	},
};
use std::{
	path::{Path, PathBuf},
	time::Duration,
};
use uuid::Uuid;

pub use account::{
	AccountBindingRecord, CredentialSourceRecord, NewAccountBinding,
};
pub use audit::chain::{AuditEntryHash, AuditTargetRef};
pub use audit::epoch::AuditGap;
pub use audit::head::{AuditHead, audit_head_path};
pub use audit::integrity::{
	AuditBreach, AuditIntegrity, AuditIntegrityFailure,
};
pub use audit::{
	AUDIT_PAGE_LIMIT, AuditOutcome, AuditRecord, AuditRisk, AuditTip,
	NewAuditRecord,
};
pub use conversation::CONVERSATION_PAGE_LIMIT;
pub use conversation::fork::ForkLaunchContextRecord;
pub use conversation::import::{
	ImportedConversationRecord, NewImportedConversation,
};
pub use journal::{EVENT_COMPACTION_BATCH_LIMIT, ForkContextEvents};
pub use pairing::PairingGate;
pub use pairing::offer::{
	NewPairingClaim, NewPairingOffer, PairingInvalidation, PairingKeyAlgorithm,
	PairingMethod, PairingOfferRecord, PairingOfferState,
};
pub use pairing::paired_client::{
	NewPairedClient, PairedClientAccess, PairedClientRecord,
};
pub use plane::PlaneRecord;
pub use project::{NewProject, ProjectRecord};
pub use records::{
	ActorRecord, CommandReceiptRecord, ConversationOriginRecord,
	ConversationPageKey, ConversationPageStart, ConversationRecord,
	EffectKindRecord, EffectRecord, EffectSafetyRecord, EffectStateRecord,
	EventClass, EventRecord, NameRecord, NameSourceRecord, NewCommandReceipt,
	NewConversation, NewEffect, NewEvent, NewRun, NewUserEditIntent,
	RetentionPolicy, RunLifecycle, RunRecord, SettingRecord,
	SettingScopeRecord, UserEditIntentRecord, VerifiedSnapshotCoverage,
	WorkingTreeRecord,
};
pub use run::execution::RunExecutionRecord;
pub use search::{
	NewSearchDocument, SEARCH_DOCUMENT_BODY_LIMIT, SEARCH_HIT_LIMIT,
	SEARCH_INDEX_BATCH_LIMIT, SearchHitRecord,
};
pub use transaction::{ReadTransaction, WriteTransaction};
pub use usage::quota::{
	ProviderReachRecord, QuotaScopeRecord, QuotaUnitRecord,
	UsageProviderReachRecord, UsageQuotaHeartbeatRecord,
	UsageQuotaSnapshotRecord,
};
pub use usage::{
	NewUsageObservation, UsageEstimationRecord, UsageFinalityRecord,
	UsageScopeRecord, UsageSelectionRecord, UsageTokensRecord,
	UsageTotalRecord,
};
pub use workspace::promotion::{
	NewWorkspacePromotion, PromotionConflictKindRecord,
	PromotionConflictRecord, PromotionDestinationRecord, PromotionStateRecord,
	WorkspacePromotionRecord,
};
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
mod tests {
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use super::{
		ActorRecord, CommandReceiptRecord, ConversationOriginRecord,
		ConversationRecord, EVENT_COMPACTION_BATCH_LIMIT, EventClass,
		EventRecord, NameRecord, NameSourceRecord, NewCommandReceipt,
		NewConversation, NewEvent, NewRun, PlaneRecord, RetentionPolicy,
		RunLifecycle, RunRecord, SettingRecord, SettingScopeRecord, Store,
		StoreError, WorkingTreeRecord, is_unavailable_code,
	};

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;

	#[tokio::test]
	async fn plane_identity_and_start_count_survive_reopening_the_store() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");

		let first = Store::open(&path).await.unwrap();
		let after_first_start = first.record_daemon_start().await.unwrap();
		drop(first);

		let second = Store::open(&path).await.unwrap();
		let after_second_start = second.record_daemon_start().await.unwrap();

		assert_eq!(
			(after_first_start, &after_second_start),
			(
				PlaneRecord {
					plane_id: after_second_start.plane_id,
					daemon_starts: 1,
				},
				&PlaneRecord {
					plane_id: after_second_start.plane_id,
					daemon_starts: 2,
				}
			)
		);
		assert_eq!(second.plane().await.unwrap(), after_second_start);
	}

	#[tokio::test]
	async fn a_fresh_store_has_a_plane_that_never_started_a_daemon() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();

		let plane = store.plane().await.unwrap();
		assert_eq!(plane.daemon_starts, 0);
		assert!(!plane.plane_id.is_nil());
	}

	#[tokio::test]
	async fn opening_a_store_in_a_missing_directory_is_reported_as_unavailable()
	{
		let dir = tempfile::tempdir().unwrap();
		let error =
			Store::open(&dir.path().join("missing").join("plane.sqlite3"))
				.await
				.unwrap_err();

		assert!(matches!(error, StoreError::Unavailable(_)), "{error:?}");
	}

	/// SQLite reports transient lock and I/O trouble through extended result
	/// codes whose low byte carries the primary code, so the mapping has to mask
	/// before it decides that the store is merely unreachable.
	#[test]
	fn extended_result_codes_keep_the_meaning_of_the_code_they_extend() {
		let unavailable =
			[5, 261, 517, 773, 6, 262, 518, 10, 266, 778, 14, 270];
		let integrity = [1, 8, 11, 19, 26, 275, 787, 1299, 1555, 2067];

		assert_eq!(
			(
				unavailable.map(is_unavailable_code),
				integrity.map(is_unavailable_code)
			),
			([true; 12], [false; 10])
		);
	}

	/// ADR-0057 asks for one pinned SQLite build with WAL and FTS5. Linking a
	/// distribution's SQLite instead is silent, and a build without FTS5 would
	/// only surface once Plane-local search is written, so the capabilities
	/// answer for themselves here.
	#[tokio::test]
	async fn the_linked_sqlite_build_offers_wal_and_fts5() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();

		sqlx::query("CREATE VIRTUAL TABLE fts5_probe USING fts5(body)")
			.execute(&store.pool)
			.await
			.expect("the linked SQLite build must provide FTS5");
		let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
			.fetch_one(&store.pool)
			.await
			.unwrap();

		assert_eq!(journal_mode, "wal");
	}

	/// ADR-0073 keeps releases rollback-compatible, so a `jetd` that predates a
	/// migration still opens the store a newer release migrated, skipping the
	/// version it does not know rather than refusing the whole store.
	#[tokio::test]
	async fn a_store_migrated_by_a_newer_release_still_opens() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		sqlx::query(
			"INSERT INTO _sqlx_migrations (version, description, success,
			checksum, execution_time)
		 VALUES (99990101000000, 'written by a newer release', TRUE, X'00', 0)",
		)
		.execute(&store.pool)
		.await
		.unwrap();
		drop(store);

		let reopened = Store::open(&path).await.unwrap();

		assert_eq!(reopened.plane().await.unwrap().daemon_starts, 0);
	}

	/// Closing the store lets SQLite checkpoint its write-ahead log and remove
	/// it, so the next open has nothing to replay.
	#[tokio::test]
	async fn closing_the_store_checkpoints_the_write_ahead_log() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let log = path.with_extension("sqlite3-wal");
		let store = Store::open(&path).await.unwrap();
		store
			.write(async |tx| {
				tx.insert_conversation(NewConversation {
					conversation_id: Uuid::now_v7(),
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRecord::NoProject,
					origin: ConversationOriginRecord::New,
					created_at_unix_ms: NOW_UNIX_MS,
				})
				.await
			})
			.await
			.unwrap();
		let while_serving = log.exists();

		store.close().await;

		assert_eq!((while_serving, log.exists()), (true, false));
	}

	/// SQLite ends a transaction by itself when a statement fails on a full
	/// disk, which desynchronizes the driver's transaction counter and makes the
	/// rollback that follows fail. A connection returned to the pool in that
	/// state would refuse every later transaction, so it must not be reused.
	#[tokio::test]
	async fn a_connection_left_mid_transaction_is_replaced_rather_than_reused()
	{
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let conversation_id = Uuid::now_v7();

		let wedged = store
			.write(async |tx| {
				// Stands in for the rollback SQLite performs on its own: the
				// transaction ends without the driver being told.
				sqlx::query("ROLLBACK").execute(tx.connection()).await?;
				Err::<(), _>(StoreError::Integrity("the disk filled up".into()))
			})
			.await
			.unwrap_err();

		let recorded = store
			.write(async |tx| {
				tx.insert_conversation(NewConversation {
					conversation_id,
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRecord::NoProject,
					origin: ConversationOriginRecord::New,
					created_at_unix_ms: NOW_UNIX_MS,
				})
				.await
			})
			.await
			.unwrap();

		assert!(matches!(wedged, StoreError::Integrity(_)), "{wedged:?}");
		assert_eq!(
			recorded,
			ConversationRecord {
				conversation_id,
				revision: 1,
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRecord::NoProject,
				origin: ConversationOriginRecord::New,
				name: NameRecord {
					value: format!(
						"Conversation {}",
						&conversation_id.simple().to_string()[..8]
					),
					source: NameSourceRecord::Deterministic,
				},
				created_at_unix_ms: NOW_UNIX_MS,
			}
		);
	}

	/// A `jetd` from before the schema tracker moved into the driver leaves an
	/// empty `schema_migrations` behind on any store it opens, including one
	/// this release wrote. That leftover must not condemn a healthy store.
	#[tokio::test]
	async fn a_current_store_survives_being_opened_by_a_pre_release_jetd() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		sqlx::query(
			"CREATE TABLE schema_migrations (
			version INTEGER PRIMARY KEY,
			applied_at_unix_ms INTEGER NOT NULL
		)",
		)
		.execute(&store.pool)
		.await
		.unwrap();
		drop(store);

		let reopened = Store::open(&path).await.unwrap();

		assert_eq!(reopened.plane().await.unwrap().daemon_starts, 0);
	}

	/// A store the pre-release tracker still owns has nothing the migrator can
	/// build on, so it is refused with an instruction rather than left to fail
	/// on the first `CREATE TABLE`.
	#[tokio::test]
	async fn a_pre_release_store_is_refused_before_it_is_migrated() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		// The shape the previous release left: its own tracker and none of the
		// driver's.
		for statement in [
			"DROP TABLE _sqlx_migrations",
			"CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY)",
		] {
			sqlx::query(statement).execute(&store.pool).await.unwrap();
		}
		drop(store);

		let error = Store::open(&path).await.unwrap_err();

		assert!(matches!(error, StoreError::Integrity(_)), "{error:?}");
	}

	fn actor() -> ActorRecord {
		ActorRecord::InteractiveClient {
			client_id: Uuid::nil(),
		}
	}

	fn conversation_event(conversation_id: Uuid, kind: &str) -> NewEvent {
		NewEvent {
			event_id: Uuid::now_v7(),
			actor: actor(),
			recorded_at_unix_ms: NOW_UNIX_MS,
			conversation_id: Some(conversation_id),
			run_id: None,
			kind: kind.into(),
			payload_version: 1,
			payload: "{}".into(),
			class: EventClass::Semantic,
		}
	}

	fn run_event(conversation_id: Uuid, run_id: Uuid, kind: &str) -> NewEvent {
		NewEvent {
			run_id: Some(run_id),
			..conversation_event(conversation_id, kind)
		}
	}

	fn operational_event(
		conversation_id: Uuid,
		recorded_at_unix_ms: i64,
	) -> NewEvent {
		NewEvent {
			recorded_at_unix_ms,
			class: EventClass::Operational,
			..conversation_event(conversation_id, "run.output_progressed")
		}
	}

	#[tokio::test]
	async fn legacy_event_cursor_stops_before_trailing_name_changes() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let conversation_id = Uuid::now_v7();
		store
			.write(async |tx| {
				tx.append_event(conversation_event(conversation_id, "visible"))
					.await?;
				tx.append_event(conversation_event(
					conversation_id,
					"conversation.name_changed",
				))
				.await?;
				tx.append_event(conversation_event(
					conversation_id,
					"run.name_changed",
				))
				.await?;
				Ok::<_, StoreError>(())
			})
			.await
			.unwrap();

		let page = store
			.read(async |tx| tx.legacy_events_after(0, 10).await)
			.await
			.unwrap();

		assert_eq!(
			(
				page.0,
				page.1
					.into_iter()
					.map(|event| event.sequence)
					.collect::<Vec<_>>()
			),
			(1, vec![1])
		);
	}

	#[tokio::test]
	async fn legacy_event_paging_filters_names_before_its_limit() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let conversation_id = Uuid::now_v7();
		for kind in [
			"first",
			"conversation.name_changed",
			"second",
			"third",
			"run.name_changed",
		] {
			store
				.write(async |tx| {
					tx.append_event(conversation_event(conversation_id, kind))
						.await?;
					Ok::<_, StoreError>(())
				})
				.await
				.unwrap();
		}

		let (first_cursor, first) = store
			.read(async |tx| tx.legacy_events_after(0, 2).await)
			.await
			.unwrap();
		let (last_cursor, last) = store
			.read(async |tx| tx.legacy_events_after(3, 2).await)
			.await
			.unwrap();

		assert_eq!(
			(
				first_cursor,
				first
					.into_iter()
					.map(|event| event.sequence)
					.collect::<Vec<_>>(),
				last_cursor,
				last.into_iter()
					.map(|event| event.sequence)
					.collect::<Vec<_>>(),
			),
			(4, vec![1, 3], 4, vec![4])
		);
	}

	#[tokio::test]
	async fn operational_event_compaction_is_bounded_and_preserves_cursor_truth()
	 {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let conversation_id = Uuid::now_v7();
		let event_count = EVENT_COMPACTION_BATCH_LIMIT + 2;
		store
			.write(async |tx| {
				tx.insert_conversation(NewConversation {
					conversation_id,
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRecord::NoProject,
					origin: ConversationOriginRecord::New,
					created_at_unix_ms: NOW_UNIX_MS,
				})
				.await?;
				tx.append_event(conversation_event(
					conversation_id,
					"conversation.created",
				))
				.await?;
				for index in 0..event_count {
					tx.append_event(operational_event(
						conversation_id,
						index as i64,
					))
					.await?;
				}
				Ok::<_, StoreError>(())
			})
			.await
			.unwrap();

		let compact = async || {
			store
				.write(async |tx| {
					let coverage = tx.verified_projection_coverage().await?;
					tx.compact_operational_events(coverage, i64::MAX).await
				})
				.await
				.unwrap()
		};
		let first = compact().await;
		let expired = store
			.read(async |tx| tx.events_after(0, 10).await)
			.await
			.unwrap_err();
		let second = compact().await;

		assert_eq!((first, second), (EVENT_COMPACTION_BATCH_LIMIT, 2));
		let StoreError::CursorExpired {
			minimum_available_cursor,
			current_snapshot_revision,
		} = expired
		else {
			panic!("expected an expired cursor, got {expired:?}");
		};
		assert_eq!(
			(minimum_available_cursor, current_snapshot_revision),
			(
				u64::try_from(EVENT_COMPACTION_BATCH_LIMIT + 1).unwrap(),
				u64::try_from(event_count + 1).unwrap(),
			)
		);
	}

	#[tokio::test]
	async fn compaction_stops_before_operational_events_inside_the_grace_period()
	 {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let conversation_id = Uuid::now_v7();
		store
			.write(async |tx| {
				tx.append_event(operational_event(conversation_id, 1))
					.await?;
				tx.append_event(operational_event(conversation_id, 3))
					.await?;
				tx.append_event(operational_event(conversation_id, 1))
					.await?;
				Ok::<_, StoreError>(())
			})
			.await
			.unwrap();

		let removed = store
			.write(async |tx| {
				let coverage = tx.verified_projection_coverage().await?;
				tx.compact_operational_events(coverage, 2).await
			})
			.await
			.unwrap();
		let (cursor, events) = store
			.read(async |tx| tx.events_after(1, 10).await)
			.await
			.unwrap();

		assert_eq!(removed, 1);
		assert_eq!(cursor, 3);
		assert_eq!(
			events
				.into_iter()
				.map(|event| (event.sequence, event.recorded_at_unix_ms))
				.collect::<Vec<_>>(),
			vec![(2, 3), (3, 1)]
		);
	}

	/// Compaction is the one statement that reuses a numbered parameter: it
	/// mentions the coverage bound three times and the grace cutoff twice. The
	/// two are interchangeable on the data the other compaction tests use, so
	/// this one is shaped so that binding them the other way round removes a
	/// different number of Events.
	#[tokio::test]
	async fn compaction_binds_the_coverage_bound_apart_from_the_grace_cutoff() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let conversation_id = Uuid::now_v7();
		store
			.write(async |tx| {
				for _ in 0..3 {
					tx.append_event(operational_event(conversation_id, 1))
						.await?;
				}
				Ok::<_, StoreError>(())
			})
			.await
			.unwrap();

		// Coverage 3 with cutoff 2 removes all three; swapping them would bound
		// the sequence by 2 and remove only two.
		let removed = store
			.write(async |tx| {
				let coverage = tx.verified_projection_coverage().await?;
				tx.compact_operational_events(coverage, 2).await
			})
			.await
			.unwrap();

		assert_eq!(removed, 3);
	}

	#[tokio::test]
	async fn snapshot_coverage_cannot_compact_another_plane() {
		let dir = tempfile::tempdir().unwrap();
		let first = Store::open(&dir.path().join("first.sqlite3"))
			.await
			.unwrap();
		let second = Store::open(&dir.path().join("second.sqlite3"))
			.await
			.unwrap();
		let coverage = first
			.write(async |tx| {
				tx.append_event(operational_event(Uuid::now_v7(), 1))
					.await?;
				tx.verified_projection_coverage().await
			})
			.await
			.unwrap();
		second
			.write(async |tx| {
				tx.append_event(operational_event(Uuid::now_v7(), 1))
					.await?;
				Ok::<_, StoreError>(())
			})
			.await
			.unwrap();

		let error = second
			.write(async |tx| tx.compact_operational_events(coverage, 2).await)
			.await
			.unwrap_err();

		assert!(matches!(error, StoreError::Integrity(_)), "{error:?}");
	}

	#[tokio::test]
	async fn a_cursor_ahead_of_the_journal_is_rejected() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();

		let error = store
			.read(async |tx| tx.events_after(1, 10).await)
			.await
			.unwrap_err();

		assert!(
			matches!(
				error,
				StoreError::CursorAhead {
					current_snapshot_revision: 0
				}
			),
			"{error:?}"
		);
	}

	#[tokio::test]
	async fn conversations_runs_and_events_survive_reopening_the_store() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let conversation_id = Uuid::now_v7();
		let run_id = Uuid::now_v7();

		let first = Store::open(&path).await.unwrap();
		let (conversation, run, created, ended) = first
			.write(async |tx| {
				let conversation = tx
					.insert_conversation(NewConversation {
						conversation_id,
						retention: RetentionPolicy::Retain,
						working_tree: WorkingTreeRecord::NoProject,
						origin: ConversationOriginRecord::New,
						created_at_unix_ms: NOW_UNIX_MS,
					})
					.await?;
				assert_eq!(tx.runs(conversation_id).await?, vec![]);
				let created = tx
					.append_event(conversation_event(
						conversation_id,
						"conversation.created",
					))
					.await?;
				tx.insert_run(NewRun {
					run_id,
					conversation_id,
					created_at_unix_ms: NOW_UNIX_MS + 1,
				})
				.await?;
				let run = tx
					.update_run_lifecycle(
						run_id,
						RunLifecycle::Completed,
						NOW_UNIX_MS + 2,
					)
					.await?;
				let ended = tx
					.append_event(run_event(
						conversation_id,
						run_id,
						"run.lifecycle_changed",
					))
					.await?;
				Ok::<_, StoreError>((conversation, run, created, ended))
			})
			.await
			.unwrap();
		drop(first);

		let second = Store::open(&path).await.unwrap();
		let (conversations, runs, events, cursor) = second
			.read(async |tx| {
				Ok::<_, StoreError>((
					tx.conversations().await?,
					tx.runs(conversation_id).await?,
					tx.events_after(0, 10).await?,
					tx.event_cursor().await?,
				))
			})
			.await
			.unwrap();
		let later = second
			.write(async |tx| {
				tx.append_event(conversation_event(conversation_id, "later"))
					.await
			})
			.await
			.unwrap();

		assert_eq!(
			conversation,
			ConversationRecord {
				conversation_id,
				revision: 1,
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRecord::NoProject,
				origin: ConversationOriginRecord::New,
				name: NameRecord {
					value: format!(
						"Conversation {}",
						&conversation_id.simple().to_string()[..8]
					),
					source: NameSourceRecord::Deterministic,
				},
				created_at_unix_ms: NOW_UNIX_MS,
			}
		);
		assert_eq!(
			run,
			RunRecord {
				run_id,
				conversation_id,
				revision: 2,
				lifecycle: RunLifecycle::Completed,
				name: NameRecord {
					value: format!("Run {}", &run_id.simple().to_string()[..8]),
					source: NameSourceRecord::Deterministic,
				},
				created_at_unix_ms: NOW_UNIX_MS + 1,
				ended_at_unix_ms: Some(NOW_UNIX_MS + 2),
			}
		);
		assert_eq!(conversations, vec![conversation]);
		assert_eq!(runs, vec![run]);
		assert_eq!(
			events,
			(
				2,
				vec![
					EventRecord {
						sequence: 1,
						..created
					},
					EventRecord {
						sequence: 2,
						..ended
					}
				],
			)
		);
		assert_eq!((cursor, later.sequence), (2, 3));
	}

	#[tokio::test]
	async fn a_failed_write_leaves_no_trace_of_its_changes() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let conversation_id = Uuid::now_v7();

		let error = store
			.write(async |tx| {
				tx.insert_conversation(NewConversation {
					conversation_id,
					retention: RetentionPolicy::ForgetAfterFinalRun,
					working_tree: WorkingTreeRecord::NoProject,
					origin: ConversationOriginRecord::New,
					created_at_unix_ms: NOW_UNIX_MS,
				})
				.await?;
				tx.append_event(conversation_event(
					conversation_id,
					"conversation.created",
				))
				.await?;
				Err::<(), _>(StoreError::Integrity(
					"rejected by the caller".into(),
				))
			})
			.await
			.unwrap_err();

		let (conversation, cursor) = store
			.read(async |tx| {
				Ok::<_, StoreError>((
					tx.conversation(conversation_id).await?,
					tx.event_cursor().await?,
				))
			})
			.await
			.unwrap();
		assert!(matches!(error, StoreError::Integrity(_)), "{error:?}");
		assert_eq!((conversation, cursor), (None, 0));
	}

	#[tokio::test]
	async fn a_run_cannot_be_recorded_for_an_unknown_conversation() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();

		let error = store
			.write(async |tx| {
				tx.insert_run(NewRun {
					run_id: Uuid::now_v7(),
					conversation_id: Uuid::now_v7(),
					created_at_unix_ms: NOW_UNIX_MS,
				})
				.await
			})
			.await
			.unwrap_err();

		assert!(matches!(error, StoreError::Integrity(_)), "{error:?}");
	}

	#[tokio::test]
	async fn expired_command_receipts_keep_only_an_identity_tombstone() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let actor = actor();
		let command_id = Uuid::now_v7();
		store
			.write(async |tx| {
				tx.insert_command_receipt(&NewCommandReceipt {
					actor,
					command_id,
					request_digest: [7; 32],
					recorded_at_unix_ms: 10,
					outcome_version: 1,
					outcome: r#"{"Ok":{}}"#.into(),
				})
				.await?;
				tx.prune_command_receipts_before(11).await
			})
			.await
			.unwrap();

		let receipt = store
			.read(async |tx| tx.command_receipt(actor, command_id).await)
			.await
			.unwrap()
			.unwrap();

		assert_eq!(
			receipt,
			CommandReceiptRecord {
				actor,
				command_id,
				request_digest: None,
				recorded_at_unix_ms: 10,
				outcome_version: None,
				outcome: None,
			}
		);
	}

	fn setting(
		key: &str,
		scope: SettingScopeRecord,
		value: &str,
	) -> SettingRecord {
		SettingRecord {
			key: key.into(),
			scope,
			value: value.into(),
			updated_at_unix_ms: NOW_UNIX_MS,
		}
	}

	#[tokio::test]
	async fn a_scope_reads_the_values_it_stores_beside_the_planes() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let conversation_id = Uuid::now_v7();
		let elsewhere = SettingScopeRecord::Conversation {
			conversation_id: Uuid::now_v7(),
		};
		let addressed = SettingScopeRecord::Conversation { conversation_id };
		store
			.write(async |tx| {
				tx.upsert_setting(&setting(
					"utility.automatic_naming",
					SettingScopeRecord::Plane,
					"true",
				))
				.await?;
				tx.upsert_setting(&setting(
					"utility.automatic_naming",
					addressed,
					"false",
				))
				.await?;
				tx.upsert_setting(&setting(
					"git.auto_commit",
					elsewhere,
					"true",
				))
				.await
			})
			.await
			.unwrap();

		let chain = store
			.read(async |tx| tx.settings_for_scope(addressed).await)
			.await
			.unwrap();

		assert_eq!(
			chain,
			vec![
				setting("utility.automatic_naming", addressed, "false"),
				setting(
					"utility.automatic_naming",
					SettingScopeRecord::Plane,
					"true"
				),
			]
		);
	}

	#[tokio::test]
	async fn writing_a_scope_replaces_only_the_value_that_scope_stored() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let conversation_id = Uuid::now_v7();
		let addressed = SettingScopeRecord::Conversation { conversation_id };
		let key = "utility.automatic_naming";

		let (replaced, cleared) = store
			.write(async |tx| {
				tx.upsert_setting(&setting(
					key,
					SettingScopeRecord::Plane,
					"false",
				))
				.await?;
				tx.upsert_setting(&setting(key, addressed, "false")).await?;
				tx.upsert_setting(&setting(key, addressed, "true")).await?;
				let replaced = tx.settings_for_scope(addressed).await?;
				tx.delete_setting(key, addressed).await?;
				let cleared = tx.settings_for_scope(addressed).await?;
				Ok::<_, StoreError>((replaced, cleared))
			})
			.await
			.unwrap();

		assert_eq!(
			(replaced, cleared),
			(
				vec![
					setting(key, addressed, "true"),
					setting(key, SettingScopeRecord::Plane, "false"),
				],
				vec![setting(key, SettingScopeRecord::Plane, "false")]
			)
		);
	}
}

mod utility;

mod extension;

mod auto_continue;

mod git_delivery;

mod craft;
