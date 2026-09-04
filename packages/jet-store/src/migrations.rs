//! Forward-only schema migrations tracked in `_sqlx_migrations`
//! (ADR-0073).
//!
//! Each migration commits together with its bookkeeping row in one
//! transaction, so a failure leaves the store at the previous version, and
//! an older `jetd` opens a newer store by skipping versions it does not
//! know. Schema changes are expand-only until the rollback window of the
//! release that introduced them has passed. The verified Recovery snapshot
//! that precedes a migration (ADR-0097) arrives with the recovery work;
//! until then a pre-existing store is migrated in place.

use sqlx::SqlitePool;
use sqlx::migrate::Migrator;

use crate::StoreError;

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
