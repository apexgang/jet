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
use support::{Daemon, start_jetd};
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

/// The first change of the day earns a snapshot as soon as the daemon
/// wakes for maintenance, so it is there within moments of the Command.
async fn wait_for_daily_snapshot(home: &Path) -> String {
	for _ in 0..500 {
		if let Ok(entries) = std::fs::read_dir(home.join("snapshots")) {
			let mut names: Vec<String> = entries
				.map(|entry| entry.unwrap().file_name().into_string().unwrap())
				.filter(|name| name.ends_with("-daily.sqlite3"))
				.collect();
			if let Some(name) = names.pop() {
				return name;
			}
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
	panic!("no daily snapshot was taken");
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
		let snapshot = wait_for_daily_snapshot(&home).await;
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
					reason: SnapshotReason::Daily,
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
				2
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
			(3, Some(SecurityState::Trusted))
		);
	})
	.await
	.unwrap();
}
