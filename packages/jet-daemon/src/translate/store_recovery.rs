//! Store Recovery: mode, reason, and the snapshots a Plane can restore.

use jet_core::{
	IntegrityFailureReason, RecoveryMode, RecoverySnapshot, RecoveryStatus,
	SnapshotReason,
};
use jet_protocol as wire;

pub(super) fn recovery_status(status: &RecoveryStatus) -> wire::RecoveryStatus {
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
