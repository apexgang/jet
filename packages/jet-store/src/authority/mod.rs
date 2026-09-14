//! Which Plane owns a Conversation: its authority, the epoch that
//! authority is in, the Plane transfers that move it, and the Authority
//! fences that retire it (ADR-0062, ADR-0070).
//!
//! A fence reaches its ledger before the store commits the relinquishment
//! it describes, like a deletion reaches the Deletion ledger: a crash in
//! between leaves a fence for an authority the store still claims, and
//! the next open reapplies the fences and retires it. A restored snapshot
//! from before the transfer meets the same fences at open, so it cannot
//! continue the authority it still remembers holding.

mod fence;
mod transfer;

pub use fence::{AuthorityFenceRecord, AuthorityFences, PendingFence};
pub(crate) use fence::{append, read};
pub use transfer::{
	NewPlaneTransfer, PlaneTransferPhase, PlaneTransferRecord,
	PlaneTransferRole,
};

use crate::{StoreError, plane, records::column_error};
use sqlx::SqlitePool;

/// Who owns a Conversation, as this Plane sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityState {
	/// This Plane is its Home Plane.
	Home,
	/// A validated Prepared transfer: this Plane holds a copy that cannot
	/// run work or fire schedules until its source relinquishes.
	Prepared,
	/// This Plane retired its authority through a fence; what remains is
	/// the Transfer tombstone.
	Relinquished,
}

impl AuthorityState {
	/// The durable spelling.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Home => "home",
			Self::Prepared => "prepared",
			Self::Relinquished => "relinquished",
		}
	}

	pub(crate) fn parse(text: &str) -> Result<Self, StoreError> {
		[Self::Home, Self::Prepared, Self::Relinquished]
			.into_iter()
			.find(|state| state.as_str() == text)
			.ok_or_else(|| {
				column_error("authority", format!("unknown authority {text:?}"))
			})
	}
}

/// A Conversation's authority and the epoch it is in. The epoch counts
/// the authorities the Conversation has had across Planes; a transfer
/// retires one and opens the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorityRecord {
	/// Who owns the Conversation here.
	pub state: AuthorityState,
	/// The epoch this Plane's copy belongs to, from one.
	pub epoch: u64,
}

impl AuthorityRecord {
	/// The authority every new Conversation starts with: this Plane's, in
	/// its first epoch.
	pub const HOME: Self = Self {
		state: AuthorityState::Home,
		epoch: 1,
	};

	pub(crate) fn parse(state: &str, epoch: i64) -> Result<Self, StoreError> {
		Ok(Self {
			state: AuthorityState::parse(state)?,
			epoch: u64::try_from(epoch).map_err(|_| {
				column_error(
					"authority_epoch",
					format!("authority epoch {epoch} is not positive"),
				)
			})?,
		})
	}
}

/// Retires in the store every authority the fences say is gone and
/// records the fences as applied, in one transaction, and returns how
/// many rows that changed: zero when the store already agreed. A
/// Conversation still claiming a fenced epoch as its Home Plane is what a
/// restored snapshot, or a crash between the fence and its commit,
/// leaves behind.
pub(crate) async fn reapply(
	pool: &SqlitePool,
	records: &[AuthorityFenceRecord],
) -> Result<u64, StoreError> {
	let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
	let mut changed = 0;
	for record in records {
		let conversation_id = record.conversation_id.to_string();
		let epoch = i64::try_from(record.retired_epoch).unwrap_or(i64::MAX);
		changed += sqlx::query!(
			"UPDATE conversations SET authority = 'relinquished',
				revision = revision + 1
			 WHERE conversation_id = ?1 AND authority_epoch = ?2
				AND authority = 'home'",
			conversation_id,
			epoch
		)
		.execute(&mut *transaction)
		.await?
		.rows_affected();
	}
	let vouched = u64::try_from(records.len()).unwrap_or(u64::MAX);
	if plane::fences_applied(&mut *transaction).await? < vouched {
		plane::record_fences_applied(&mut *transaction, vouched).await?;
		changed += 1;
	}
	transaction.commit().await?;
	Ok(changed)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		ConversationOriginRecord, NewConversation, RetentionPolicy,
		SnapshotReason, Store, StoreError, TrashReasonRecord, TrashRecord,
		WorkingTreeRecord,
	};
	use pretty_assertions::assert_eq;
	use std::{
		fs,
		io::{Seek as _, SeekFrom, Write as _},
		path::Path,
	};
	use uuid::Uuid;

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;
	const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

	fn damage(path: &Path) {
		let mut file = fs::OpenOptions::new().write(true).open(path).unwrap();
		file.seek(SeekFrom::Start(4096)).unwrap();
		file.write_all(&[0xff; 4096]).unwrap();
		file.sync_all().unwrap();
	}

	async fn converse(store: &Store, conversation_id: Uuid) {
		store
			.write(async |tx| {
				tx.insert_conversation(NewConversation {
					conversation_id,
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRecord::NoProject,
					origin: ConversationOriginRecord::New,
					created_at_unix_ms: NOW_UNIX_MS,
				})
				.await
				.map(|_| ())
			})
			.await
			.unwrap();
	}

	async fn prepare(
		store: &Store,
		conversation_id: Uuid,
		target: Uuid,
	) -> PlaneTransferRecord {
		store
			.write(async |tx| {
				tx.insert_plane_transfer(NewPlaneTransfer {
					transfer_id: Uuid::now_v7(),
					conversation_id,
					role: PlaneTransferRole::Source,
					retired_epoch: 1,
					peer_plane_id: target,
					bundle_sha256: "ab".repeat(32),
					prepared_at_unix_ms: NOW_UNIX_MS,
				})
				.await
			})
			.await
			.unwrap()
	}

	async fn authority(
		store: &Store,
		conversation_id: Uuid,
	) -> AuthorityRecord {
		store
			.read(async |tx| {
				Ok::<_, StoreError>(
					tx.conversation(conversation_id).await?.unwrap().authority,
				)
			})
			.await
			.unwrap()
	}

	/// The fence is in its ledger before the store commits the
	/// relinquishment, so a snapshot from before it cannot bring the
	/// authority back: restoring that snapshot reapplies the fences, and
	/// the Transfer tombstone is what the live store keeps (ADR-0070).
	#[tokio::test]
	async fn restoring_a_snapshot_reapplies_the_fences_raised_after_it() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let plane_id = store.plane().await.unwrap().plane_id;
		let conversation_id = Uuid::now_v7();
		let target = Uuid::now_v7();
		converse(&store, conversation_id).await;
		let transfer = prepare(&store, conversation_id, target).await;
		let snapshot = store
			.snapshot(SnapshotReason::Daily, NOW_UNIX_MS)
			.await
			.unwrap();
		store
			.write(async |tx| {
				tx.relinquish_plane_transfer(
					&transfer,
					NOW_UNIX_MS + 1,
					NOW_UNIX_MS + 1 + 30 * DAY_MS,
				)
				.await
			})
			.await
			.unwrap();
		let live = (
			authority(&store, conversation_id).await,
			store
				.read(async |tx| tx.trash_entry(conversation_id).await)
				.await
				.unwrap(),
			store
				.read(async |tx| tx.plane_transfer(transfer.transfer_id).await)
				.await
				.unwrap()
				.map(|record| (record.phase, record.settled_at_unix_ms)),
		);
		let fences = store.authority_fences().unwrap();
		let header = fs::read_to_string(
			crate::evidence::recovery_dir(&path).join("plane.sqlite3.fences"),
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
				live,
				fences,
				header,
				authority(&store, conversation_id).await
			),
			(
				(
					AuthorityRecord {
						state: AuthorityState::Relinquished,
						epoch: 1,
					},
					Some(TrashRecord {
						conversation_id,
						reason: TrashReasonRecord::Transferred,
						trashed_at_unix_ms: NOW_UNIX_MS + 1,
						expires_at_unix_ms: NOW_UNIX_MS + 1 + 30 * DAY_MS,
					}),
					Some((
						PlaneTransferPhase::Relinquished,
						Some(NOW_UNIX_MS + 1)
					)),
				),
				AuthorityFences::Verified(vec![AuthorityFenceRecord {
					sequence: 1,
					fenced_at_unix_ms: NOW_UNIX_MS + 1,
					conversation_id,
					retired_epoch: 1,
					transfer_id: transfer.transfer_id,
					target_plane_id: target,
				}]),
				vec![
					"jet-authority-fences 1".to_string(),
					format!("plane {plane_id}")
				],
				AuthorityRecord {
					state: AuthorityState::Relinquished,
					epoch: 1,
				},
			)
		);
	}

	/// A fence the ledger holds and the store does not is the trace of a
	/// crash between the two writes; the next open finishes it. A later
	/// epoch of the same Conversation, transferred back here, is not the
	/// one the fence names and keeps its authority.
	#[tokio::test]
	async fn a_fence_the_store_missed_is_finished_at_the_next_open() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let plane_id = store.plane().await.unwrap().plane_id;
		let fenced = Uuid::now_v7();
		let returned = Uuid::now_v7();
		converse(&store, fenced).await;
		converse(&store, returned).await;
		store
			.write(async |tx| {
				tx.set_conversation_authority(
					returned,
					AuthorityRecord {
						state: AuthorityState::Home,
						epoch: 3,
					},
				)
				.await
			})
			.await
			.unwrap();
		append(
			&path,
			plane_id,
			0,
			&[
				PendingFence {
					fenced_at_unix_ms: NOW_UNIX_MS,
					conversation_id: fenced,
					retired_epoch: 1,
					transfer_id: Uuid::now_v7(),
					target_plane_id: Uuid::now_v7(),
				},
				PendingFence {
					fenced_at_unix_ms: NOW_UNIX_MS,
					conversation_id: returned,
					retired_epoch: 1,
					transfer_id: Uuid::now_v7(),
					target_plane_id: Uuid::now_v7(),
				},
			],
		)
		.unwrap();
		let before = authority(&store, fenced).await;
		store.close().await;

		let store = Store::open(&path).await.unwrap();

		assert_eq!(
			(
				before,
				authority(&store, fenced).await,
				authority(&store, returned).await
			),
			(
				AuthorityRecord::HOME,
				AuthorityRecord {
					state: AuthorityState::Relinquished,
					epoch: 1,
				},
				AuthorityRecord {
					state: AuthorityState::Home,
					epoch: 3,
				},
			)
		);
	}

	/// Fences that cannot be trusted refuse a restoration, because the
	/// snapshot may still claim an authority nothing else remembers
	/// retiring, and refuse a new relinquishment (ADR-0070).
	#[tokio::test]
	async fn corrupt_fences_refuse_restoration_and_new_relinquishments() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let conversation_id = Uuid::now_v7();
		converse(&store, conversation_id).await;
		let snapshot = store
			.snapshot(SnapshotReason::Daily, NOW_UNIX_MS)
			.await
			.unwrap();
		let transfer = prepare(&store, conversation_id, Uuid::now_v7()).await;
		store
			.write(async |tx| {
				tx.relinquish_plane_transfer(
					&transfer,
					NOW_UNIX_MS + 1,
					NOW_UNIX_MS + 2,
				)
				.await
			})
			.await
			.unwrap();
		fs::remove_file(
			crate::evidence::recovery_dir(&path).join("plane.sqlite3.fences"),
		)
		.unwrap();
		let again = prepare(&store, conversation_id, Uuid::now_v7()).await;
		let refused = store
			.write(async |tx| {
				tx.relinquish_plane_transfer(
					&again,
					NOW_UNIX_MS + 3,
					NOW_UNIX_MS + 4,
				)
				.await
			})
			.await
			.unwrap_err();
		store.close().await;
		damage(&path);

		let store = Store::open(&path).await.unwrap();
		let restore = store
			.restore(&snapshot.name, NOW_UNIX_MS + 5)
			.await
			.unwrap_err();

		assert_eq!(
			(
				refused.to_string(),
				restore.to_string(),
				store.authority_fences().unwrap(),
			),
			(
				"store integrity failure: the Authority fences cannot be \
				 trusted: its head names fence 1, but the ledger is missing"
					.to_string(),
				"store integrity failure: the Authority fences cannot be \
				 trusted: its head names fence 1, but the ledger is missing"
					.to_string(),
				AuthorityFences::Corrupt(
					"its head names fence 1, but the ledger is missing".into()
				),
			)
		);
	}
}
