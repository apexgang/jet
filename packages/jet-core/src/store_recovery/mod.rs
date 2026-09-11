//! Recovery of the Plane store itself: verified Recovery snapshots, when
//! the core takes them (ADR-0097), and read-only Recovery mode, which the
//! Plane enters when its store fails the checks that make it
//! authoritative (ADR-0077).
//!
//! The store decides whether a routine snapshot is due; the core supplies
//! the clock and the moments. A migration is snapshotted by the store as
//! it opens, destructive maintenance at start is preceded by one here, and
//! the first meaningful change of a day gets one when the daemon next
//! wakes for maintenance, which every Command and Effect commit triggers.
//!
//! In Recovery mode the Plane still answers Queries, so it can be
//! diagnosed and exported, and refuses every Command but one: restoring a
//! verified snapshot, which is the deliberate way out and belongs to the
//! person. Nothing that writes runs meanwhile, and surviving `jetfueld`
//! executions are reconnected only after the restoration succeeds.

mod restore;
pub(crate) use restore::read_only;

use crate::{Core, error::CoreError};
use jet_store::{IntegrityFailureReason, StoreIntegrity};
pub use jet_store::{RecoverySnapshot, SnapshotReason};

/// Whether the Plane store serves or answers reads only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryMode {
	/// The store passed its checks and accepts Commands.
	Serving,
	/// It did not, and the Plane is in read-only Recovery mode.
	ReadOnly(RecoveryReason),
}

/// What put the store into read-only Recovery mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryReason {
	/// SQLite's integrity check reported damage.
	IntegrityCheckFailed,
	/// A schema migration failed, leaving the store at its previous
	/// version (ADR-0073).
	MigrationFailed,
}

impl RecoveryMode {
	/// The mode a store's integrity puts the Plane in.
	pub(crate) fn of(integrity: &StoreIntegrity) -> Self {
		match integrity {
			StoreIntegrity::Verified => Self::Serving,
			StoreIntegrity::Failed(failure) => {
				Self::ReadOnly(match failure.reason {
					IntegrityFailureReason::IntegrityCheck => {
						RecoveryReason::IntegrityCheckFailed
					}
					IntegrityFailureReason::Migration => {
						RecoveryReason::MigrationFailed
					}
				})
			}
		}
	}
}

/// The store's Recovery mode and what it could be restored from, as the
/// Plane status reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryStatus {
	/// Serving, or read-only and why.
	pub mode: RecoveryMode,
	/// Every verified snapshot, newest first.
	pub snapshots: Vec<RecoverySnapshot>,
}

impl Core {
	/// Whether the Plane store serves or answers reads only, right now
	/// (ADR-0077).
	#[must_use]
	pub fn recovery_mode(&self) -> RecoveryMode {
		*self.recovery.borrow()
	}

	/// Resolves once the Plane serves: at once when it does already, or
	/// after a snapshot is restored. The daemon's workers wait here so
	/// nothing writes, reconciles, or reconnects while the store is in
	/// doubt.
	pub async fn wait_until_serving(&self) {
		let mut mode = self.recovery.subscribe();
		// The sender lives as long as the core, so waiting cannot fail.
		let _ = mode.wait_for(|mode| *mode == RecoveryMode::Serving).await;
	}

	/// The Recovery mode and the snapshots the Plane could restore.
	pub(crate) fn recovery_status(&self) -> Result<RecoveryStatus, CoreError> {
		Ok(RecoveryStatus {
			mode: self.recovery_mode(),
			snapshots: self.recovery_snapshots()?,
		})
	}

	/// Takes the day's Recovery snapshot when a committed change is the
	/// first of its day, and returns it (ADR-0097). Nothing is taken while
	/// the store is unchanged since the newest snapshot.
	///
	/// # Errors
	///
	/// Returns a store category [`CoreError`] when a due snapshot cannot be
	/// written or fails verification. The change it followed is already
	/// durable either way.
	pub async fn snapshot_if_due(
		&self,
	) -> Result<Option<RecoverySnapshot>, CoreError> {
		Ok(self
			.store
			.snapshot_if_due(SnapshotReason::Daily, self.now_unix_ms())
			.await?)
	}

	/// Every verified Recovery snapshot of this Plane, newest first.
	///
	/// # Errors
	///
	/// Returns a store category [`CoreError`] when the snapshot directory
	/// cannot be read.
	pub fn recovery_snapshots(
		&self,
	) -> Result<Vec<RecoverySnapshot>, CoreError> {
		Ok(self.store.recovery_snapshots()?)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		Command, PairingGate,
		test_support::{
			FixedProbe, ManualClock, actor, equipped, request, start_core_with,
		},
	};
	use pretty_assertions::assert_eq;
	use std::{
		sync::Arc,
		time::{Duration, UNIX_EPOCH},
	};

	const NOW: Duration = Duration::from_millis(1_700_000_000_000);
	const DAY: Duration = Duration::from_secs(24 * 60 * 60);

	/// Starting a core creates the store and takes no snapshot of it; the
	/// first Command of a day earns one, the second does not, and the next
	/// day starts over (ADR-0097).
	#[tokio::test]
	async fn the_first_change_of_a_day_is_snapshotted_once() {
		let dir = tempfile::tempdir().unwrap();
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start_core_with(
			&dir.path().join("plane.sqlite3"),
			Arc::clone(&clock) as Arc<dyn crate::clock::Clock>,
			FixedProbe::new(equipped()),
		)
		.await;
		let gate = |gate| request(Command::SetPairingGate { gate });
		core.execute(&actor(), gate(PairingGate::Closed))
			.await
			.unwrap();
		let first = core.snapshot_if_due().await.unwrap().unwrap();
		assert_eq!(
			(first.reason, first.taken_at_unix_ms),
			(SnapshotReason::Daily, 1_700_000_000_000)
		);
		// A later change the same day waits for the next day.
		core.execute(&actor(), gate(PairingGate::Open))
			.await
			.unwrap();
		assert_eq!(core.snapshot_if_due().await.unwrap(), None);
		clock.advance(DAY);
		let second = core.snapshot_if_due().await.unwrap().unwrap();
		assert_eq!(
			core.recovery_snapshots().unwrap(),
			vec![second.clone(), first.clone()]
		);
		assert!(second.bytes > 0 && first.bytes > 0);
		// Unchanged since that snapshot: a new day alone is not a reason.
		clock.advance(DAY);
		assert_eq!(core.snapshot_if_due().await.unwrap(), None);
	}

	/// A restarted core's start-time maintenance is preceded by a snapshot
	/// of the state it is about to sweep (ADR-0097).
	#[tokio::test]
	async fn maintenance_at_start_is_preceded_by_a_snapshot() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start_core_with(
			&path,
			Arc::clone(&clock) as Arc<dyn crate::clock::Clock>,
			FixedProbe::new(equipped()),
		)
		.await;
		assert_eq!(core.recovery_snapshots().unwrap(), vec![]);
		core.close().await;
		drop(core);
		clock.advance(DAY);
		let restarted = start_core_with(
			&path,
			Arc::clone(&clock) as Arc<dyn crate::clock::Clock>,
			FixedProbe::new(equipped()),
		)
		.await;
		let snapshots = restarted.recovery_snapshots().unwrap();
		assert_eq!(
			snapshots
				.iter()
				.map(|snapshot| (snapshot.reason, snapshot.taken_at_unix_ms))
				.collect::<Vec<_>>(),
			vec![(SnapshotReason::Maintenance, 1_700_086_400_000)]
		);
	}
}
