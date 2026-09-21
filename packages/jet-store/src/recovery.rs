//! Read-only Recovery of the store itself: the integrity check an open
//! runs after an unclean shutdown, what the store looks like when it
//! fails, and restoring a verified Recovery snapshot over the damaged
//! database (ADR-0077).
//!
//! A store that fails its check still opens. Reads work as far as the
//! damage allows, so the Plane can be diagnosed and exported, and nothing
//! writes: the damaged database is preserved exactly as found until an
//! owner restores a snapshot over it. Restoring moves the damaged files
//! aside rather than deleting them, and reopens the restored copy through
//! the same checks as any other open.

use crate::{
	Opened, Store, StoreError,
	snapshot::{self, Tracker, unavailable},
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::{
	fs,
	path::{Path, PathBuf},
};

/// Suffix the database and its journal files keep when a snapshot is
/// restored over them, followed by the restoration's stamp: what the
/// check found them to be.
const DAMAGED_SUFFIX: &str = ".damaged-";
const UNMIGRATED_SUFFIX: &str = ".unmigrated-";
const REPLACED_SUFFIX: &str = ".replaced-";

/// The write-ahead log, which SQLite removes when the last connection to
/// the database closes.
const WAL_SUFFIX: &str = "-wal";

/// The companions SQLite keeps beside a database in write-ahead-log mode.
const JOURNAL_SUFFIXES: [&str; 2] = [WAL_SUFFIX, "-shm"];

/// How the last process to hold the store left it, as far as the files
/// beside the database say before anything opens it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreviousShutdown {
	/// The last connection closed and SQLite removed its write-ahead log,
	/// so every acknowledged commit is in the database file itself.
	Clean,
	/// A write-ahead log is still beside the database: the last holder
	/// never closed it. What it left is checked before it is served
	/// (ADR-0077).
	Unclean,
}

impl PreviousShutdown {
	/// Reads the mark a crash leaves. It is read before the store connects,
	/// because connecting creates the very log it looks for.
	pub(crate) fn of(database: &Path) -> Self {
		match sibling(database, WAL_SUFFIX) {
			Ok(log) if log.exists() => Self::Unclean,
			Ok(_) | Err(_) => Self::Clean,
		}
	}
}

/// Whether the open store passed the checks that make it authoritative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreIntegrity {
	/// The check passed and the schema is current.
	Verified,
	/// It did not, and the store answers reads only.
	Failed(IntegrityFailure),
}

/// What kept the store from being authoritative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityFailure {
	/// Which check failed.
	pub reason: IntegrityFailureReason,
	/// SQLite's own account of it, for local diagnostics only (ADR-0061).
	pub detail: String,
}

/// Which check a store failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityFailureReason {
	/// `PRAGMA quick_check` at open, or the deep check while idle,
	/// reported damage.
	IntegrityCheck,
	/// A schema migration failed, leaving the store at its previous
	/// version (ADR-0073).
	Migration,
}

/// What a restoration replaced and with what.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoredStore {
	/// The snapshot now serving as the database.
	pub snapshot: String,
	/// The file name the previous database was moved to, beside the
	/// store: `plane.sqlite3.damaged-<stamp>` after a failed integrity
	/// check, `plane.sqlite3.unmigrated-<stamp>` after a failed migration,
	/// which left it intact at its previous version.
	pub replaced: String,
}

impl Store {
	/// Refuses a write while the store is in read-only Recovery mode: the
	/// damaged database is preserved exactly as found until a verified
	/// snapshot is restored over it (ADR-0077). The refusal is
	/// unavailability, like the one a Command meets at the door, because
	/// the same write succeeds after the restoration.
	pub(crate) fn require_writable(&self) -> Result<(), StoreError> {
		match self.integrity() {
			StoreIntegrity::Verified => Ok(()),
			StoreIntegrity::Failed(_) => Err(StoreError::Unavailable(
				"the store is in read-only Recovery mode; nothing is written \
				 until a verified Recovery snapshot is restored"
					.into(),
			)),
		}
	}
}

/// Runs SQLite's lightweight check over the database behind `pool` and
/// returns what it found, which is empty when the database is sound. It
/// reads every page, so it costs what the store weighs; an open runs it
/// only after an unclean shutdown, and the deeper checks belong to idle
/// time (ADR-0077, ADR-0022).
///
/// The macros cannot describe pragmas; see `verify_durability`.
pub(crate) async fn quick_check(
	pool: &SqlitePool,
) -> Result<Vec<String>, StoreError> {
	let report: Vec<String> = sqlx::query_scalar("PRAGMA quick_check")
		.fetch_all(pool)
		.await?;
	Ok(findings(report))
}

/// What an integrity pragma reported, minus the one row that means it
/// found nothing.
pub(crate) fn findings(report: Vec<String>) -> Vec<String> {
	if report.len() == 1 && report[0] == "ok" {
		vec![]
	} else {
		report
	}
}

/// Moves the database and its journal files aside under a name that says
/// what `integrity` found them to be, copies the verified snapshot `name`
/// into their place, and reopens it. The copy was verified when it was
/// taken, not now, so it is checked page by page before it serves.
pub(crate) async fn restore(
	database: &Path,
	snapshots: &Tracker,
	integrity: &StoreIntegrity,
	name: &str,
	now_unix_ms: i64,
) -> Result<(Opened, RestoredStore), StoreError> {
	let source = snapshot::locate(database, name)?;
	let damaged = replaced_name(database, integrity, now_unix_ms)?;
	let directory = database.parent().ok_or_else(|| {
		StoreError::Unavailable("store has no directory".into())
	})?;
	rename_aside(database, &directory.join(&damaged))?;
	for suffix in JOURNAL_SUFFIXES {
		let journal = sibling(database, suffix)?;
		if journal.exists() {
			rename_aside(
				&journal,
				&directory.join(format!("{damaged}{suffix}")),
			)?;
		}
	}
	// ASVS 16.3.1: the restored database is owner-only like the snapshot.
	fs::copy(&source, database)
		.map_err(|error| unavailable(database, &error))?;
	fs::File::open(database)
		.and_then(|file| file.sync_all())
		.map_err(|error| unavailable(database, &error))?;
	fs::File::open(directory)
		.and_then(|dir| dir.sync_all())
		.map_err(|error| unavailable(directory, &error))?;
	let opened = crate::open::connect(
		database,
		snapshots,
		crate::open::PageCheck::Always,
	)
	.await?;
	snapshots.mark_dirty();
	Ok((
		opened,
		RestoredStore {
			snapshot: name.to_owned(),
			replaced: damaged,
		},
	))
}

/// Whether a migration error means the store is unreachable, which is
/// reported as such, or that the migration itself failed, which leaves a
/// store at its previous version to be served read-only.
pub(crate) fn migration_failure(
	error: StoreError,
) -> Result<IntegrityFailure, StoreError> {
	match error {
		StoreError::Unavailable(_) => Err(error),
		StoreError::Integrity(detail) => Ok(IntegrityFailure {
			reason: IntegrityFailureReason::Migration,
			detail,
		}),
		StoreError::CursorExpired { .. } | StoreError::CursorAhead { .. } => {
			Err(error)
		}
	}
}

fn replaced_name(
	database: &Path,
	integrity: &StoreIntegrity,
	now_unix_ms: i64,
) -> Result<String, StoreError> {
	let file = database
		.file_name()
		.and_then(|name| name.to_str())
		.ok_or_else(|| {
			StoreError::Unavailable("store file name is not valid UTF-8".into())
		})?;
	let suffix = match integrity {
		StoreIntegrity::Verified => REPLACED_SUFFIX,
		StoreIntegrity::Failed(IntegrityFailure { reason, .. }) => match reason
		{
			IntegrityFailureReason::IntegrityCheck => DAMAGED_SUFFIX,
			IntegrityFailureReason::Migration => UNMIGRATED_SUFFIX,
		},
	};
	Ok(format!("{file}{suffix}{now_unix_ms}"))
}

fn sibling(database: &Path, suffix: &str) -> Result<PathBuf, StoreError> {
	let mut name = database
		.file_name()
		.ok_or_else(|| {
			StoreError::Unavailable("store has no file name".into())
		})?
		.to_os_string();
	name.push(suffix);
	Ok(database.with_file_name(name))
}

fn rename_aside(from: &Path, to: &Path) -> Result<(), StoreError> {
	if to.exists() {
		return Err(StoreError::Integrity(format!(
			"{} already exists",
			to.display()
		)));
	}
	fs::rename(from, to).map_err(|error| unavailable(from, &error))
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;
	use crate::{SnapshotReason, Store};
	use pretty_assertions::assert_eq;
	use sqlx::Connection as _;
	use std::io::{Seek as _, SeekFrom, Write as _};

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;

	/// Overwrites one page of a closed database, starting at `offset`.
	fn damage_at(path: &Path, offset: u64) {
		let mut file = fs::OpenOptions::new().write(true).open(path).unwrap();
		file.seek(SeekFrom::Start(offset)).unwrap();
		file.write_all(&[0xff; 4096]).unwrap();
		file.sync_all().unwrap();
	}

	/// Overwrites the second page of a closed database, which holds schema
	/// or table content in any store this crate creates.
	fn damage(path: &Path) {
		damage_at(path, 4096);
	}

	/// A damaged store opens read-only with the damage named, keeps every
	/// file as found, and restores its newest verified snapshot on request,
	/// moving the damaged database aside rather than deleting it
	/// (ADR-0077).
	#[tokio::test]
	async fn a_damaged_store_opens_read_only_and_restores_a_snapshot() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let plane = store.plane().await.unwrap();
		let snapshot = store
			.snapshot(SnapshotReason::Daily, NOW_UNIX_MS)
			.await
			.unwrap();
		store.close().await;
		damage(&path);

		let damaged = Store::open(&path).await.unwrap();
		let StoreIntegrity::Failed(failure) = damaged.integrity() else {
			panic!("the damage went unnoticed");
		};
		assert_eq!(failure.reason, IntegrityFailureReason::IntegrityCheck);
		assert!(!failure.detail.is_empty());
		let unknown = damaged
			.restore("plane-1-daily.sqlite3", NOW_UNIX_MS)
			.await
			.unwrap_err();
		assert_eq!(
			unknown.to_string(),
			"store integrity failure: no verified snapshot is called plane-1-daily.sqlite3"
		);

		let restored = damaged
			.restore(&snapshot.name, NOW_UNIX_MS + 1)
			.await
			.unwrap();
		assert_eq!(
			restored,
			RestoredStore {
				snapshot: snapshot.name.clone(),
				replaced: format!("plane.sqlite3.damaged-{}", NOW_UNIX_MS + 1),
			}
		);
		assert_eq!(damaged.integrity(), StoreIntegrity::Verified);
		assert_eq!(damaged.plane().await.unwrap(), plane);
		assert!(dir.path().join(&restored.replaced).exists());
		// The restored store writes again, and its snapshot is still there.
		damaged.record_daemon_start().await.unwrap();
		assert_eq!(
			damaged
				.recovery_snapshots()
				.unwrap()
				.into_iter()
				.map(|s| s.name)
				.collect::<Vec<_>>(),
			vec![snapshot.name]
		);
	}

	/// Overwrites the page in the middle of a closed database, which the
	/// Event journal fills once [`fill_journal`] has run: a page nothing
	/// reads on the way to serving the store.
	pub(crate) fn damage_journal_page(path: &Path) {
		let length = fs::metadata(path).unwrap().len();
		damage_at(path, length / 2 / 4096 * 4096);
	}

	/// Appends Events until the journal is most of the file, so the page
	/// in its middle is the journal's wherever the schema's pages fall.
	pub(crate) async fn fill_journal(store: &Store) {
		store
			.write(async |tx| {
				for _ in 0..256 {
					tx.append_event(crate::NewEvent {
						event_id: uuid::Uuid::now_v7(),
						actor: crate::ActorRecord::InteractiveClient {
							client_id: uuid::Uuid::nil(),
						},
						recorded_at_unix_ms: 0,
						conversation_id: None,
						run_id: None,
						kind: "run.progress".into(),
						payload_version: 1,
						payload: format!(
							"{{\"p\":\"{}\"}}",
							"x".repeat(16 * 1024)
						),
						class: crate::EventClass::Operational,
					})
					.await?;
				}
				Ok::<(), StoreError>(())
			})
			.await
			.unwrap();
	}

	/// The lightweight check follows an unclean shutdown alone (ADR-0077):
	/// damage on a page nothing reads at open is served after a clean
	/// close, and puts the store in read-only Recovery mode when a
	/// write-ahead log says the last holder never closed it.
	#[tokio::test]
	async fn the_check_runs_after_an_unclean_shutdown_alone() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		fill_journal(&store).await;
		store.close().await;
		damage_journal_page(&path);

		let clean = Store::open(&path).await.unwrap();
		assert_eq!(clean.integrity(), StoreIntegrity::Verified);
		clean.close().await;

		// What a crash leaves behind: the log SQLite removes on a clean
		// close is still beside the database.
		fs::write(sibling(&path, WAL_SUFFIX).unwrap(), b"").unwrap();
		let unclean = Store::open(&path).await.unwrap();
		let StoreIntegrity::Failed(failure) = unclean.integrity() else {
			panic!("the damage went unnoticed after an unclean shutdown");
		};
		assert_eq!(failure.reason, IntegrityFailureReason::IntegrityCheck);
		assert!(!failure.detail.is_empty());
	}

	/// A snapshot that rotted after it was verified is checked as it is
	/// restored, whatever the files beside the copy say, and the Plane
	/// stays in read-only Recovery mode rather than serving it (ADR-0077).
	#[tokio::test]
	async fn a_restored_copy_is_checked_however_cleanly_it_arrives() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		fill_journal(&store).await;
		let snapshot = store
			.snapshot(SnapshotReason::Daily, NOW_UNIX_MS)
			.await
			.unwrap();
		store.close().await;
		damage(&path);
		damage_journal_page(
			&snapshot::snapshots_dir(&path).join(&snapshot.name),
		);

		let damaged = Store::open(&path).await.unwrap();
		damaged.restore(&snapshot.name, NOW_UNIX_MS).await.unwrap();

		let StoreIntegrity::Failed(failure) = damaged.integrity() else {
			panic!("the rotten copy was served");
		};
		assert_eq!(failure.reason, IntegrityFailureReason::IntegrityCheck);
	}

	/// A migration that fails leaves the store at its previous version and
	/// serves it read-only rather than refusing to open (ADR-0073).
	#[tokio::test]
	async fn a_failed_migration_opens_read_only_at_the_previous_version() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let mut connection = sqlx::SqliteConnection::connect_with(
			&crate::open::connect_options(&path),
		)
		.await
		.unwrap();
		// A tracker with nothing applied, plus an object the first
		// migration will trip over.
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
		sqlx::query("CREATE TABLE plane (stranger TEXT)")
			.execute(&mut connection)
			.await
			.unwrap();
		connection.close().await.unwrap();

		let store = Store::open(&path).await.unwrap();

		let StoreIntegrity::Failed(failure) = store.integrity() else {
			panic!("the failed migration went unnoticed");
		};
		assert_eq!(failure.reason, IntegrityFailureReason::Migration);
		// The rollback point was taken before the attempt.
		let rollback = SnapshotReason::Migration { applied_version: 0 };
		let reasons = |store: &Store| {
			store
				.recovery_snapshots()
				.unwrap()
				.into_iter()
				.map(|s| s.reason)
				.collect::<Vec<_>>()
		};
		assert_eq!(reasons(&store), vec![rollback]);
		// Restoring it over the intact previous version moves that aside
		// under an honest name, tries the migration again, and fails the
		// same way, with the two attempts sharing one rollback point.
		let name = store.recovery_snapshots().unwrap()[0].name.clone();
		let restored = store.restore(&name, NOW_UNIX_MS).await.unwrap();
		assert_eq!(
			(
				restored.replaced.as_str(),
				store.integrity(),
				reasons(&store).len()
			),
			(
				"plane.sqlite3.unmigrated-1700000000000",
				StoreIntegrity::Failed(failure),
				1
			)
		);
	}
}
