//! The Deletion ledger: a content-free, append-only record of every
//! permanent deletion, kept outside SQLite and its rollback-capable
//! snapshots so that restoring a snapshot cannot bring a deleted identity
//! back (ADR-0102).
//!
//! Each record names what kind of identity was deleted, which one, and
//! when, folded into an integrity chain whose durable head lives in its own
//! file. A deletion reaches the ledger before the store commits it: a crash
//! between the two leaves a record for a row the store still holds, and
//! the next open reapplies the ledger and removes it. The order rules out
//! the other gap, a deletion the store committed without the ledger, which
//! is exactly the deletion a restoration would undo.
//!
//! The ledger and its head are not secrets, and they are no defence against
//! code already running as the same operating-system user (ADR-0105). What
//! they make visible is a store that was put back to before a deletion.

mod files;
mod ledger;
mod purge;
mod reapply;

pub use ledger::{DeletedIdentityKind, DeletionLedger, DeletionRecord};
pub(crate) use ledger::{append, read};
pub(crate) use purge::{affected_snapshots, remove_snapshots};
pub(crate) use reapply::reapply;

use uuid::Uuid;

/// A deletion a write transaction has made and the ledger has not yet
/// recorded. The transaction collects them and the store writes them to
/// the ledger just before it commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingDeletion {
	pub(crate) kind: DeletedIdentityKind,
	pub(crate) identity: Uuid,
	pub(crate) deleted_at_unix_ms: i64,
}

#[cfg(test)]
mod tests {
	use super::{files::recovery_dir, *};
	use crate::{
		NewPairedClient, PairingKeyAlgorithm, SnapshotReason, Store,
		StoreError, StoreIntegrity,
	};
	use pretty_assertions::assert_eq;
	use std::{
		fs,
		io::{Seek as _, SeekFrom, Write as _},
		path::Path,
	};

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;

	fn damage(path: &Path) {
		let mut file = fs::OpenOptions::new().write(true).open(path).unwrap();
		file.seek(SeekFrom::Start(4096)).unwrap();
		file.write_all(&[0xff; 4096]).unwrap();
		file.sync_all().unwrap();
	}

	async fn pair(store: &Store, client_id: Uuid) {
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
	}

	async fn paired_clients(store: &Store) -> Vec<Uuid> {
		store
			.read(async |tx| {
				Ok::<_, StoreError>(
					tx.paired_clients()
						.await?
						.into_iter()
						.map(|client| client.client_id)
						.collect(),
				)
			})
			.await
			.unwrap()
	}

	/// A revocation is in the ledger before the store commits it, so a
	/// snapshot taken before it cannot bring the client back: restoring
	/// that snapshot reapplies the ledger (ADR-0102).
	#[tokio::test]
	async fn restoring_a_snapshot_reapplies_the_deletions_made_after_it() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let plane_id = store.plane().await.unwrap().plane_id;
		let revoked = Uuid::now_v7();
		let kept = Uuid::now_v7();
		pair(&store, revoked).await;
		pair(&store, kept).await;
		let snapshot = store
			.snapshot(SnapshotReason::Daily, NOW_UNIX_MS)
			.await
			.unwrap();
		store
			.write(async |tx| {
				tx.delete_paired_client(revoked, NOW_UNIX_MS + 1).await
			})
			.await
			.unwrap();
		let ledger = store.deletion_ledger().unwrap();
		let header = fs::read_to_string(
			recovery_dir(&path).join("plane.sqlite3.deletions"),
		)
		.unwrap()
		.lines()
		.take(2)
		.map(ToOwned::to_owned)
		.collect::<Vec<_>>();
		store.close().await;
		damage(&path);

		let store = Store::open(&path).await.unwrap();
		store
			.restore(&snapshot.name, NOW_UNIX_MS + 2)
			.await
			.unwrap();

		assert_eq!(
			(
				ledger,
				header,
				store.integrity(),
				paired_clients(&store).await
			),
			(
				DeletionLedger::Verified(vec![DeletionRecord {
					sequence: 1,
					deleted_at_unix_ms: NOW_UNIX_MS + 1,
					kind: DeletedIdentityKind::PairedClient,
					identity: revoked,
				}]),
				vec![
					"jet-deletion-ledger 1".to_string(),
					format!("plane {plane_id}")
				],
				StoreIntegrity::Verified,
				vec![kept]
			)
		);
	}

	/// A deletion the ledger holds and the store does not is the trace of
	/// a crash between the two writes; the next open finishes it.
	#[tokio::test]
	async fn a_deletion_the_store_missed_is_finished_at_the_next_open() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let plane_id = store.plane().await.unwrap().plane_id;
		let client_id = Uuid::now_v7();
		pair(&store, client_id).await;
		append(
			&path,
			plane_id,
			0,
			&[PendingDeletion {
				kind: DeletedIdentityKind::PairedClient,
				identity: client_id,
				deleted_at_unix_ms: NOW_UNIX_MS,
			}],
		)
		.unwrap();
		assert_eq!(paired_clients(&store).await, vec![client_id]);
		store.close().await;

		let store = Store::open(&path).await.unwrap();

		assert_eq!(paired_clients(&store).await, Vec::<Uuid>::new());
	}

	/// The store counts the deletions it has applied, so a ledger that has
	/// gone missing altogether, files and all, is still noticed: the count
	/// is ahead of a ledger that vouches for nothing (ADR-0102).
	#[tokio::test]
	async fn a_lost_ledger_is_noticed_by_the_store_that_applied_it() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let client_id = Uuid::now_v7();
		pair(&store, client_id).await;
		store
			.write(async |tx| {
				tx.delete_paired_client(client_id, NOW_UNIX_MS).await
			})
			.await
			.unwrap();
		let before = store.deletion_ledger().unwrap();
		fs::remove_dir_all(recovery_dir(&path)).unwrap();

		let refused = store
			.write(async |tx| {
				tx.delete_paired_client(Uuid::now_v7(), NOW_UNIX_MS + 1)
					.await
			})
			.await
			.unwrap_err();

		assert_eq!(
			(
				matches!(before, DeletionLedger::Verified(_)),
				store.deletion_ledger().unwrap(),
				refused.to_string()
			),
			(
				true,
				DeletionLedger::Corrupt(
					"the store has applied 1 deletions, but the ledger \
					 vouches for 0"
						.into()
				),
				"store integrity failure: the Deletion ledger cannot be \
				 trusted: the store has applied 1 deletions, but the ledger \
				 vouches for 0"
					.to_string()
			)
		);
	}

	/// A ledger that cannot be trusted refuses a restoration, because the
	/// snapshot may predate a deletion nothing else remembers, and refuses
	/// a new deletion, which could not be chained to it (ADR-0102).
	#[tokio::test]
	async fn a_corrupt_ledger_refuses_restoration_and_new_deletions() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let client_id = Uuid::now_v7();
		pair(&store, client_id).await;
		let snapshot = store
			.snapshot(SnapshotReason::Daily, NOW_UNIX_MS)
			.await
			.unwrap();
		store
			.write(async |tx| {
				tx.delete_paired_client(client_id, NOW_UNIX_MS + 1).await
			})
			.await
			.unwrap();
		fs::remove_file(recovery_dir(&path).join("plane.sqlite3.deletions"))
			.unwrap();
		let refused = store
			.write(async |tx| {
				tx.delete_paired_client(Uuid::now_v7(), NOW_UNIX_MS + 2)
					.await
			})
			.await
			.unwrap_err();
		store.close().await;
		damage(&path);

		let store = Store::open(&path).await.unwrap();
		let restore = store
			.restore(&snapshot.name, NOW_UNIX_MS + 3)
			.await
			.unwrap_err();

		assert_eq!(
			(
				refused.to_string(),
				restore.to_string(),
				store.deletion_ledger().unwrap(),
				store.integrity() == StoreIntegrity::Verified,
			),
			(
				"store integrity failure: the Deletion ledger cannot be trusted: its head names deletion 1, but the ledger is missing".to_string(),
				"store integrity failure: the Deletion ledger cannot be trusted: its head names deletion 1, but the ledger is missing".to_string(),
				DeletionLedger::Corrupt(
					"its head names deletion 1, but the ledger is missing".into()
				),
				false,
			)
		);
	}
}
