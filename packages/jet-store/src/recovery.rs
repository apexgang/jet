//! Read-only Recovery of the store itself: the integrity check every open
//! runs, what the store looks like when it fails, and restoring a verified
//! Recovery snapshot over the damaged database (ADR-0077).
//!
//! A store that fails its check still opens. Reads work as far as the
//! damage allows, so the Plane can be diagnosed and exported, and nothing
//! writes: the damaged database is preserved exactly as found until an
//! owner restores a snapshot over it. Restoring moves the damaged files
//! aside rather than deleting them, and reopens the restored copy through
//! the same checks as any other open.

use crate::{
	Opened, StoreError,
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

/// The companions SQLite keeps beside a database in write-ahead-log mode.
const JOURNAL_SUFFIXES: [&str; 2] = ["-wal", "-shm"];

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
	/// `PRAGMA quick_check` reported damage.
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

/// Runs SQLite's lightweight check over the database behind `pool` and
/// returns what it found, which is empty when the database is sound.
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
/// into their place, and reopens it.
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
	let opened = crate::open::connect(database, snapshots).await?;
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
mod tests {
	use super::*;
	use crate::{SnapshotReason, Store};
	use pretty_assertions::assert_eq;
	use sqlx::Connection as _;
	use std::io::{Seek as _, SeekFrom, Write as _};

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;

	/// Overwrites the second page of a closed database, which holds schema
	/// or table content in any store this crate creates.
	fn damage(path: &Path) {
		let mut file = fs::OpenOptions::new().write(true).open(path).unwrap();
		file.seek(SeekFrom::Start(4096)).unwrap();
		file.write_all(&[0xff; 4096]).unwrap();
		file.sync_all().unwrap();
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
