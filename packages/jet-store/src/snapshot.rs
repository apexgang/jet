//! Verified, bounded Recovery snapshots of the store (ADR-0097).
//!
//! A snapshot is a complete copy of the database written by SQLite itself,
//! checked for integrity before it is given its final name, so a file
//! under that name is a snapshot the Plane can restore from. Snapshots live
//! beside the database under their own directory, outside every namespace
//! disk-pressure cleanup visits (ADR-0079), and are rotated by count rather
//! than by age: seven daily, four weekly, and the two newest taken before a
//! schema migration, which are what the current and previous release pair
//! roll back to (ADR-0073).
//!
//! The Security audit head beside the database is deliberately not copied.
//! Restoring a snapshot moves authoritative state backwards, and the head
//! left in place is what makes that visible to the audit (ADR-0105).

use crate::StoreError;
use serde::{Deserialize, Serialize};
use sqlx::{
	Connection as _, SqliteConnection, SqlitePool, sqlite::SqliteConnectOptions,
};
use std::{
	fs::{self, File},
	os::unix::fs::{DirBuilderExt as _, PermissionsExt as _},
	path::{Path, PathBuf},
	sync::atomic::{AtomicBool, AtomicI64, Ordering},
	time::{SystemTime, UNIX_EPOCH},
};

/// How many distinct days keep their newest snapshot.
pub const DAILY_RETENTION: usize = 7;

/// How many distinct weeks before those days keep their newest snapshot.
pub const WEEKLY_RETENTION: usize = 4;

/// How many schema versions keep their newest pre-migration snapshot,
/// whatever its age: the one the current release migrated from and the one
/// before it (ADR-0073). Repeated attempts at one migration share a
/// version, so they cannot crowd the older rollback point out.
pub const ROLLBACK_RETENTION: usize = 2;

/// Directory beside the database that holds its snapshots.
const DIRECTORY: &str = "snapshots";

const PREFIX: &str = "plane-";
const SUFFIX: &str = ".sqlite3";

/// Prefix of a snapshot still being written or verified. It never matches
/// a snapshot name, so a crash leaves nothing that looks restorable.
const PENDING_PREFIX: &str = ".pending-";

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Why a snapshot was taken. It is part of the snapshot's name, so
/// rotation can tell a rollback point from a routine copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotReason {
	/// The first meaningful committed change of a day.
	Daily,
	/// A schema migration was about to run on a store at this version.
	Migration {
		/// The newest migration the store had applied.
		applied_version: i64,
	},
	/// Destructive maintenance was about to run.
	Maintenance,
}

impl SnapshotReason {
	fn segment(self) -> String {
		match self {
			Self::Daily => "daily".into(),
			Self::Migration { applied_version } => {
				format!("migration-{applied_version}")
			}
			Self::Maintenance => "maintenance".into(),
		}
	}

	fn parse(text: &str) -> Option<Self> {
		match text {
			"daily" => Some(Self::Daily),
			"maintenance" => Some(Self::Maintenance),
			_ => text.strip_prefix("migration-").and_then(|version| {
				(!version.is_empty()
					&& version.bytes().all(|byte| byte.is_ascii_digit()))
				.then(|| version.parse().ok())
				.flatten()
				.map(|applied_version| Self::Migration { applied_version })
			}),
		}
	}

	/// The schema version a pre-migration snapshot is the rollback point
	/// for, and `None` for a routine one.
	fn rollback_version(self) -> Option<i64> {
		match self {
			Self::Migration { applied_version } => Some(applied_version),
			Self::Daily | Self::Maintenance => None,
		}
	}
}

/// One verified snapshot on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverySnapshot {
	/// The file name under the snapshot directory, which names the
	/// snapshot in every request about it.
	pub name: String,
	/// When it was taken, in Unix milliseconds.
	pub taken_at_unix_ms: i64,
	/// Why it was taken.
	pub reason: SnapshotReason,
	/// Its size on disk.
	pub bytes: u64,
}

impl RecoverySnapshot {
	fn day(&self) -> i64 {
		self.taken_at_unix_ms.div_euclid(DAY_MS)
	}

	fn week(&self) -> i64 {
		self.day().div_euclid(7)
	}

	/// Reads a snapshot back from its file name, or `None` for a file that
	/// is not one.
	fn parse(name: &str, bytes: u64) -> Option<Self> {
		let body = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
		let (stamp, reason) = body.split_once('-')?;
		if stamp.is_empty() || !stamp.bytes().all(|byte| byte.is_ascii_digit())
		{
			return None;
		}
		Some(Self {
			name: name.to_owned(),
			taken_at_unix_ms: stamp.parse().ok()?,
			reason: SnapshotReason::parse(reason)?,
			bytes,
		})
	}
}

/// What an interactive Recovery purge did: the verified post-deletion
/// snapshot it took, and the older snapshots it removed because they may
/// hold an identity the Deletion ledger says is gone (ADR-0102).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotPurge {
	/// The snapshot taken after every recorded deletion.
	pub snapshot: String,
	/// The snapshots removed, newest first.
	pub removed: Vec<String>,
}

/// Where the snapshots of the store at `database` live.
#[must_use]
pub fn snapshots_dir(database: &Path) -> PathBuf {
	database.parent().map_or_else(
		|| PathBuf::from(DIRECTORY),
		|parent| parent.join(DIRECTORY),
	)
}

/// The wall clock as the store stamps snapshots it takes on its own,
/// before any caller with a clock exists.
pub(crate) fn wall_clock_unix_ms() -> i64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(0))
}

/// Whether the store has changed since a snapshot last captured it, and
/// which day that snapshot belongs to. Both are advisory: they decide when
/// a routine snapshot is due, never whether one is valid.
#[derive(Debug)]
pub(crate) struct Tracker {
	/// A write has committed through this store since the newest snapshot.
	/// What happened before this open is unknown and counts as nothing:
	/// a change the previous daemon made after its last snapshot is copied
	/// with the next change, or before the next maintenance, whichever
	/// comes first.
	dirty: AtomicBool,
	/// The database existed before this open. A store created just now
	/// holds nothing worth copying before maintenance.
	existed: bool,
	/// The day of the newest snapshot, or `i64::MIN` when there is none.
	day: AtomicI64,
}

impl Tracker {
	pub(crate) fn at_open(database: &Path) -> Result<Self, StoreError> {
		let newest = list(database)?.into_iter().next();
		let existed =
			fs::metadata(database).is_ok_and(|metadata| metadata.len() > 0);
		Ok(Self {
			dirty: AtomicBool::new(false),
			existed,
			day: AtomicI64::new(
				newest.map_or(i64::MIN, |snapshot| snapshot.day()),
			),
		})
	}

	pub(crate) fn mark_dirty(&self) {
		self.dirty.store(true, Ordering::Relaxed);
	}

	/// Whether a snapshot for `reason` is due at `now_unix_ms`. A daily one
	/// follows a change on a day without a snapshot; a maintenance one
	/// precedes destructive work on a day without one, changed or not, as
	/// long as the store predates this open; a pre-migration one is always
	/// due.
	pub(crate) fn is_due(
		&self,
		reason: SnapshotReason,
		now_unix_ms: i64,
	) -> bool {
		let new_day =
			self.day.load(Ordering::Relaxed) != now_unix_ms.div_euclid(DAY_MS);
		match reason {
			SnapshotReason::Daily => {
				new_day && self.dirty.load(Ordering::Relaxed)
			}
			SnapshotReason::Maintenance => new_day && self.existed,
			SnapshotReason::Migration { .. } => true,
		}
	}

	fn captured(&self, snapshot: &RecoverySnapshot) {
		self.day.store(snapshot.day(), Ordering::Relaxed);
		self.dirty.store(false, Ordering::Relaxed);
	}
}

/// Writes, verifies, and names a snapshot of the database behind `pool`,
/// then rotates the directory so the retained set stays bounded.
pub(crate) async fn create(
	pool: &SqlitePool,
	database: &Path,
	tracker: &Tracker,
	reason: SnapshotReason,
	now_unix_ms: i64,
) -> Result<RecoverySnapshot, StoreError> {
	let directory = snapshots_dir(database);
	prepare_directory(&directory)?;
	let name = format!("{PREFIX}{now_unix_ms}-{}{SUFFIX}", reason.segment());
	let pending = directory.join(format!("{PENDING_PREFIX}{name}"));
	let target = directory.join(&name);
	if target.exists() {
		return Err(StoreError::Integrity(format!(
			"snapshot {name} already exists"
		)));
	}
	remove_if_present(&pending)?;
	let Some(pending_text) = pending.to_str() else {
		return Err(StoreError::Unavailable(
			"snapshot path is not valid UTF-8".into(),
		));
	};
	// `VACUUM INTO` writes a consistent copy of the whole database from a
	// read transaction of its own, without the write-ahead log. Its
	// argument is bound, never spliced, and it runs on the runtime API
	// because the compile-time macros cannot describe VACUUM.
	if let Err(error) = sqlx::query("VACUUM INTO ?1")
		.bind(pending_text)
		.execute(pool)
		.await
	{
		remove_if_present(&pending)?;
		return Err(error.into());
	}
	if let Err(error) = verify(&pending).await {
		remove_if_present(&pending)?;
		return Err(error);
	}
	let bytes = publish(&pending, &target, &directory)?;
	let snapshot = RecoverySnapshot {
		name,
		taken_at_unix_ms: now_unix_ms,
		reason,
		bytes,
	};
	tracker.captured(&snapshot);
	rotate(database)?;
	Ok(snapshot)
}

/// Every verified snapshot of the store at `database`, newest first.
pub(crate) fn list(
	database: &Path,
) -> Result<Vec<RecoverySnapshot>, StoreError> {
	let directory = snapshots_dir(database);
	let entries = match fs::read_dir(&directory) {
		Ok(entries) => entries,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
			return Ok(vec![]);
		}
		Err(error) => return Err(unavailable(&directory, &error)),
	};
	let mut snapshots = vec![];
	for entry in entries {
		let entry = entry.map_err(|error| unavailable(&directory, &error))?;
		let Ok(name) = entry.file_name().into_string() else {
			continue;
		};
		let metadata = entry
			.metadata()
			.map_err(|error| unavailable(&directory, &error))?;
		if !metadata.is_file() {
			continue;
		}
		if let Some(snapshot) = RecoverySnapshot::parse(&name, metadata.len()) {
			snapshots.push(snapshot);
		}
	}
	snapshots.sort_by(|a, b| {
		b.taken_at_unix_ms
			.cmp(&a.taken_at_unix_ms)
			.then_with(|| b.name.cmp(&a.name))
	});
	Ok(snapshots)
}

/// The path of the verified snapshot called `name`, or an integrity error
/// naming what was asked for when there is no such snapshot. Only a name
/// the directory listing produced is accepted, so no path reaches the
/// filesystem that was not a snapshot's own (ASVS 5.3.2).
pub(crate) fn locate(
	database: &Path,
	name: &str,
) -> Result<PathBuf, StoreError> {
	let known = list(database)?
		.into_iter()
		.any(|snapshot| snapshot.name == name);
	if known {
		Ok(snapshots_dir(database).join(name))
	} else {
		Err(StoreError::Integrity(format!(
			"no verified snapshot is called {name}"
		)))
	}
}

/// The names of every snapshot taken at or before `cutoff`, newest first:
/// the ones that may still hold an identity deleted by then. With no
/// cutoff nothing is affected.
pub(crate) fn affected_by(
	database: &Path,
	cutoff: Option<i64>,
) -> Result<Vec<String>, StoreError> {
	let Some(cutoff) = cutoff else {
		return Ok(vec![]);
	};
	Ok(list(database)?
		.into_iter()
		.filter(|snapshot| snapshot.taken_at_unix_ms <= cutoff)
		.map(|snapshot| snapshot.name)
		.collect())
}

/// Removes the snapshots called `names`, whichever of them still exist.
pub(crate) fn remove(
	database: &Path,
	names: &[String],
) -> Result<(), StoreError> {
	let directory = snapshots_dir(database);
	for name in names {
		remove_if_present(&directory.join(name))?;
	}
	Ok(())
}

/// Removes every snapshot the retention tiers no longer cover, plus any
/// pending file a crash left behind.
pub(crate) fn rotate(database: &Path) -> Result<(), StoreError> {
	let directory = snapshots_dir(database);
	let snapshots = list(database)?;
	let keep = retained(&snapshots);
	for snapshot in &snapshots {
		if !keep.contains(&snapshot.name.as_str()) {
			remove_if_present(&directory.join(&snapshot.name))?;
		}
	}
	for entry in fs::read_dir(&directory)
		.map_err(|error| unavailable(&directory, &error))?
	{
		let entry = entry.map_err(|error| unavailable(&directory, &error))?;
		if entry
			.file_name()
			.to_str()
			.is_some_and(|name| name.starts_with(PENDING_PREFIX))
		{
			remove_if_present(&entry.path())?;
		}
	}
	Ok(())
}

/// The names retention keeps out of `snapshots`, which must be newest
/// first: the newest of each of the [`DAILY_RETENTION`] newest days, the
/// newest of each of the [`WEEKLY_RETENTION`] weeks before those days, and
/// the newest pre-migration snapshot of each of the [`ROLLBACK_RETENTION`]
/// newest schema versions.
fn retained(snapshots: &[RecoverySnapshot]) -> Vec<&str> {
	let mut keep = vec![];
	let mut days: Vec<i64> = vec![];
	let mut weeks: Vec<i64> = vec![];
	let mut versions: Vec<i64> = vec![];
	for snapshot in snapshots {
		let mut kept = false;
		if let Some(version) = snapshot.reason.rollback_version()
			&& versions.len() < ROLLBACK_RETENTION
			&& !versions.contains(&version)
		{
			versions.push(version);
			kept = true;
		}
		if days.len() < DAILY_RETENTION {
			if !days.contains(&snapshot.day()) {
				days.push(snapshot.day());
				kept = true;
			}
		} else if weeks.len() < WEEKLY_RETENTION
			&& !weeks.contains(&snapshot.week())
		{
			weeks.push(snapshot.week());
			kept = true;
		}
		if kept {
			keep.push(snapshot.name.as_str());
		}
	}
	keep
}

/// Opens the copy on its own and asks SQLite to check every page and
/// index. Anything but a single `ok` means the copy is not a snapshot.
async fn verify(pending: &Path) -> Result<(), StoreError> {
	let options = SqliteConnectOptions::new()
		.filename(pending)
		.create_if_missing(false);
	let mut connection = SqliteConnection::connect_with(&options).await?;
	// The macros cannot describe pragmas; see `verify_durability`.
	let report: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
		.fetch_all(&mut connection)
		.await?;
	connection.close().await?;
	let findings = crate::recovery::findings(report);
	if findings.is_empty() {
		Ok(())
	} else {
		Err(StoreError::Integrity(format!(
			"snapshot failed its integrity check with {} finding(s)",
			findings.len()
		)))
	}
}

/// Gives the verified copy its final name durably, the way the audit head
/// is published: owner-only, synced, renamed, and the directory synced so
/// the rename itself reaches disk.
fn publish(
	pending: &Path,
	target: &Path,
	directory: &Path,
) -> Result<u64, StoreError> {
	let file =
		File::open(pending).map_err(|error| unavailable(pending, &error))?;
	// ASVS 16.3.1: a copy of authoritative state is owner-only.
	file.set_permissions(fs::Permissions::from_mode(0o600))
		.map_err(|error| unavailable(pending, &error))?;
	file.sync_all()
		.map_err(|error| unavailable(pending, &error))?;
	let bytes = file
		.metadata()
		.map_err(|error| unavailable(pending, &error))?
		.len();
	drop(file);
	fs::rename(pending, target).map_err(|error| unavailable(target, &error))?;
	File::open(directory)
		.and_then(|dir| dir.sync_all())
		.map_err(|error| unavailable(directory, &error))?;
	Ok(bytes)
}

fn prepare_directory(directory: &Path) -> Result<(), StoreError> {
	fs::DirBuilder::new()
		.recursive(true)
		.mode(0o700)
		.create(directory)
		.map_err(|error| unavailable(directory, &error))
}

fn remove_if_present(path: &Path) -> Result<(), StoreError> {
	match fs::remove_file(path) {
		Ok(()) => Ok(()),
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
		Err(error) => Err(unavailable(path, &error)),
	}
}

/// An I/O failure at `path`, filed as the store being unreachable.
pub(crate) fn unavailable(path: &Path, error: &std::io::Error) -> StoreError {
	StoreError::Unavailable(format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::Store;
	use pretty_assertions::assert_eq;

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;

	const MIGRATION: SnapshotReason = SnapshotReason::Migration {
		applied_version: 20_260_910_090_010,
	};
	const OLDER_MIGRATION: SnapshotReason = SnapshotReason::Migration {
		applied_version: 20_260_904_185_326,
	};

	fn snapshot(
		taken_at_unix_ms: i64,
		reason: SnapshotReason,
	) -> RecoverySnapshot {
		RecoverySnapshot {
			name: format!(
				"{PREFIX}{taken_at_unix_ms}-{}{SUFFIX}",
				reason.segment()
			),
			taken_at_unix_ms,
			reason,
			bytes: 0,
		}
	}

	fn names(snapshots: &[RecoverySnapshot]) -> Vec<&str> {
		snapshots.iter().map(|s| s.name.as_str()).collect()
	}

	/// What was taken up to the newest deletion is affected, whatever its
	/// reason; what came after is not, and a Plane with no deletions has
	/// nothing affected at all.
	#[tokio::test]
	async fn the_snapshots_a_deletion_may_survive_in_are_the_older_ones() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		for (offset, reason) in [
			(2, SnapshotReason::Daily),
			(1, MIGRATION),
			(0, SnapshotReason::Daily),
		] {
			store
				.snapshot(reason, NOW_UNIX_MS - offset * DAY_MS)
				.await
				.unwrap();
		}
		let untouched = affected_by(&path, None).unwrap();

		let affected = affected_by(&path, Some(NOW_UNIX_MS - DAY_MS)).unwrap();
		remove(&path, &affected).unwrap();

		assert_eq!(
			(untouched, affected, names(&list(&path).unwrap())),
			(
				vec![],
				vec![
					snapshot(NOW_UNIX_MS - DAY_MS, MIGRATION).name,
					snapshot(NOW_UNIX_MS - 2 * DAY_MS, SnapshotReason::Daily)
						.name,
				],
				vec![
					snapshot(NOW_UNIX_MS, SnapshotReason::Daily).name.as_str()
				]
			)
		);
	}

	#[test]
	fn a_snapshot_name_round_trips_and_rejects_strangers() {
		let parsed = RecoverySnapshot::parse(
			"plane-1700000000000-migration-20260910090010.sqlite3",
			42,
		);
		assert_eq!(
			parsed,
			Some(RecoverySnapshot {
				name: "plane-1700000000000-migration-20260910090010.sqlite3"
					.into(),
				taken_at_unix_ms: 1_700_000_000_000,
				reason: MIGRATION,
				bytes: 42,
			})
		);
		for stranger in [
			"plane.sqlite3",
			".pending-plane-1700000000000-daily.sqlite3",
			"plane-abc-daily.sqlite3",
			"plane-1700000000000-hourly.sqlite3",
			"plane-1700000000000-migration.sqlite3",
			"plane-1700000000000-migration-x.sqlite3",
			"plane--daily.sqlite3",
		] {
			assert_eq!(
				RecoverySnapshot::parse(stranger, 0),
				None,
				"{stranger}"
			);
		}
	}

	/// Seven days keep their newest copy, the four weeks before them keep
	/// one each, and the two newest schema versions keep their newest
	/// pre-migration snapshot however old it is, so repeated attempts at
	/// one migration cannot crowd the older rollback point out (ADR-0097,
	/// ADR-0073).
	#[test]
	fn retention_keeps_daily_weekly_and_rollback_tiers() {
		let day = |offset: i64| NOW_UNIX_MS - offset * DAY_MS;
		let mut snapshots = vec![
			// Two on the newest day: only the newer one is a daily.
			snapshot(day(0) + 1, SnapshotReason::Daily),
			snapshot(day(0), SnapshotReason::Maintenance),
			snapshot(day(1), SnapshotReason::Daily),
			snapshot(day(2), SnapshotReason::Daily),
			snapshot(day(3), SnapshotReason::Daily),
			snapshot(day(4), SnapshotReason::Daily),
			snapshot(day(5), SnapshotReason::Daily),
			snapshot(day(6), SnapshotReason::Daily),
			// Older than the daily window.
			snapshot(day(7), SnapshotReason::Daily),
			snapshot(day(14), SnapshotReason::Daily),
			snapshot(day(15), SnapshotReason::Daily),
			snapshot(day(21), SnapshotReason::Daily),
			snapshot(day(28), SnapshotReason::Daily),
			snapshot(day(35), SnapshotReason::Daily),
			// Rollback points: two attempts at the newest migration, the
			// version before it, and one beyond the pair.
			snapshot(day(40), MIGRATION),
			snapshot(day(41), MIGRATION),
			snapshot(day(90), OLDER_MIGRATION),
			snapshot(
				day(200),
				SnapshotReason::Migration { applied_version: 1 },
			),
		];
		snapshots.sort_by_key(|s| std::cmp::Reverse(s.taken_at_unix_ms));
		let kept = retained(&snapshots);
		let expected: Vec<String> = [
			(day(0) + 1, SnapshotReason::Daily),
			(day(1), SnapshotReason::Daily),
			(day(2), SnapshotReason::Daily),
			(day(3), SnapshotReason::Daily),
			(day(4), SnapshotReason::Daily),
			(day(5), SnapshotReason::Daily),
			(day(6), SnapshotReason::Daily),
			// One per week among what is older than the daily window; the
			// day after each is the same week and goes.
			(day(7), SnapshotReason::Daily),
			(day(14), SnapshotReason::Daily),
			(day(21), SnapshotReason::Daily),
			(day(28), SnapshotReason::Daily),
			(day(40), MIGRATION),
			(day(90), OLDER_MIGRATION),
		]
		.into_iter()
		.map(|(stamp, reason)| snapshot(stamp, reason).name)
		.collect();
		let mut kept: Vec<String> =
			kept.into_iter().map(String::from).collect();
		let mut expected = expected;
		kept.sort();
		expected.sort();
		assert_eq!(kept, expected);
	}

	#[tokio::test]
	async fn a_snapshot_is_verified_published_and_listed() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let taken = store
			.snapshot(SnapshotReason::Daily, NOW_UNIX_MS)
			.await
			.unwrap();
		let listed = store.recovery_snapshots().unwrap();
		assert_eq!(listed, vec![taken.clone()]);
		assert_eq!(taken.reason, SnapshotReason::Daily);
		let file = snapshots_dir(&path).join(&taken.name);
		let metadata = std::fs::metadata(&file).unwrap();
		assert_eq!(
			(metadata.permissions().mode() & 0o777, metadata.len()),
			(0o600, taken.bytes)
		);
		assert_eq!(
			std::fs::metadata(snapshots_dir(&path))
				.unwrap()
				.permissions()
				.mode() & 0o777,
			0o700
		);
		// The copy opens as a store of its own.
		let copy = Store::open(&file).await.unwrap();
		assert_eq!(copy.plane().await.unwrap(), store.plane().await.unwrap());
	}

	#[tokio::test]
	async fn a_routine_snapshot_is_due_once_per_day_after_a_change() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		// A store created just now holds nothing worth copying.
		assert_eq!(
			store
				.snapshot_if_due(SnapshotReason::Daily, NOW_UNIX_MS)
				.await
				.unwrap(),
			None
		);
		store
			.write(async |_tx| Ok::<(), StoreError>(()))
			.await
			.unwrap();
		let first = store
			.snapshot_if_due(SnapshotReason::Daily, NOW_UNIX_MS)
			.await
			.unwrap();
		assert!(first.is_some());
		// Nothing changed: the same day and the next both stay quiet.
		assert_eq!(
			store
				.snapshot_if_due(SnapshotReason::Daily, NOW_UNIX_MS + 1)
				.await
				.unwrap(),
			None
		);
		assert_eq!(
			store
				.snapshot_if_due(SnapshotReason::Daily, NOW_UNIX_MS + DAY_MS)
				.await
				.unwrap(),
			None
		);
		store.record_daemon_start().await.unwrap();
		// A daemon start bypasses the write path and is not a change.
		assert_eq!(
			store
				.snapshot_if_due(SnapshotReason::Daily, NOW_UNIX_MS + DAY_MS)
				.await
				.unwrap(),
			None
		);
		store
			.write(async |_tx| Ok::<(), StoreError>(()))
			.await
			.unwrap();
		// Changed, but the day already has its snapshot.
		assert_eq!(
			store
				.snapshot_if_due(SnapshotReason::Daily, NOW_UNIX_MS + 1)
				.await
				.unwrap(),
			None
		);
		// A store created in this process owes maintenance nothing yet; the
		// change it holds is copied as the next day's daily snapshot.
		assert_eq!(
			store
				.snapshot_if_due(
					SnapshotReason::Maintenance,
					NOW_UNIX_MS + DAY_MS
				)
				.await
				.unwrap(),
			None
		);
		let next = store
			.snapshot_if_due(SnapshotReason::Daily, NOW_UNIX_MS + DAY_MS)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(
			names(&store.recovery_snapshots().unwrap()),
			vec![next.name.as_str(), first.unwrap().name.as_str()]
		);
	}

	#[tokio::test]
	async fn reopening_remembers_the_newest_snapshot_day() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		store
			.snapshot(SnapshotReason::Daily, NOW_UNIX_MS)
			.await
			.unwrap();
		store.close().await;
		let reopened = Store::open(&path).await.unwrap();
		// Nothing has changed through this store yet, and the day is
		// taken: only maintenance on a new day is owed a copy.
		for (reason, now) in [
			(SnapshotReason::Daily, NOW_UNIX_MS + 1),
			(SnapshotReason::Daily, NOW_UNIX_MS + DAY_MS),
			(SnapshotReason::Maintenance, NOW_UNIX_MS + 1),
		] {
			assert_eq!(
				reopened.snapshot_if_due(reason, now).await.unwrap(),
				None,
				"{reason:?} at {now}"
			);
		}
		assert!(
			reopened
				.snapshot_if_due(
					SnapshotReason::Maintenance,
					NOW_UNIX_MS + DAY_MS
				)
				.await
				.unwrap()
				.is_some()
		);
	}

	#[tokio::test]
	async fn rotation_removes_what_retention_drops_and_stale_pending_files() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		for offset in (0..=DAILY_RETENTION as i64).rev() {
			store
				.snapshot(SnapshotReason::Daily, NOW_UNIX_MS - offset * DAY_MS)
				.await
				.unwrap();
		}
		let directory = snapshots_dir(&path);
		std::fs::write(directory.join(".pending-plane-1-daily.sqlite3"), b"x")
			.unwrap();
		std::fs::write(directory.join("notes.txt"), b"kept").unwrap();
		store
			.snapshot(SnapshotReason::Daily, NOW_UNIX_MS + DAY_MS)
			.await
			.unwrap();
		let listed = store.recovery_snapshots().unwrap();
		// Eight dailies: the newest seven days, plus the one before them
		// becomes the week's representative.
		assert_eq!(listed.len(), DAILY_RETENTION + 1);
		let mut remaining: Vec<String> = std::fs::read_dir(&directory)
			.unwrap()
			.map(|entry| entry.unwrap().file_name().into_string().unwrap())
			.collect();
		remaining.sort();
		let mut expected: Vec<String> =
			listed.iter().map(|s| s.name.clone()).collect();
		expected.push("notes.txt".into());
		expected.sort();
		assert_eq!(remaining, expected);
	}

	#[tokio::test]
	async fn a_copy_that_fails_verification_is_not_published() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let directory = snapshots_dir(&path);
		std::fs::create_dir_all(&directory).unwrap();
		let pending = directory.join(".pending-plane-7-daily.sqlite3");
		std::fs::write(&pending, b"not a database").unwrap();
		let error = verify(&pending).await.unwrap_err();
		assert!(matches!(error, StoreError::Integrity(_)), "{error:?}");
		// A real snapshot still lands beside it and rotation clears it.
		store
			.snapshot(SnapshotReason::Daily, NOW_UNIX_MS)
			.await
			.unwrap();
		assert!(!pending.exists());
	}
}
