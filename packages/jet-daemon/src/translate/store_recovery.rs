//! Store Recovery: mode, reason, and the snapshots a Plane can restore.

use jet_core::{
	DeletionLedger, IntegrityFailureReason, RecoveryMode, RecoverySnapshot,
	RecoveryStatus, SnapshotReason,
};
use jet_protocol as wire;

pub(super) fn recovery_status(
	status: &RecoveryStatus,
	minor: u32,
) -> wire::RecoveryStatus {
	let (state, reason) = match status.mode {
		RecoveryMode::Serving => (wire::RecoveryState::Serving, None),
		RecoveryMode::ReadOnly(reason) => {
			(wire::RecoveryState::ReadOnly, Some(recovery_reason(reason)))
		}
	};
	wire::RecoveryStatus {
		state,
		reason,
		snapshots: status.snapshots.iter().map(snapshot).collect(),
		// A client that negotiated an older minor does not name the
		// ledger, so it is not told about it either (ADR-0019).
		deletion_ledger: (minor >= wire::DELETION_LEDGER_MINOR)
			.then(|| deletion_ledger(&status.deletions)),
	}
}

/// The ledger's state without its records: the wire carries how many
/// deletions it vouches for, never which identities.
fn deletion_ledger(ledger: &DeletionLedger) -> wire::DeletionLedgerStatus {
	match ledger {
		DeletionLedger::Verified(records) => {
			wire::DeletionLedgerStatus::Verified {
				deletions: u64::try_from(records.len()).unwrap_or(u64::MAX),
			}
		}
		DeletionLedger::Corrupt(_) => wire::DeletionLedgerStatus::Corrupt,
	}
}

fn recovery_reason(reason: IntegrityFailureReason) -> wire::RecoveryReason {
	match reason {
		IntegrityFailureReason::IntegrityCheck => {
			wire::RecoveryReason::IntegrityCheckFailed
		}
		IntegrityFailureReason::Migration => {
			wire::RecoveryReason::MigrationFailed
		}
	}
}

fn snapshot(snapshot: &RecoverySnapshot) -> wire::RecoverySnapshot {
	wire::RecoverySnapshot {
		name: snapshot.name.clone(),
		taken_at_unix_ms: snapshot.taken_at_unix_ms,
		reason: match snapshot.reason {
			SnapshotReason::Daily => wire::SnapshotReason::Daily,
			SnapshotReason::Migration { .. } => wire::SnapshotReason::Migration,
			SnapshotReason::Maintenance => wire::SnapshotReason::Maintenance,
		},
		bytes: snapshot.bytes,
	}
}
