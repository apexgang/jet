use std::path::Path;

use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::{
	ActorRecord, AuditBreach, AuditHead, AuditIntegrity, AuditIntegrityFailure,
	AuditOutcome, AuditRecord, AuditRisk, NewAuditRecord, Store, StoreError,
	audit_head_path,
};

const NOW_UNIX_MS: i64 = 1_700_000_000_000;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

fn decision() -> NewAuditRecord {
	decision_about(&Uuid::now_v7().to_string(), NOW_UNIX_MS)
}

fn decision_about(target_id: &str, recorded_at_unix_ms: i64) -> NewAuditRecord {
	NewAuditRecord {
		record_id: Uuid::now_v7(),
		recorded_at_unix_ms,
		actor: ActorRecord::InteractiveClient {
			client_id: Uuid::nil(),
		},
		target_kind: "account_binding".into(),
		target_id: Some(target_id.into()),
		decision: "account.bound".into(),
		risk: AuditRisk::Elevated,
		outcome: AuditOutcome::Succeeded,
	}
}

async fn append(store: &Store) -> AuditRecord {
	append_record(store, decision()).await
}

async fn append_record(store: &Store, record: NewAuditRecord) -> AuditRecord {
	store
		.write(async |tx| tx.append_audit_record(record).await)
		.await
		.unwrap()
}

async fn page(store: &Store) -> (u64, Vec<AuditRecord>) {
	store
		.read(async |tx| tx.audit_page(0, 16).await)
		.await
		.unwrap()
}

fn head_of(record: &AuditRecord) -> AuditHead {
	AuditHead {
		epoch: record.epoch,
		sequence: record.sequence,
		entry_hash: record.entry_hash,
	}
}

/// Copies a closed store the way a Recovery snapshot does, taking the
/// write-ahead log with it when one is still there.
fn copy_store(from: &Path, to: &Path) {
	std::fs::copy(from, to).unwrap();
	for suffix in ["-wal", "-shm"] {
		let mut source = from.as_os_str().to_owned();
		source.push(suffix);
		let mut target = to.as_os_str().to_owned();
		target.push(suffix);
		let (source, target) = (Path::new(&source), Path::new(&target));
		if source.exists() {
			std::fs::copy(source, target).unwrap();
		} else if target.exists() {
			std::fs::remove_file(target).unwrap();
		}
	}
}

#[tokio::test]
async fn a_recorded_decision_is_read_back_whole() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();

	let appended = append(&store).await;

	assert_eq!(page(&store).await, (1, vec![appended]));
}

#[tokio::test]
async fn an_audit_nothing_has_been_recorded_in_validates() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();

	assert_eq!(
		store.validate_audit().await.unwrap(),
		AuditIntegrity::Verified { head: None }
	);
}

#[tokio::test]
async fn the_head_beside_the_store_names_the_newest_decision() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let store = Store::open(&path).await.unwrap();

	append(&store).await;
	let newest = append(&store).await;

	let plane_id = store.plane().await.unwrap().plane_id;
	let sequence = newest.sequence;
	let entry_hash = newest.entry_hash;
	assert_eq!(
		(
			store.validate_audit().await.unwrap(),
			std::fs::read_to_string(audit_head_path(&path)).unwrap()
		),
		(
			AuditIntegrity::Verified {
				head: Some(head_of(&newest))
			},
			format!(
				"jet-security-audit-head 1\nplane {plane_id}\nepoch 1\n\
				 sequence {sequence}\nhash {entry_hash}\n"
			)
		)
	);
}

#[tokio::test]
async fn a_decision_that_rolls_back_publishes_no_head() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let store = Store::open(&path).await.unwrap();

	let refused = store
		.write(async |tx| {
			tx.append_audit_record(decision()).await?;
			Err::<(), StoreError>(StoreError::Integrity(
				"the Command failed after recording its decision".into(),
			))
		})
		.await
		.unwrap_err();

	assert_eq!(
		(
			refused.to_string(),
			audit_head_path(&path).exists(),
			store.validate_audit().await.unwrap()
		),
		(
			"store integrity failure: the Command failed after recording \
			 its decision"
				.into(),
			false,
			AuditIntegrity::Verified { head: None }
		)
	);
}

#[tokio::test]
async fn a_store_restored_behind_its_head_fails_validation() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let snapshot = dir.path().join("snapshot.sqlite3");

	let store = Store::open(&path).await.unwrap();
	append(&store).await;
	store.close().await;
	copy_store(&path, &snapshot);

	let store = Store::open(&path).await.unwrap();
	let lost = append(&store).await;
	store.close().await;

	// The database goes back to the snapshot. The head, which lives outside
	// it, does not.
	copy_store(&snapshot, &path);
	let store = Store::open(&path).await.unwrap();

	assert_eq!(
		store.validate_audit().await.unwrap(),
		AuditIntegrity::Failed(AuditIntegrityFailure {
			breach: AuditBreach::HeadNotInStore,
			epoch: 1,
			head: Some(head_of(&lost)),
			store_sequence: 1,
		})
	);
}

#[tokio::test]
async fn a_head_lost_after_its_commit_is_repaired_at_the_next_start() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let store = Store::open(&path).await.unwrap();

	append(&store).await;
	let stale = std::fs::read(audit_head_path(&path)).unwrap();
	let newest = append(&store).await;
	// The head write that the crash after the commit swallowed.
	std::fs::write(audit_head_path(&path), &stale).unwrap();

	let repaired = store.validate_audit().await.unwrap();

	assert_eq!(
		(
			repaired,
			std::fs::read(audit_head_path(&path)).unwrap() == stale
		),
		(
			AuditIntegrity::Verified {
				head: Some(head_of(&newest))
			},
			false
		)
	);
}

#[tokio::test]
async fn an_audit_whose_head_is_gone_fails_validation() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let store = Store::open(&path).await.unwrap();

	append(&store).await;
	std::fs::remove_file(audit_head_path(&path)).unwrap();

	assert_eq!(
		store.validate_audit().await.unwrap(),
		AuditIntegrity::Failed(AuditIntegrityFailure {
			breach: AuditBreach::HeadMissing,
			epoch: 1,
			head: None,
			store_sequence: 1,
		})
	);
}

#[tokio::test]
async fn a_decision_edited_after_the_fact_no_longer_folds_to_its_link() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let store = Store::open(&path).await.unwrap();

	let edited = append(&store).await;
	let newest = append(&store).await;
	store
		.write(async |tx| {
			sqlx::query!(
				"UPDATE security_audit SET outcome = 'denied'
				 WHERE sequence = ?1",
				1
			)
			.execute(tx.connection())
			.await?;
			Ok::<_, StoreError>(())
		})
		.await
		.unwrap();

	assert_eq!(
		store.validate_audit().await.unwrap(),
		AuditIntegrity::Failed(AuditIntegrityFailure {
			breach: AuditBreach::RecordAltered {
				sequence: edited.sequence
			},
			epoch: 1,
			head: Some(head_of(&newest)),
			store_sequence: 2,
		})
	);
}

#[tokio::test]
async fn a_target_swapped_under_its_reference_fails_validation() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let store = Store::open(&path).await.unwrap();

	let swapped = append(&store).await;
	let elsewhere = Uuid::now_v7().to_string();
	store
		.write(async |tx| {
			sqlx::query!(
				"UPDATE security_audit SET target_id = ?1 WHERE sequence = ?2",
				elsewhere,
				1
			)
			.execute(tx.connection())
			.await?;
			Ok::<_, StoreError>(())
		})
		.await
		.unwrap();

	assert_eq!(
		store.validate_audit().await.unwrap(),
		AuditIntegrity::Failed(AuditIntegrityFailure {
			breach: AuditBreach::TargetAltered {
				sequence: swapped.sequence
			},
			epoch: 1,
			head: Some(head_of(&swapped)),
			store_sequence: 1,
		})
	);
}

#[tokio::test]
async fn retention_removes_expired_records_and_leaves_a_whole_chain() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let store = Store::open(&path).await.unwrap();

	append_record(&store, decision_about("first", NOW_UNIX_MS - 9 * DAY_MS))
		.await;
	append_record(&store, decision_about("second", NOW_UNIX_MS - 8 * DAY_MS))
		.await;
	let kept =
		append_record(&store, decision_about("third", NOW_UNIX_MS)).await;

	let removed = store
		.prune_audit_before(NOW_UNIX_MS - DAY_MS)
		.await
		.unwrap();

	assert_eq!(
		(
			removed,
			page(&store).await,
			store.validate_audit().await.unwrap()
		),
		(
			2,
			(kept.sequence, vec![kept.clone()]),
			AuditIntegrity::Verified {
				head: Some(head_of(&kept))
			}
		)
	);
}

#[tokio::test]
async fn retention_keeps_the_record_the_head_names() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let store = Store::open(&path).await.unwrap();

	append_record(&store, decision_about("first", NOW_UNIX_MS - 9 * DAY_MS))
		.await;
	let newest = append_record(
		&store,
		decision_about("second", NOW_UNIX_MS - 8 * DAY_MS),
	)
	.await;

	// Every record has expired, and the one the head names still cannot go.
	let removed = store.prune_audit_before(NOW_UNIX_MS).await.unwrap();

	assert_eq!(
		(
			removed,
			page(&store).await,
			store.validate_audit().await.unwrap()
		),
		(
			1,
			(newest.sequence, vec![newest.clone()]),
			AuditIntegrity::Verified {
				head: Some(head_of(&newest))
			}
		)
	);
}

#[tokio::test]
async fn retention_stops_at_the_first_record_it_may_not_remove() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let store = Store::open(&path).await.unwrap();

	append_record(&store, decision_about("expired", NOW_UNIX_MS - 9 * DAY_MS))
		.await;
	// A clock that moved backwards leaves a fresh record among expired
	// ones. Removing around it would leave a hole the chain cannot be
	// folded across.
	let recent =
		append_record(&store, decision_about("recent", NOW_UNIX_MS)).await;
	let behind = append_record(
		&store,
		decision_about("behind", NOW_UNIX_MS - 9 * DAY_MS),
	)
	.await;

	let removed = store
		.prune_audit_before(NOW_UNIX_MS - DAY_MS)
		.await
		.unwrap();

	assert_eq!(
		(
			removed,
			page(&store).await,
			store.validate_audit().await.unwrap()
		),
		(
			1,
			(behind.sequence, vec![recent, behind.clone()]),
			AuditIntegrity::Verified {
				head: Some(head_of(&behind))
			}
		)
	);
}

#[tokio::test]
async fn anonymizing_a_target_forgets_its_name_and_keeps_the_chain() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let store = Store::open(&path).await.unwrap();
	let deleted = Uuid::now_v7().to_string();

	let first =
		append_record(&store, decision_about(&deleted, NOW_UNIX_MS)).await;
	let other = append_record(&store, decision()).await;
	let second =
		append_record(&store, decision_about(&deleted, NOW_UNIX_MS)).await;

	let anonymized = store
		.write(async |tx| {
			tx.anonymize_audit_target("account_binding", &deleted).await
		})
		.await
		.unwrap();

	let expected = vec![
		AuditRecord {
			target_id: None,
			..first.clone()
		},
		other,
		AuditRecord {
			target_id: None,
			..second.clone()
		},
	];
	assert_eq!(
		(
			anonymized,
			page(&store).await,
			first.target_reference == second.target_reference,
			store.validate_audit().await.unwrap()
		),
		(
			2,
			(second.sequence, expected),
			true,
			AuditIntegrity::Verified {
				head: Some(head_of(&second))
			}
		)
	);
}
