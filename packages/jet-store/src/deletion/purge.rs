//! Which Recovery snapshots a deletion may survive in, and removing them
//! once a post-deletion snapshot exists (ADR-0102).
//!
//! A snapshot is affected when the store it copies had applied fewer
//! ledger records than the ledger now vouches for. The count travels
//! inside the copy, written in the same commit as each deletion, so it
//! cannot disagree with the copy's content the way a stamp compared to a
//! deletion time could.

use crate::{StoreError, plane, snapshot};
use sqlx::{Connection as _, SqliteConnection, sqlite::SqliteConnectOptions};
use std::path::Path;

/// The names of every snapshot that had applied fewer than `vouched`
/// ledger records when it was taken, newest first.
pub(crate) async fn affected_snapshots(
	database: &Path,
	vouched: u64,
) -> Result<Vec<String>, StoreError> {
	let directory = snapshot::snapshots_dir(database);
	let mut affected = vec![];
	for snapshot in snapshot::list(database)? {
		if deletions_applied(&directory.join(&snapshot.name)).await < vouched {
			affected.push(snapshot.name);
		}
	}
	Ok(affected)
}

/// Removes the snapshots called `names`, whichever of them still exist.
pub(crate) fn remove_snapshots(
	database: &Path,
	names: &[String],
) -> Result<(), StoreError> {
	let directory = snapshot::snapshots_dir(database);
	for name in names {
		snapshot::remove_if_present(&directory.join(name))?;
	}
	Ok(())
}

/// How many ledger records the copy at `path` had applied. A copy that
/// cannot say, because it predates the column or cannot be opened, is
/// taken to have applied none, so any deletion at all affects it.
async fn deletions_applied(path: &Path) -> u64 {
	let options = SqliteConnectOptions::new()
		.filename(path)
		.read_only(true)
		.create_if_missing(false);
	let Ok(mut connection) = SqliteConnection::connect_with(&options).await
	else {
		return 0;
	};
	let applied = plane::deletions_applied(&mut connection).await.unwrap_or(0);
	let _ = connection.close().await;
	applied
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{NewPairedClient, PairingKeyAlgorithm, SnapshotReason, Store};
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;

	fn names(database: &Path) -> Vec<String> {
		snapshot::list(database)
			.unwrap()
			.into_iter()
			.map(|snapshot| snapshot.name)
			.collect()
	}

	/// A snapshot taken before a deletion is affected by it, whatever its
	/// reason and however it is stamped; one taken after is not, and a
	/// Plane with no deletions has nothing affected at all.
	#[tokio::test]
	async fn the_snapshots_a_deletion_may_survive_in_predate_it() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let client_id = Uuid::now_v7();
		store
			.write(async |tx| {
				tx.upsert_paired_client(NewPairedClient {
					client_id,
					key_algorithm: PairingKeyAlgorithm::Ed25519,
					public_key: [7; 32],
					pairing_protocol: "jet-pairing-1".into(),
					paired_at_unix_ms: NOW_UNIX_MS,
				})
				.await
				.map(|_| ())
			})
			.await
			.unwrap();
		let before = store
			.snapshot(SnapshotReason::Daily, NOW_UNIX_MS)
			.await
			.unwrap();
		let untouched = affected_snapshots(&path, 0).await.unwrap();
		store
			.write(async |tx| {
				tx.delete_paired_client(client_id, NOW_UNIX_MS + 1).await
			})
			.await
			.unwrap();
		// Stamped before the deletion, taken after it.
		let after = store
			.snapshot(
				SnapshotReason::Migration { applied_version: 1 },
				NOW_UNIX_MS - 1,
			)
			.await
			.unwrap();

		let affected = affected_snapshots(&path, 1).await.unwrap();
		remove_snapshots(&path, &affected).unwrap();

		assert_eq!(
			(untouched, affected, names(&path)),
			(vec![], vec![before.name], vec![after.name])
		);
	}
}
