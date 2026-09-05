use std::path::Path;

use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::{
	ActorRecord, AuditBreach, AuditHead, AuditIntegrity, AuditIntegrityFailure,
	AuditOutcome, AuditRecord, AuditRisk, NewAuditRecord, Store, StoreError,
	audit_head_path,
};

const NOW_UNIX_MS: i64 = 1_700_000_000_000;

fn decision() -> NewAuditRecord {
	NewAuditRecord {
		record_id: Uuid::now_v7(),
		recorded_at_unix_ms: NOW_UNIX_MS,
		actor: ActorRecord::InteractiveClient {
			client_id: Uuid::nil(),
		},
		target_kind: "account_binding".into(),
		target_id: Some(Uuid::now_v7().to_string()),
		decision: "account.bound".into(),
		risk: AuditRisk::Elevated,
		outcome: AuditOutcome::Succeeded,
	}
}

async fn append(store: &Store) -> AuditRecord {
	store
		.write(async |tx| tx.append_audit_record(decision()).await)
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

	let page = store.read(async |tx| tx.audit_page(0, 16).await).await;
	assert_eq!(page.unwrap(), (1, vec![appended]));
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
