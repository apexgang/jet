//! The deep check of the live store, owed after a change and run while the
//! Plane is idle: SQLite's `PRAGMA integrity_check`, one table at a time,
//! so that new work finds the check between two tables rather than behind
//! them (ADR-0077, ADR-0055).
//!
//! The lightweight page check belongs to an open after an unclean
//! shutdown and the full check to every snapshot as it is taken; neither
//! looks at the live store once it serves. This one does, on a connection
//! of its own, so the store's one connection is never held through a
//! Command, and never on a timer: a write commits, and the check is owed
//! the next time the Plane is idle, at most once a day. Damage puts the
//! store in read-only Recovery mode as an open would, with the database
//! preserved exactly as found.

use crate::{
	IntegrityFailure, IntegrityFailureReason, Store, StoreError,
	StoreIntegrity, open, recovery, snapshot::DAY_MS,
};
use sqlx::{Connection as _, SqliteConnection};
use std::{
	path::Path,
	sync::atomic::{AtomicBool, AtomicI64, Ordering},
};

/// What one call to [`crate::Store::deep_check`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeepCheck {
	/// Nothing is owed: no write has committed since a check last began,
	/// one completed today already, or the store answers reads only.
	NotDue,
	/// Every table was checked and none is damaged.
	Passed,
	/// Work arrived between two tables: what was checked is discarded and
	/// the check is owed again.
	Abandoned,
	/// A table is damaged, and the store answers reads only from now on.
	Damaged,
}

/// Whether a deep check is owed, and which day the last completed one
/// belongs to. Advisory, like the snapshot tracker: it decides when a
/// check runs, never what one finds.
#[derive(Debug)]
pub(crate) struct Tracker {
	/// A write has committed through this store since a check last began.
	/// What happened before this open is unknown, so every open owes one
	/// check.
	dirty: AtomicBool,
	/// The day of the last completed check, or `i64::MIN` when none has
	/// completed since this open.
	day: AtomicI64,
}

impl Tracker {
	pub(crate) fn at_open() -> Self {
		Self {
			dirty: AtomicBool::new(true),
			day: AtomicI64::new(i64::MIN),
		}
	}

	pub(crate) fn mark_dirty(&self) {
		self.dirty.store(true, Ordering::Relaxed);
	}

	/// Whether a check is owed at `now_unix_ms`: a write has committed
	/// since one last began, and none completed today.
	pub(crate) fn is_due(&self, now_unix_ms: i64) -> bool {
		self.dirty.load(Ordering::Relaxed)
			&& self.day.load(Ordering::Relaxed)
				!= now_unix_ms.div_euclid(DAY_MS)
	}

	/// Takes the check when one is due. The mark is cleared as it begins,
	/// so a write that commits while it runs shows as an interruption.
	fn begin(&self, now_unix_ms: i64) -> bool {
		if self.day.load(Ordering::Relaxed) == now_unix_ms.div_euclid(DAY_MS) {
			return false;
		}
		self.dirty.swap(false, Ordering::Relaxed)
	}

	/// Whether a write committed since the check began.
	fn interrupted(&self) -> bool {
		self.dirty.load(Ordering::Relaxed)
	}

	/// Leaves the check owed again.
	fn abandon(&self) {
		self.mark_dirty();
	}

	fn complete(&self, now_unix_ms: i64) {
		self.day
			.store(now_unix_ms.div_euclid(DAY_MS), Ordering::Relaxed);
	}
}

impl Store {
	/// Whether a deep check is owed at `now_unix_ms`: a write has committed
	/// since one last began, and none completed today. Cheap enough to ask
	/// on every wake before deciding whether the Plane is idle enough to
	/// run one.
	#[must_use]
	pub fn deep_check_due(&self, now_unix_ms: i64) -> bool {
		self.integrity() == StoreIntegrity::Verified
			&& self.deep_checks.is_due(now_unix_ms)
	}

	/// Runs SQLite's `integrity_check` over the live store when one is
	/// owed, the schema and then one table at a time on a connection of
	/// its own, so the store's one connection is never held through a
	/// Command (ADR-0077). Every table and index is checked in full; only
	/// SQLite's accounting of free pages, which holds no data, is not. The
	/// caller decides that the Plane is idle before calling; between
	/// tables, a write that committed meanwhile or `interrupted` answering
	/// `true` abandons the check, which is then owed again. A completed
	/// check is not owed again until a write commits on a later day, and
	/// nothing here sets a timer (ADR-0055).
	///
	/// Damage puts the store in read-only Recovery mode exactly as a
	/// failed open does: [`Store::integrity`] reports it, nothing writes,
	/// and the database is preserved as found until a verified snapshot is
	/// restored over it.
	///
	/// # Errors
	///
	/// Returns [`StoreError::Unavailable`] when the checking connection
	/// cannot be opened or the store cannot be reached, in which case the
	/// check is owed again.
	pub async fn deep_check(
		&self,
		now_unix_ms: i64,
		interrupted: impl Fn() -> bool,
	) -> Result<DeepCheck, StoreError> {
		if !self.deep_check_due(now_unix_ms)
			|| !self.deep_checks.begin(now_unix_ms)
		{
			return Ok(DeepCheck::NotDue);
		}
		match run(&self.database, &self.deep_checks, now_unix_ms, interrupted)
			.await?
		{
			Outcome::Passed => Ok(DeepCheck::Passed),
			Outcome::Abandoned => Ok(DeepCheck::Abandoned),
			Outcome::Damaged(detail) => {
				self.opened
					.write()
					.expect("store state is not poisoned")
					.integrity = StoreIntegrity::Failed(IntegrityFailure {
					reason: IntegrityFailureReason::IntegrityCheck,
					detail,
				});
				Ok(DeepCheck::Damaged)
			}
		}
	}
}

/// What a check that ran found.
enum Outcome {
	Passed,
	Abandoned,
	/// SQLite's own account of the damage, for local diagnostics only
	/// (ADR-0061).
	Damaged(String),
}

/// How one table's check ended.
enum Step {
	Sound,
	Interrupted,
	Damaged(String),
}

/// Runs the check over the database at `database`, on a connection of its
/// own, once `tracker` has taken it. Between tables, the tracker's own
/// mark and `interrupted` decide whether to go on.
///
/// A statement that fails on content rather than reachability is damage,
/// like one met on the way to serving the store; an unreachable store
/// leaves the check owed and reports the error.
async fn run(
	database: &Path,
	tracker: &Tracker,
	now_unix_ms: i64,
	interrupted: impl Fn() -> bool,
) -> Result<Outcome, StoreError> {
	let mut connection = match SqliteConnection::connect_with(
		&open::connect_options(database).create_if_missing(false),
	)
	.await
	{
		Ok(connection) => connection,
		Err(error) => {
			tracker.abandon();
			return Err(error.into());
		}
	};
	let checked = check_tables(&mut connection, tracker, &interrupted).await;
	// The connection served the check alone; a close that fails changes
	// nothing about what the check found.
	let _ = connection.close().await;
	match checked {
		Ok(Step::Sound) => {
			tracker.complete(now_unix_ms);
			Ok(Outcome::Passed)
		}
		Ok(Step::Interrupted) => {
			tracker.abandon();
			Ok(Outcome::Abandoned)
		}
		Ok(Step::Damaged(detail)) | Err(StoreError::Integrity(detail)) => {
			Ok(Outcome::Damaged(detail))
		}
		Err(error) => {
			tracker.abandon();
			Err(error)
		}
	}
}

/// Checks the schema and then every table, the schema first because the
/// tables are read from it.
async fn check_tables(
	connection: &mut SqliteConnection,
	tracker: &Tracker,
	interrupted: &impl Fn() -> bool,
) -> Result<Step, StoreError> {
	// `sqlite_schema` is read before anything the compile-time query
	// cache describes, so this runs on the runtime API.
	let tables: Vec<String> = sqlx::query_scalar(
		"SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name",
	)
	.fetch_all(&mut *connection)
	.await?;
	for table in std::iter::once("sqlite_schema".to_owned()).chain(tables) {
		if tracker.interrupted() || interrupted() {
			return Ok(Step::Interrupted);
		}
		let findings = check_table(connection, &table).await?;
		if !findings.is_empty() {
			return Ok(Step::Damaged(findings.join("; ")));
		}
	}
	Ok(Step::Sound)
}

/// Runs SQLite's full check over one table and its indexes, and returns
/// what it found, which is empty when the table is sound.
///
/// The macros cannot describe pragmas, and a pragma takes no bound
/// parameter, so the name is quoted as a string literal. It came out of
/// the schema this crate's own migrations wrote, and the quoting keeps
/// any name inside the literal (ASVS 5.3.4).
async fn check_table(
	connection: &mut SqliteConnection,
	table: &str,
) -> Result<Vec<String>, StoreError> {
	let report: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
		"PRAGMA integrity_check('{}')",
		table.replace('\'', "''")
	)))
	.fetch_all(connection)
	.await?;
	Ok(recovery::findings(report))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		IntegrityFailureReason, SnapshotReason, Store, StoreIntegrity,
		recovery::tests::{damage_journal_page, fill_journal},
	};
	use pretty_assertions::assert_eq;
	use std::sync::atomic::AtomicUsize;

	/// The start of a day, so that a stamp one day on is the next day and
	/// one millisecond earlier is the same day.
	const NOW_UNIX_MS: i64 = 1_700_000_000_000 / DAY_MS * DAY_MS;

	fn never() -> bool {
		false
	}

	/// Every open owes a check; after that a change owes one, once a
	/// day, and a completed one is not owed again until the next change
	/// on a later day.
	#[test]
	fn a_check_is_owed_by_an_open_and_by_a_change_once_a_day() {
		let tracker = Tracker::at_open();
		assert!(tracker.is_due(NOW_UNIX_MS));
		assert!(tracker.begin(NOW_UNIX_MS));
		// Taken: nothing is due until a later write, and a second taker
		// gets nothing.
		assert!(!tracker.is_due(NOW_UNIX_MS));
		assert!(!tracker.begin(NOW_UNIX_MS));
		tracker.complete(NOW_UNIX_MS);
		assert!(!tracker.is_due(NOW_UNIX_MS + DAY_MS));
		tracker.mark_dirty();
		assert!(!tracker.is_due(NOW_UNIX_MS + DAY_MS - 1));
		assert!(tracker.is_due(NOW_UNIX_MS + DAY_MS));
	}

	/// A write during a check interrupts it, and an abandoned check is
	/// owed again at once.
	#[test]
	fn a_write_during_a_check_interrupts_it_and_leaves_it_owed() {
		let tracker = Tracker::at_open();
		assert!(tracker.begin(NOW_UNIX_MS));
		assert!(!tracker.interrupted());
		tracker.mark_dirty();
		assert!(tracker.interrupted());
		tracker.abandon();
		assert!(tracker.is_due(NOW_UNIX_MS));
	}

	/// The deep check is owed by the open and passes over a sound store;
	/// a write the same day owes nothing more, and one the day after owes
	/// another.
	#[tokio::test]
	async fn a_sound_store_passes_its_deep_check_once_a_day() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		fill_journal(&store).await;
		assert_eq!(
			(
				store.deep_check(NOW_UNIX_MS, never).await.unwrap(),
				store.deep_check(NOW_UNIX_MS, never).await.unwrap(),
				store.integrity(),
			),
			(
				DeepCheck::Passed,
				DeepCheck::NotDue,
				StoreIntegrity::Verified
			)
		);
		assert!(!store.deep_check_due(NOW_UNIX_MS + DAY_MS));
		fill_journal(&store).await;
		let same_day = NOW_UNIX_MS + DAY_MS - 1;
		let next_day = NOW_UNIX_MS + DAY_MS;
		assert_eq!(
			(
				store.deep_check(same_day, never).await.unwrap(),
				store.deep_check(next_day, never).await.unwrap(),
			),
			(DeepCheck::NotDue, DeepCheck::Passed)
		);
	}

	/// Damage on a page nothing reads at open is served after a clean
	/// close, and found by the deep check, which puts the store in
	/// read-only Recovery mode with the damaged file exactly as it was
	/// (ADR-0077).
	#[tokio::test]
	async fn the_deep_check_finds_damage_the_open_did_not_and_preserves_it() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		fill_journal(&store).await;
		let snapshot = store
			.snapshot(SnapshotReason::Daily, NOW_UNIX_MS)
			.await
			.unwrap();
		store.close().await;
		damage_journal_page(&path);
		let damaged_bytes = std::fs::read(&path).unwrap();

		let store = Store::open(&path).await.unwrap();
		assert_eq!(store.integrity(), StoreIntegrity::Verified);
		assert_eq!(
			store.deep_check(NOW_UNIX_MS, never).await.unwrap(),
			DeepCheck::Damaged
		);
		let StoreIntegrity::Failed(failure) = store.integrity() else {
			panic!("the damage went unnoticed");
		};
		assert_eq!(failure.reason, IntegrityFailureReason::IntegrityCheck);
		assert!(!failure.detail.is_empty());
		// Nothing writes to a store in Recovery mode, and nothing is owed
		// of it either.
		let refused = store.record_daemon_start().await.unwrap_err();
		assert!(matches!(refused, StoreError::Unavailable(_)), "{refused}");
		let refused = store
			.write(async |_| Ok::<(), StoreError>(()))
			.await
			.unwrap_err();
		assert!(matches!(refused, StoreError::Unavailable(_)), "{refused}");
		assert_eq!(
			store.deep_check(NOW_UNIX_MS + DAY_MS, never).await.unwrap(),
			DeepCheck::NotDue
		);
		assert_eq!(std::fs::read(&path).unwrap(), damaged_bytes);
		let moved_aside = std::fs::read_dir(dir.path())
			.unwrap()
			.filter_map(|entry| entry.ok()?.file_name().into_string().ok())
			.filter(|name| name.contains(".damaged-"))
			.count();
		assert_eq!(moved_aside, 0);
		// The way out is the same as after a failed open.
		store.restore(&snapshot.name, NOW_UNIX_MS).await.unwrap();
		assert_eq!(store.integrity(), StoreIntegrity::Verified);
		store.record_daemon_start().await.unwrap();
	}

	/// New work between two tables abandons the check without touching
	/// the store's own connection, and the check is owed again.
	#[tokio::test]
	async fn new_work_between_tables_abandons_the_check() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let boundaries = AtomicUsize::new(0);
		let after_two_tables =
			|| boundaries.fetch_add(1, Ordering::Relaxed) == 2;
		assert_eq!(
			store
				.deep_check(NOW_UNIX_MS, after_two_tables)
				.await
				.unwrap(),
			DeepCheck::Abandoned
		);
		assert_eq!(boundaries.load(Ordering::Relaxed), 3);
		// Owed again, and the store's connection was free throughout.
		store.record_daemon_start().await.unwrap();
		assert_eq!(
			store.deep_check(NOW_UNIX_MS, never).await.unwrap(),
			DeepCheck::Passed
		);
	}
}
