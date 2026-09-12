//! Recovery of the Plane store: whether it serves or answers reads only,
//! and the verified Recovery snapshots it can restore (ADR-0077,
//! ADR-0097). Introduced by protocol minor 37.

use serde::{Deserialize, Serialize};

/// Whether the Plane store is authoritative or in read-only Recovery mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RecoveryState {
	/// The store passed its checks and accepts Commands.
	Serving,
	/// It did not. Queries are answered as far as the damage allows; new
	/// Runs and every other Command wait until a verified snapshot is
	/// restored.
	ReadOnly,
}

/// What put the store into read-only Recovery mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RecoveryReason {
	/// SQLite's integrity check reported damage.
	IntegrityCheckFailed,
	/// A schema migration failed, leaving the store at its previous
	/// version (ADR-0073).
	MigrationFailed,
}

/// Why a Recovery snapshot was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SnapshotReason {
	/// The first meaningful committed change of a day.
	Daily,
	/// A schema migration was about to run.
	Migration,
	/// Destructive maintenance was about to run.
	Maintenance,
}

/// One verified Recovery snapshot the Plane can restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RecoverySnapshot {
	/// The name a restore request names it by.
	pub name: String,
	/// When it was taken, in signed Unix milliseconds.
	pub taken_at_unix_ms: i64,
	/// Why it was taken.
	pub reason: SnapshotReason,
	/// Its size on disk.
	pub bytes: u64,
}

/// The store's Recovery state and what it could be restored from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RecoveryStatus {
	/// Serving, or read-only.
	pub state: RecoveryState,
	/// Why it is read-only. Present exactly when `state` is `read_only`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub reason: Option<RecoveryReason>,
	/// Every verified snapshot, newest first.
	pub snapshots: Vec<RecoverySnapshot>,
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	#[test]
	fn a_read_only_plane_reports_its_state_in_the_agreed_wire_shape() {
		let status = RecoveryStatus {
			state: RecoveryState::ReadOnly,
			reason: Some(RecoveryReason::IntegrityCheckFailed),
			snapshots: vec![RecoverySnapshot {
				name: "plane-1700000000000-daily.sqlite3".into(),
				taken_at_unix_ms: 1_700_000_000_000,
				reason: SnapshotReason::Daily,
				bytes: 4096,
			}],
		};
		let serving = RecoveryStatus {
			state: RecoveryState::Serving,
			reason: None,
			snapshots: vec![],
		};
		assert_eq!(
			(
				serde_json::to_string(&status).unwrap(),
				serde_json::to_string(&serving).unwrap()
			),
			(
				r#"{"state":"read_only","reason":"integrity_check_failed","snapshots":[{"name":"plane-1700000000000-daily.sqlite3","taken_at_unix_ms":1700000000000,"reason":"daily","bytes":4096}]}"#.to_string(),
				r#"{"state":"serving","snapshots":[]}"#.to_string()
			)
		);
	}
}
