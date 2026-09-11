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
use sqlx::{SqlitePool, migrate::Migrator};

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
	/// This build knows a migration the store has not applied.
	Behind,
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
		SchemaState::Behind
	} else {
		SchemaState::Current
	})
}
