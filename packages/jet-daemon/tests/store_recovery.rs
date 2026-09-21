//! Black-box conformance tests for store Recovery at the public Jet
//! protocol boundary: a real `jetd`, a real store, a real snapshot, real
//! damage, and the Rust client (ADR-0077, ADR-0097).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_client::ClientError;
use jet_protocol::{
	DeletionLedgerStatus, ErrorCategory, RecoveryReason, RecoverySnapshot,
	RecoveryState, RecoveryStatus, RetentionPolicy, SecurityState,
	SnapshotReason,
};
use pretty_assertions::assert_eq;
use std::{
	io::{Seek as _, SeekFrom, Write as _},
	path::Path,
	time::Duration,
};
use support::{Daemon, start_jetd, wait_for_snapshot};
use uuid::Uuid;

fn send_sigterm(daemon: &Daemon) {
	let pid = rustix::process::Pid::from_raw(
		i32::try_from(daemon.child.id().unwrap()).unwrap(),
	)
	.unwrap();
	rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
}

/// Overwrites the second page of the closed database.
fn damage(home: &Path) {
	let mut file = std::fs::OpenOptions::new()
		.write(true)
		.open(home.join("plane.sqlite3"))
		.unwrap();
	file.seek(SeekFrom::Start(4096)).unwrap();
	file.write_all(&[0xff; 4096]).unwrap();
	file.sync_all().unwrap();
}

#[tokio::test]
async fn a_damaged_store_is_served_read_only_until_a_snapshot_is_restored() {
	tokio::time::timeout(Duration::from_secs(60), async {
		let dir = tempfile::tempdir().unwrap();
		let home = dir.path().join(".jet");
		let client_id = Uuid::new_v4();

		let daemon = start_jetd(&home).await;
		assert_eq!(daemon.ready["recovery"], "serving");
		let client = support::connect(&daemon, client_id).await;
		let kept = client
			.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
			.await
			.unwrap();
		// Startup work can earn the daily snapshot before this Conversation.
		// Restart with no snapshots so the restart's maintenance takes a
		// snapshot containing the state this test intends to restore. It is
		// taken behind the ready line (ADR-0022), so it is waited for.
		drop(client);
		let mut daemon = daemon;
		daemon.child.kill().await.unwrap();
		if home.join("snapshots").exists() {
			std::fs::remove_dir_all(home.join("snapshots")).unwrap();
		}
		let daemon = start_jetd(&home).await;
		let snapshot =
			wait_for_snapshot(&home, SnapshotReason::Maintenance).await;
		let client = support::connect(&daemon, client_id).await;
		assert_eq!(
			client
				.status()
				.await
				.unwrap()
				.recovery
				.unwrap()
				.snapshots
				.iter()
				.map(|snapshot| snapshot.name.clone())
				.collect::<Vec<_>>(),
			vec![snapshot.clone()]
		);
		// Anything after the snapshot is what a restoration gives up.
		client
			.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
			.await
			.unwrap();
		send_sigterm(&daemon);
		let mut daemon = daemon;
		assert!(daemon.child.wait().await.unwrap().success());
		damage(&home);

		let daemon = start_jetd(&home).await;
		assert_eq!(daemon.ready["recovery"], "read_only");
		let client = support::connect(&daemon, client_id).await;
		let status = client.status().await.unwrap();
		let recovery = status.recovery.clone().unwrap();
		let taken = recovery.snapshots[0].clone();
		assert_eq!(
			recovery,
			RecoveryStatus {
				state: RecoveryState::ReadOnly,
				reason: Some(RecoveryReason::IntegrityCheckFailed),
				snapshots: vec![RecoverySnapshot {
					name: snapshot.clone(),
					taken_at_unix_ms: taken.taken_at_unix_ms,
					reason: SnapshotReason::Maintenance,
					bytes: taken.bytes,
				}],
				deletion_ledger: Some(DeletionLedgerStatus::Verified {
					deletions: 0,
				}),
			}
		);
		assert!(taken.bytes > 0);
		// Reads still answer; every Command but the restoration is refused
		// without a receipt, so the same identity succeeds afterwards.
		let command_id = Uuid::now_v7();
		let refused = client
			.create_conversation(command_id, RetentionPolicy::Retain)
			.await
			.unwrap_err();
		let ClientError::Remote(error) = refused else {
			panic!("{refused:?}");
		};
		assert_eq!(
			(error.category, error.code.as_str(), error.retryable),
			(ErrorCategory::Unavailable, "recovery.read_only", true)
		);

		let restored = client
			.restore_recovery_snapshot(Uuid::now_v7(), snapshot.clone())
			.await
			.unwrap();
		assert_eq!(restored.snapshot, snapshot);
		assert!(restored.replaced.starts_with("plane.sqlite3.damaged-"));
		assert!(home.join(&restored.replaced).exists());
		let status = client.status().await.unwrap();
		assert_eq!(
			(
				status.recovery.clone(),
				status.security.clone(),
				status.daemon_starts
			),
			(
				Some(RecoveryStatus {
					state: RecoveryState::Serving,
					reason: None,
					snapshots: vec![taken],
					deletion_ledger: Some(DeletionLedgerStatus::Verified {
						deletions: 0,
					}),
				}),
				Some(SecurityState::Trusted),
				3
			)
		);
		let conversations = client.conversations().await.unwrap();
		assert_eq!(
			conversations
				.conversations
				.iter()
				.map(|c| c.conversation_id)
				.collect::<Vec<_>>(),
			vec![kept.conversation_id]
		);
		client
			.create_conversation(command_id, RetentionPolicy::Retain)
			.await
			.unwrap();
		let again = client
			.restore_recovery_snapshot(Uuid::now_v7(), snapshot)
			.await
			.unwrap_err();
		let ClientError::Remote(error) = again else {
			panic!("{again:?}");
		};
		assert_eq!(error.code, "recovery.not_read_only");
		drop(client);
		send_sigterm(&daemon);
		let mut daemon = daemon;
		assert!(daemon.child.wait().await.unwrap().success());

		// The restored store opens as any other.
		let daemon = start_jetd(&home).await;
		assert_eq!(daemon.ready["recovery"], "serving");
		let status = support::connect(&daemon, client_id)
			.await
			.status()
			.await
			.unwrap();
		assert_eq!(
			(status.daemon_starts, status.security),
			(4, Some(SecurityState::Trusted))
		);
	})
	.await
	.unwrap();
}

/// Appends Events until the journal is most of the file, so the page in
/// its middle is the journal's wherever the schema's pages fall.
async fn fill_journal(home: &Path) {
	let store = jet_store::Store::open(&home.join("plane.sqlite3"))
		.await
		.unwrap();
	store
		.write(async |tx| {
			for _ in 0..256 {
				tx.append_event(jet_store::NewEvent {
					event_id: Uuid::now_v7(),
					actor: jet_store::ActorRecord::InteractiveClient {
						client_id: Uuid::nil(),
					},
					recorded_at_unix_ms: 0,
					conversation_id: None,
					run_id: None,
					kind: "run.progress".into(),
					payload_version: 1,
					payload: format!("{{\"p\":\"{}\"}}", "x".repeat(16 * 1024)),
					class: jet_store::EventClass::Operational,
				})
				.await?;
			}
			Ok::<(), jet_store::StoreError>(())
		})
		.await
		.unwrap();
	store.close().await;
}

/// Overwrites the page in the middle of the closed database, which the
/// journal fills once [`fill_journal`] has run: a page nothing reads on
/// the way to serving the store.
fn damage_journal_page(home: &Path) {
	let path = home.join("plane.sqlite3");
	let length = std::fs::metadata(&path).unwrap().len();
	let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
	file.seek(SeekFrom::Start(length / 2 / 4096 * 4096))
		.unwrap();
	file.write_all(&[0xff; 4096]).unwrap();
	file.sync_all().unwrap();
}

async fn stop_cleanly(mut daemon: Daemon) {
	use tokio::io::AsyncReadExt as _;
	send_sigterm(&daemon);
	let status = daemon.child.wait().await.unwrap();
	let mut stderr = String::new();
	if let Some(mut pipe) = daemon.child.stderr.take() {
		pipe.read_to_string(&mut stderr).await.unwrap();
	}
	assert!(status.success(), "jetd exited with {status}: {stderr}");
}

#[tokio::test]
async fn damage_the_open_did_not_meet_is_found_while_idle() {
	tokio::time::timeout(Duration::from_secs(60), async {
		let dir = tempfile::tempdir().unwrap();
		let home = dir.path().join(".jet");
		let client_id = Uuid::new_v4();

		let daemon = start_jetd(&home).await;
		let client = support::connect(&daemon, client_id).await;
		client
			.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
			.await
			.unwrap();
		let snapshot = wait_for_snapshot(&home, SnapshotReason::Daily).await;
		drop(client);
		stop_cleanly(daemon).await;
		fill_journal(&home).await;
		// A start indexes what was appended, so the next one has no
		// reason to read those pages before the idle check does.
		stop_cleanly(start_jetd(&home).await).await;
		damage_journal_page(&home);
		let damaged_bytes = std::fs::read(home.join("plane.sqlite3")).unwrap();

		// A clean close owes no page check at open, so the store serves;
		// the idle check then finds what the open did not.
		let daemon = start_jetd(&home).await;
		assert_eq!(daemon.ready["recovery"], "serving");
		let client = support::connect(&daemon, client_id).await;
		let recovery = loop {
			let recovery = client.status().await.unwrap().recovery.unwrap();
			if recovery.state == RecoveryState::ReadOnly {
				break recovery;
			}
			tokio::time::sleep(Duration::from_millis(20)).await;
		};
		assert_eq!(
			(recovery.reason, recovery.snapshots.len()),
			(Some(RecoveryReason::IntegrityCheckFailed), 1)
		);
		let refused = client
			.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
			.await
			.unwrap_err();
		let ClientError::Remote(error) = refused else {
			panic!("{refused:?}");
		};
		assert_eq!(
			(error.category, error.code.as_str(), error.retryable),
			(ErrorCategory::Unavailable, "recovery.read_only", true)
		);
		// Preserved as found: nothing moved it aside or wrote to it.
		assert_eq!(
			std::fs::read(home.join("plane.sqlite3")).unwrap(),
			damaged_bytes
		);
		assert!(!std::fs::read_dir(&home).unwrap().any(|entry| {
			entry
				.unwrap()
				.file_name()
				.to_string_lossy()
				.contains(".damaged-")
		}));

		// The way out is the same, and the daemon serves again after it.
		let restored = client
			.restore_recovery_snapshot(Uuid::now_v7(), snapshot.clone())
			.await
			.unwrap();
		assert!(restored.replaced.starts_with("plane.sqlite3.damaged-"));
		let status = client.status().await.unwrap();
		assert_eq!(
			status.recovery.map(|recovery| recovery.state),
			Some(RecoveryState::Serving)
		);
		client
			.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
			.await
			.unwrap();
		drop(client);
		stop_cleanly(daemon).await;
	})
	.await
	.unwrap();
}
