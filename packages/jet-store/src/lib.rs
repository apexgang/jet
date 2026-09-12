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
mod deletion;
pub use deletion::{DeletedIdentityKind, DeletionLedger, DeletionRecord};
mod effect;
mod journal;
mod migrations;
mod open;
mod pairing;
mod plane;
mod project;
mod records;
mod recovery;
pub use recovery::{
	IntegrityFailure, IntegrityFailureReason, RestoredStore, StoreIntegrity,
};
pub use snapshot::SnapshotPurge;
mod remote_operation;
pub use remote_operation::RemoteOperationRecord;
mod run;
mod schedule;
mod search;
mod setting;
mod snapshot;
mod terminal;
mod transaction;
mod turn_queue;
mod usage;
mod workspace;
pub use terminal::TerminalRecord;

use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
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
pub use snapshot::{RecoverySnapshot, SnapshotReason};
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
	/// The connection and what was learned opening it. Restoring a snapshot
	/// replaces it as a whole (ADR-0077).
	opened: std::sync::RwLock<Opened>,
	/// The database file, which also names the Security audit head kept
	/// beside it (ADR-0105).
	database: PathBuf,
	/// When the next routine Recovery snapshot is due (ADR-0097).
	snapshots: snapshot::Tracker,
}

/// One connection to the database and what opening it established.
#[derive(Debug)]
struct Opened {
	pool: SqlitePool,
	/// This Plane's durable identity, read once at open. It binds the audit
	/// head to the store it describes. It is nil while the store is damaged
	/// enough that its Plane row cannot be read, and nothing writes the head
	/// then.
	plane_id: Uuid,
	integrity: StoreIntegrity,
}

impl Store {
	/// Opens or creates the store at `path` and applies pending migrations.
	///
	/// Every open runs SQLite's lightweight integrity check first. A store
	/// that fails it, or whose migration fails, still opens, but reads only:
	/// [`Store::integrity`] says so, and the damaged database is kept as
	/// found until a verified snapshot is restored over it (ADR-0077).
	///
	/// A store whose schema is behind this build is copied into a verified
	/// Recovery snapshot first, so the release before this one has a
	/// rollback point whatever the migration does (ADR-0073, ADR-0097).
	///
	/// The Security audit head lives beside the database rather than in it,
	/// at [`audit_head_path`], so a database restored from a snapshot
	/// cannot silently shorten the audit (ADR-0105).
	///
	/// # Errors
	///
	/// Returns [`StoreError::Unavailable`] when the file cannot be opened
	/// and [`StoreError::Integrity`] when the pre-migration snapshot fails
	/// verification.
	pub async fn open(path: &Path) -> Result<Self, StoreError> {
		let snapshots = snapshot::Tracker::at_open(path)?;
		let opened = open::connect(path, &snapshots).await?;
		Ok(Self {
			opened: std::sync::RwLock::new(opened),
			database: path.to_owned(),
			snapshots,
		})
	}

	/// Whether this store passed the checks that make it authoritative, as
	/// established when it was opened or last restored.
	#[must_use]
	pub fn integrity(&self) -> StoreIntegrity {
		self.opened().integrity.clone()
	}

	/// Restores the verified Recovery snapshot called `name` over the
	/// damaged database, keeping the damaged files beside it under a name
	/// that carries `now_unix_ms`, and reopens the result through the same
	/// checks as any open (ADR-0077). The Security audit head stays where
	/// it is, so the audit sees that state moved backwards (ADR-0105), and
	/// the Deletion ledger is reapplied to the restored copy, so a deletion
	/// made after the snapshot stays made (ADR-0102).
	///
	/// Every connection this store held is closed first; a read in flight
	/// fails as unavailable.
	///
	/// # Errors
	///
	/// Returns [`StoreError::Integrity`] when no verified snapshot has that
	/// name, a damaged file of that stamp already exists, or the Deletion
	/// ledger cannot be trusted, and
	/// [`StoreError::Unavailable`] when the files cannot be moved or the
	/// restored database cannot be opened. The store then keeps serving
	/// whatever it could open.
	pub async fn restore(
		&self,
		name: &str,
		now_unix_ms: i64,
	) -> Result<RestoredStore, StoreError> {
		// The snapshot may predate a deletion only the ledger remembers, so
		// a ledger that vouches for nothing keeps the store as it is
		// (ADR-0102).
		if let DeletionLedger::Corrupt(detail) = self.deletion_ledger()? {
			return Err(StoreError::Integrity(format!(
				"the Deletion ledger cannot be trusted: {detail}"
			)));
		}
		self.pool().close().await;
		let (opened, restored) = recovery::restore(
			&self.database,
			&self.snapshots,
			&self.integrity(),
			name,
			now_unix_ms,
		)
		.await?;
		*self.opened.write().expect("store state is not poisoned") = opened;
		Ok(restored)
	}

	fn opened(&self) -> std::sync::RwLockReadGuard<'_, Opened> {
		self.opened.read().expect("store state is not poisoned")
	}

	/// The current connection. The pool is a handle, so cloning it out of
	/// the lock keeps no guard across an await.
	fn pool(&self) -> SqlitePool {
		self.opened().pool.clone()
	}

	fn plane_id(&self) -> Uuid {
		self.opened().plane_id
	}

	/// Takes a verified Recovery snapshot for `reason`, stamped
	/// `now_unix_ms`, and rotates the retained set (ADR-0097).
	pub(crate) async fn snapshot(
		&self,
		reason: SnapshotReason,
		now_unix_ms: i64,
	) -> Result<RecoverySnapshot, StoreError> {
		snapshot::create(
			&self.pool(),
			&self.database,
			&self.snapshots,
			reason,
			now_unix_ms,
		)
		.await
	}

	/// Takes the snapshot for `reason` only when one is due at
	/// `now_unix_ms`: a daily one after a write committed since the newest
	/// snapshot on a day without one, and a maintenance one on a day
	/// without one. Between them, at most one snapshot a day; the store
	/// takes the pre-migration one itself, every time (ADR-0097).
	///
	/// # Errors
	///
	/// Returns [`StoreError::Unavailable`] when the copy cannot be written
	/// and [`StoreError::Integrity`] when it fails verification.
	pub async fn snapshot_if_due(
		&self,
		reason: SnapshotReason,
		now_unix_ms: i64,
	) -> Result<Option<RecoverySnapshot>, StoreError> {
		if self.snapshots.is_due(reason, now_unix_ms) {
			Ok(Some(self.snapshot(reason, now_unix_ms).await?))
		} else {
			Ok(None)
		}
	}

	/// Takes a verified snapshot of the store as it is now, after every
	/// deletion the ledger records, and removes every older snapshot taken
	/// at or before the newest of those deletions, which may still hold
	/// what was deleted (ADR-0102). Snapshots newer than the newest
	/// deletion, and rollback points taken after it, stay.
	///
	/// # Errors
	///
	/// Returns [`StoreError::Integrity`] when the Deletion ledger cannot be
	/// trusted or the new snapshot fails verification, and
	/// [`StoreError::Unavailable`] when the files cannot be written or
	/// removed.
	pub async fn purge_recovery_snapshots(
		&self,
		now_unix_ms: i64,
	) -> Result<SnapshotPurge, StoreError> {
		let ledger = self.deletion_ledger()?;
		if let DeletionLedger::Corrupt(detail) = &ledger {
			return Err(StoreError::Integrity(format!(
				"the Deletion ledger cannot be trusted: {detail}"
			)));
		}
		// What is affected is decided before the copy: rotation may already
		// drop some of it when the copy is published, and the reply names
		// every snapshot the purge ended, however it ended.
		let removed = snapshot::affected_by(
			&self.database,
			ledger.newest_deletion_unix_ms(),
		)?;
		let snapshot = self
			.snapshot(SnapshotReason::Maintenance, now_unix_ms)
			.await?;
		snapshot::remove(&self.database, &removed)?;
		Ok(SnapshotPurge {
			snapshot: snapshot.name,
			removed,
		})
	}

	/// The Deletion ledger of this store: every permanent deletion it
	/// vouches for, or the finding that it vouches for nothing (ADR-0102).
	///
	/// # Errors
	///
	/// Returns [`StoreError::Unavailable`] when the ledger files cannot be
	/// read.
	pub fn deletion_ledger(&self) -> Result<DeletionLedger, StoreError> {
		deletion::read(&self.database, self.plane_id())
	}

	/// Every verified Recovery snapshot of this store, newest first.
	///
	/// # Errors
	///
	/// Returns [`StoreError::Unavailable`] when the snapshot directory
	/// cannot be read.
	pub fn recovery_snapshots(
		&self,
	) -> Result<Vec<RecoverySnapshot>, StoreError> {
		snapshot::list(&self.database)
	}

	/// Current Plane identity and daemon start count.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the Plane row cannot be read.
	pub async fn plane(&self) -> Result<PlaneRecord, StoreError> {
		plane::read(&self.pool()).await
	}

	/// Durably records that an authoritative `jetd` started on this Plane and
	/// returns the updated record.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the increment cannot be committed.
	pub async fn record_daemon_start(&self) -> Result<PlaneRecord, StoreError> {
		plane::record_daemon_start(&self.pool()).await
	}

	/// Closes the store, letting SQLite finish its write-ahead log checkpoint
	/// before the process exits. Every later call reports the store as
	/// unavailable.
	///
	/// Skipping this loses no data, because WAL with `synchronous=FULL`
	/// already survives abrupt termination (ADR-0071); it only leaves the log
	/// for the next open to replay.
	pub async fn close(&self) {
		self.pool().close().await;
	}
}

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
	use sqlx::Connection as _;
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
			.execute(&store.pool())
			.await
			.expect("the linked SQLite build must provide FTS5");
		let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
			.fetch_one(&store.pool())
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
		.execute(&store.pool())
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
		.execute(&store.pool())
		.await
		.unwrap();
		drop(store);

		let reopened = Store::open(&path).await.unwrap();

		assert_eq!(reopened.plane().await.unwrap().daemon_starts, 0);
	}

	/// A store whose schema is behind this build is copied into a verified
	/// pre-migration snapshot before anything is applied, and a store with
	/// no schema at all is not (ADR-0073, ADR-0097).
	#[tokio::test]
	async fn a_store_behind_this_build_is_snapshotted_before_migrating() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let fresh = Store::open(&path).await.unwrap();
		assert_eq!(fresh.recovery_snapshots().unwrap(), vec![]);
		fresh.close().await;
		std::fs::remove_file(&path).unwrap();
		// The driver's own tracker table, with nothing applied yet: the
		// shape of a store every migration of this build is ahead of.
		let mut connection = sqlx::SqliteConnection::connect_with(
			&crate::open::connect_options(&path),
		)
		.await
		.unwrap();
		sqlx::query(
			"CREATE TABLE _sqlx_migrations (
				version BIGINT PRIMARY KEY,
				description TEXT NOT NULL,
				installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
				success BOOLEAN NOT NULL,
				checksum BLOB NOT NULL,
				execution_time BIGINT NOT NULL
			)",
		)
		.execute(&mut connection)
		.await
		.unwrap();
		connection.close().await.unwrap();

		let migrated = Store::open(&path).await.unwrap();

		let snapshots = migrated.recovery_snapshots().unwrap();
		assert_eq!(
			snapshots
				.iter()
				.map(|snapshot| snapshot.reason)
				.collect::<Vec<_>>(),
			vec![crate::SnapshotReason::Migration { applied_version: 0 }]
		);
		assert_eq!(migrated.plane().await.unwrap().daemon_starts, 0);
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
			sqlx::query(statement).execute(&store.pool()).await.unwrap();
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
