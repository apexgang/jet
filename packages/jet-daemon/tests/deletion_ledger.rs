//! Black-box conformance tests for the Deletion ledger at the public Jet
//! protocol boundary: a real `jetd`, a real store, a real deletion, and
//! the Rust client (ADR-0102).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_protocol::{
	CredentialSource, DeletionLedgerStatus, RecoveryState, SnapshotReason,
};
use pretty_assertions::assert_eq;
use std::{path::Path, time::Duration};
use support::start_jetd;
use uuid::Uuid;

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

/// An unbinding is in the ledger before it is acknowledged, the status
/// says so, and a purge takes a post-deletion snapshot and removes the
/// one from before the unbinding, leaving the ledger as it was.
#[tokio::test]
async fn a_deletion_is_ledgered_and_a_purge_removes_the_snapshots_before_it() {
	tokio::time::timeout(Duration::from_secs(60), async {
		let dir = tempfile::tempdir().unwrap();
		let home = dir.path().join(".jet");
		let daemon = start_jetd(&home).await;
		let client = support::connect(&daemon, Uuid::new_v4()).await;
		let binding = client
			.bind_account(
				Uuid::now_v7(),
				"anthropic",
				"Work",
				None,
				CredentialSource::HarnessNative,
			)
			.await
			.unwrap();
		let before = wait_for_daily_snapshot(&home).await;
		client
			.unbind_account(Uuid::now_v7(), binding.binding_id)
			.await
			.unwrap();
		let ledger = std::fs::read_to_string(
			home.join("recovery").join("plane.sqlite3.deletions"),
		)
		.unwrap();
		let recorded = client.status().await.unwrap().recovery.unwrap();

		let purged = client
			.purge_recovery_snapshots(Uuid::now_v7())
			.await
			.unwrap();

		let after = client.status().await.unwrap().recovery.unwrap();
		assert_eq!(
			(
				ledger.lines().count(),
				ledger.lines().nth(2).map(|line| line
					.split(' ')
					.nth(2)
					.map(ToOwned::to_owned)
					.unwrap()),
				ledger.contains(&binding.binding_id.to_string()),
				recorded.deletion_ledger,
				purged.removed,
				after.state,
				after
					.snapshots
					.iter()
					.map(|snapshot| (snapshot.name.clone(), snapshot.reason))
					.collect::<Vec<_>>(),
				after.deletion_ledger,
			),
			(
				3,
				Some("account_binding".to_string()),
				true,
				Some(DeletionLedgerStatus::Verified { deletions: 1 }),
				vec![before],
				RecoveryState::Serving,
				vec![(purged.snapshot, SnapshotReason::Maintenance)],
				Some(DeletionLedgerStatus::Verified { deletions: 1 }),
			)
		);
	})
	.await
	.unwrap();
}
