//! Forward-only schema migrations tracked in `_sqlx_migrations`
//! (ADR-0073).
//!
//! Each migration commits together with its bookkeeping row in one
//! transaction, so a failure leaves the store at the previous version, and
//! an older `jetd` opens a newer store by skipping versions it does not
//! know. Schema changes are expand-only until the rollback window of the
//! release that introduced them has passed, and [`Store::open`] takes a
//! verified Recovery snapshot before applying anything to a store that has
//! a schema already (ADR-0097).
//!
//! [`Store::open`]: crate::Store::open

use crate::StoreError;
use sqlx::{
	Connection as _, SqliteConnection, SqlitePool, migrate::Migrator,
	sqlite::SqliteConnectOptions,
};
use std::path::Path;

/// The migration set embedded in this build.
///
/// Ignoring missing versions is what lets an older `jetd` open a store a
/// newer release migrated: the migrator otherwise refuses the whole run
/// before executing anything. A version this build does know whose file
/// changed after it was applied is still rejected, because the checksum
/// comparison runs either way.
fn migrator() -> Migrator {
	let mut migrator = sqlx::migrate!("./migrations");
	migrator.set_ignore_missing(true);
	migrator
}

/// The newest migration version embedded in this build: the schema a
/// store is at once this release has opened it (ADR-0073). A release
/// payload records it so an installer can tell whether the release before
/// this one can still open the store.
#[must_use]
pub fn embedded_schema_version() -> i64 {
	migrator().iter().map(|m| m.version).max().unwrap_or(0)
}

/// The newest migration version the store file at `path` has applied,
/// read without migrating, checking, or otherwise touching it. `None` when
/// there is no file or no schema tracker yet.
///
/// # Errors
///
/// Returns [`StoreError`] when the file exists but cannot be read.
pub async fn applied_schema_version(
	path: &Path,
) -> Result<Option<i64>, StoreError> {
	if !path.exists() {
		return Ok(None);
	}
	let mut connection = SqliteConnection::connect_with(
		&SqliteConnectOptions::new().filename(path).read_only(true),
	)
	.await?;
	// Both statements run before the schema the compile-time query cache
	// describes is known to exist, so they use the runtime API.
	let tracked: Option<String> = sqlx::query_scalar(
		"SELECT name FROM sqlite_master
		 WHERE type = 'table' AND name = '_sqlx_migrations'",
	)
	.fetch_optional(&mut connection)
	.await?;
	if tracked.is_none() {
		return Ok(None);
	}
	let applied: Option<i64> = sqlx::query_scalar(
		"SELECT MAX(version) FROM _sqlx_migrations WHERE success",
	)
	.fetch_one(&mut connection)
	.await?;
	connection.close().await?;
	Ok(applied)
}

pub(crate) async fn apply(pool: &SqlitePool) -> Result<(), StoreError> {
	Ok(migrator().run(pool).await.map_err(sqlx::Error::from)?)
}

/// Where a store's schema stands against the migration set of this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchemaState {
	/// No tracker table: the file has no schema yet.
	Absent,
	/// Every migration this build knows has been applied.
	Current,
	/// This build knows a migration the store has not applied. The newest
	/// version the store has applied names the rollback point a snapshot
	/// taken now is for; it is zero for a tracker with nothing applied.
	Behind { applied_version: i64 },
}

/// Reads the [`SchemaState`] of the store behind `pool`.
///
/// Both statements run before the schema this crate's compile-time query
/// cache describes exists, so they use the runtime API.
pub(crate) async fn state(
	pool: &SqlitePool,
) -> Result<SchemaState, StoreError> {
	let tracked: Option<String> = sqlx::query_scalar(
		"SELECT name FROM sqlite_master
		 WHERE type = 'table' AND name = '_sqlx_migrations'",
	)
	.fetch_optional(pool)
	.await?;
	if tracked.is_none() {
		return Ok(SchemaState::Absent);
	}
	let applied: Vec<i64> = sqlx::query_scalar(
		"SELECT version FROM _sqlx_migrations WHERE success",
	)
	.fetch_all(pool)
	.await?;
	let behind = migrator()
		.iter()
		.any(|migration| !applied.contains(&migration.version));
	Ok(if behind {
		SchemaState::Behind {
			applied_version: applied.into_iter().max().unwrap_or(0),
		}
	} else {
		SchemaState::Current
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	/// A file with no tracker has no schema; a migrated store is current;
	/// a tracker missing the newest version is behind, naming the version
	/// it last applied.
	#[tokio::test]
	async fn the_schema_state_follows_the_tracker() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let pool =
			sqlx::SqlitePool::connect_with(crate::open::connect_options(&path))
				.await
				.unwrap();
		let absent = state(&pool).await.unwrap();
		apply(&pool).await.unwrap();
		let current = state(&pool).await.unwrap();
		let newest = migrator().iter().map(|m| m.version).max().unwrap();
		let previous = migrator()
			.iter()
			.map(|m| m.version)
			.filter(|version| *version < newest)
			.max()
			.unwrap();
		sqlx::query("DELETE FROM _sqlx_migrations WHERE version = ?1")
			.bind(newest)
			.execute(&pool)
			.await
			.unwrap();
		let behind = state(&pool).await.unwrap();
		pool.close().await;
		let _ = sqlx::SqliteConnection::connect_with(
			&crate::open::connect_options(&path),
		)
		.await
		.unwrap();
		assert_eq!(
			(absent, current, behind),
			(
				SchemaState::Absent,
				SchemaState::Current,
				SchemaState::Behind {
					applied_version: previous
				}
			)
		);
	}

	/// A store the previous release left at its schema opens through this
	/// build, migrates forward, and is authoritative afterwards: the
	/// expand-only forward step ADR-0073 relies on.
	#[tokio::test]
	async fn a_store_at_the_previous_schema_migrates_forward_on_open() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let newest = embedded_schema_version();
		let previous = Migrator {
			migrations: std::borrow::Cow::Owned(
				migrator()
					.iter()
					.filter(|migration| migration.version < newest)
					.cloned()
					.collect(),
			),
			..migrator()
		};
		let previous_version =
			previous.iter().map(|m| m.version).max().unwrap();
		let pool =
			sqlx::SqlitePool::connect_with(crate::open::connect_options(&path))
				.await
				.unwrap();
		previous.run(&pool).await.unwrap();
		pool.close().await;
		let before = applied_schema_version(&path).await.unwrap();

		let store = crate::Store::open(&path).await.unwrap();
		let integrity = store.integrity();
		let plane_readable = store.plane().await.is_ok();
		store.close().await;
		let after = applied_schema_version(&path).await.unwrap();
		assert_eq!(
			(before, after, integrity, plane_readable),
			(
				Some(previous_version),
				Some(newest),
				crate::StoreIntegrity::Verified,
				true
			)
		);
	}

	/// A missing file and an untracked file report no schema; a migrated
	/// store reports the version this build embeds, and the read leaves the
	/// store as it was.
	#[tokio::test]
	async fn the_applied_version_is_read_without_migrating() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let missing = applied_schema_version(&path).await.unwrap();
		let pool =
			sqlx::SqlitePool::connect_with(crate::open::connect_options(&path))
				.await
				.unwrap();
		let untracked = applied_schema_version(&path).await.unwrap();
		apply(&pool).await.unwrap();
		let migrated = applied_schema_version(&path).await.unwrap();
		let still_current = state(&pool).await.unwrap();
		pool.close().await;
		assert_eq!(
			(missing, untracked, migrated, still_current),
			(
				None,
				None,
				Some(embedded_schema_version()),
				SchemaState::Current
			)
		);
	}
}
