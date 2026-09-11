//! Recovery of the Plane store itself: verified Recovery snapshots and
//! when the core takes them (ADR-0097).
//!
//! The store decides whether a routine snapshot is due; the core supplies
//! the clock and the moments. A migration is snapshotted by the store as
//! it opens, destructive maintenance at start is preceded by one here, and
//! the first meaningful change of a day gets one when the daemon next
//! wakes for maintenance, which every Command and Effect commit triggers.

use crate::{Core, error::CoreError};
pub use jet_store::{RecoverySnapshot, SnapshotReason};

impl Core {
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
