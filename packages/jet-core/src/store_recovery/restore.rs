//! Restoring a verified Recovery snapshot over a damaged store, and the
//! refusal every other Command meets while the store is in doubt
//! (ADR-0077).

use super::RecoveryMode;
use crate::{
	Actor, Core,
	audit::{self, AuditDecision, AuditSubject, Decision},
	command::CommandOutcome,
	error::CoreError,
	security::SecurityState,
};
use jet_store::{DeletionLedger, IntegrityFailureReason, StoreIntegrity};

/// The refusal a Command meets in read-only Recovery mode. It is
/// `unavailable` and retryable: the same Command succeeds once a snapshot
/// has been restored, and no receipt records it (ADR-0093).
pub(crate) fn read_only(reason: IntegrityFailureReason) -> CoreError {
	CoreError::unavailable(
		"recovery.read_only",
		"this Plane is in Recovery mode: its store failed its checks, so it \
		 accepts no new Runs or changes until a verified Recovery snapshot \
		 is restored",
		match reason {
			IntegrityFailureReason::IntegrityCheck => "integrity check failed",
			IntegrityFailureReason::Migration => "schema migration failed",
		},
	)
}

/// The refusal a restoration meets while the Deletion ledger vouches for
/// nothing: the snapshot may predate a deletion nothing else remembers,
/// so writable restoration stays refused (ADR-0102).
pub(crate) fn deletion_ledger_corrupt(detail: String) -> CoreError {
	CoreError::unavailable(
		"recovery.deletion_ledger_corrupt",
		"this Plane's Deletion ledger cannot be trusted, so no snapshot is \
		 restored over its store: restoring one could bring back what was \
		 deleted after it was taken",
		detail,
	)
}

/// The refusal a restoration meets on a Plane that serves.
pub(crate) fn not_read_only() -> CoreError {
	CoreError::conflict(
		"recovery.not_read_only",
		"this Plane is serving; only a Plane in Recovery mode restores a \
		 snapshot over its store",
	)
}

impl Core {
	/// Restores the verified snapshot called `snapshot` over the damaged
	/// store and brings the Plane back to serving: the daemon start is
	/// recorded, the Security audit validated against the restored state,
	/// the restoration itself recorded in the audit when that chain is
	/// whole, and the search index caught up, before workers waiting on
	/// [`Core::wait_until_serving`] resume (ADR-0077).
	///
	/// The Deletion ledger is reapplied to the restored store, so an
	/// identity deleted after the snapshot was taken does not come back
	/// with it (ADR-0102).
	///
	/// A snapshot older than the newest audit record leaves the audit
	/// head naming a record the store no longer holds. That is the audit's
	/// own evidence of the rollback, and appending to the chain would
	/// publish a new head over it, so the Plane stays Security-degraded
	/// until an owner begins a new epoch, whose first record says so
	/// (ADR-0105).
	///
	/// This Command runs outside the receipt pipeline, because the store a
	/// receipt would be written to is the one being replaced. It is not
	/// idempotent: a second request meets `recovery.not_read_only`.
	///
	/// # Errors
	///
	/// Returns a `conflict` [`CoreError`] when the Plane is serving, an
	/// `internal` one when no verified snapshot has that name, and an
	/// `unavailable` one when the Deletion ledger cannot be trusted or the
	/// restored store fails its own checks, in which case the Plane stays
	/// in Recovery mode.
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "serializes the store swap with Effect decisions and adoption"
	)]
	pub(crate) async fn restore_recovery_snapshot(
		&self,
		actor: &Actor,
		snapshot: String,
	) -> Result<CommandOutcome, CoreError> {
		let RecoveryMode::ReadOnly(_) = self.recovery_mode() else {
			return Err(not_read_only());
		};
		// Nothing else decides an Effect while the store is being replaced.
		let _reconciliation = self.effect_reconciliation.lock().await;
		if let DeletionLedger::Corrupt(detail) = self.store.deletion_ledger()? {
			return Err(deletion_ledger_corrupt(detail));
		}
		let now_unix_ms = self.now_unix_ms();
		let restored = self.store.restore(&snapshot, now_unix_ms).await?;
		if let StoreIntegrity::Failed(failure) = self.store.integrity() {
			self.recovery.send_replace(RecoveryMode::of(
				&StoreIntegrity::Failed(failure.clone()),
			));
			return Err(CoreError::unavailable(
				"recovery.restore_failed",
				"the restored snapshot did not pass the store's checks; the \
				 Plane stays in Recovery mode",
				failure.detail,
			));
		}
		self.store.record_daemon_start().await?;
		// Restoring moves state backwards and the audit head stayed put, so
		// the chain is validated against what the store holds now
		// (ADR-0105).
		let security = SecurityState::of(self.store.validate_audit().await?);
		*self.security.write().await = security;
		if security == SecurityState::Trusted {
			self.store
				.write(async |tx| {
					audit::record(
						tx,
						actor,
						Decision::succeeded(
							AuditDecision::RecoverySnapshotRestored,
							AuditSubject::Plane,
						),
						now_unix_ms,
					)
					.await
				})
				.await?;
		}
		self.index_search().await?;
		self.recovery.send_replace(RecoveryMode::Serving);
		Ok(CommandOutcome::RecoverySnapshotRestored(restored))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		Command, ErrorCategory, PairingGate, Query, QueryResult,
		RecoveryStatus, RestoredStore, SnapshotReason,
		test_support::{
			FixedProbe, ManualClock, actor, bind_native_account, equipped,
			request, start_core_with,
		},
	};
	use pretty_assertions::assert_eq;
	use std::{
		io::{Seek as _, SeekFrom, Write as _},
		path::Path,
		sync::Arc,
		time::{Duration, UNIX_EPOCH},
	};

	const NOW: Duration = Duration::from_millis(1_700_000_000_000);

	/// Overwrites the second page of a closed database.
	fn damage(path: &Path) {
		let mut file =
			std::fs::OpenOptions::new().write(true).open(path).unwrap();
		file.seek(SeekFrom::Start(4096)).unwrap();
		file.write_all(&[0xff; 4096]).unwrap();
		file.sync_all().unwrap();
	}

	async fn status(core: &Core) -> (RecoveryStatus, SecurityState) {
		let QueryResult::Status(status) =
			core.query(&actor(), Query::Status).await.unwrap()
		else {
			panic!("expected a status snapshot");
		};
		(status.recovery, status.security)
	}

	async fn decisions(core: &Core) -> Vec<String> {
		let QueryResult::SecurityAudit(page) = core
			.query(
				&actor(),
				Query::SecurityAudit {
					after: crate::AuditSequence(0),
				},
			)
			.await
			.unwrap()
		else {
			panic!("expected an audit page");
		};
		page.entries
			.into_iter()
			.map(|entry| entry.decision)
			.collect()
	}

	async fn start(path: &Path, clock: &Arc<ManualClock>) -> Core {
		start_core_with(
			path,
			Arc::clone(clock) as Arc<dyn crate::clock::Clock>,
			FixedProbe::new(equipped()),
		)
		.await
	}

	/// A core on a damaged store answers Queries, refuses Commands without
	/// a receipt, restores the snapshot it names, and serves again with the
	/// restoration in its audit; a second restoration is refused
	/// (ADR-0077).
	#[tokio::test]
	async fn a_damaged_store_is_read_only_until_its_snapshot_is_restored() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start(&path, &clock).await;
		let gate = |gate| request(Command::SetPairingGate { gate });
		core.execute(&actor(), gate(PairingGate::Closed))
			.await
			.unwrap();
		let snapshot = core.snapshot_if_due().await.unwrap().unwrap();
		core.close().await;
		drop(core);
		damage(&path);

		let core = start(&path, &clock).await;
		assert_eq!(
			status(&core).await,
			(
				RecoveryStatus {
					mode: RecoveryMode::ReadOnly(
						IntegrityFailureReason::IntegrityCheck
					),
					snapshots: vec![snapshot.clone()],
					deletions: DeletionLedger::Verified(vec![]),
				},
				SecurityState::Trusted,
			)
		);
		let refused = core
			.execute(&actor(), gate(PairingGate::Open))
			.await
			.unwrap_err();
		assert_eq!(
			(refused.category, refused.code.as_str(), refused.retryable),
			(ErrorCategory::Unavailable, "recovery.read_only", true)
		);

		let outcome = core
			.execute(
				&actor(),
				request(Command::RestoreRecoverySnapshot {
					snapshot: snapshot.name.clone(),
				}),
			)
			.await
			.unwrap();
		assert_eq!(
			outcome,
			CommandOutcome::RecoverySnapshotRestored(RestoredStore {
				snapshot: snapshot.name.clone(),
				replaced: "plane.sqlite3.damaged-1700000000000".into(),
			})
		);
		assert_eq!(
			status(&core).await,
			(
				RecoveryStatus {
					mode: RecoveryMode::Serving,
					snapshots: vec![snapshot.clone()],
					deletions: DeletionLedger::Verified(vec![]),
				},
				SecurityState::Trusted,
			)
		);
		assert_eq!(
			decisions(&core).await,
			vec![
				"pairing.gate_closed".to_string(),
				"recovery.snapshot_restored".to_string()
			]
		);
		assert!(
			dir.path()
				.join("plane.sqlite3.damaged-1700000000000")
				.exists()
		);
		assert_eq!(snapshot.reason, SnapshotReason::Daily);
		core.execute(&actor(), gate(PairingGate::Open))
			.await
			.unwrap();
		let again = core
			.execute(
				&actor(),
				request(Command::RestoreRecoverySnapshot {
					snapshot: snapshot.name,
				}),
			)
			.await
			.unwrap_err();
		assert_eq!(again.code, "recovery.not_read_only");
	}

	/// A snapshot older than the newest audit record leaves the head
	/// naming a record the restored store does not hold. The restoration
	/// does not paper over that: the Plane serves Security-degraded, and
	/// only an owner's new epoch carries the audit on (ADR-0105).
	#[tokio::test]
	async fn restoring_behind_the_audit_head_leaves_the_plane_degraded() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start(&path, &clock).await;
		let gate = |gate| request(Command::SetPairingGate { gate });
		core.execute(&actor(), gate(PairingGate::Closed))
			.await
			.unwrap();
		let snapshot = core.snapshot_if_due().await.unwrap().unwrap();
		// An audited decision the snapshot does not hold.
		core.execute(&actor(), gate(PairingGate::Open))
			.await
			.unwrap();
		core.close().await;
		drop(core);
		damage(&path);

		let core = start(&path, &clock).await;
		core.execute(
			&actor(),
			request(Command::RestoreRecoverySnapshot {
				snapshot: snapshot.name,
			}),
		)
		.await
		.unwrap();
		let (recovery, security) = status(&core).await;
		let SecurityState::Degraded(degradation) = security else {
			panic!("the rollback went unnoticed: {security:?}");
		};
		assert_eq!(
			(recovery.mode, degradation.breach, decisions(&core).await),
			(
				RecoveryMode::Serving,
				jet_store::AuditBreach::HeadNotInStore,
				vec!["pairing.gate_closed".to_string()]
			)
		);
		// Restarting sees the same evidence: nothing republished the head.
		core.close().await;
		drop(core);
		let core = start(&path, &clock).await;
		assert_eq!(status(&core).await.1, security);
		core.execute(&actor(), request(Command::BeginAuditEpoch))
			.await
			.unwrap();
		assert_eq!(status(&core).await.1, SecurityState::Trusted);
	}

	/// A snapshot the directory does not hold cannot be restored, and the
	/// Plane stays where it was.
	#[tokio::test]
	async fn an_unknown_snapshot_leaves_the_plane_in_recovery_mode() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start(&path, &clock).await;
		core.close().await;
		drop(core);
		damage(&path);
		let core = start(&path, &clock).await;
		let error = core
			.execute(
				&actor(),
				request(Command::RestoreRecoverySnapshot {
					snapshot: "plane-1-daily.sqlite3".into(),
				}),
			)
			.await
			.unwrap_err();
		assert_eq!(
			(error.code.as_str(), core.recovery_mode()),
			(
				"store.integrity",
				RecoveryMode::ReadOnly(IntegrityFailureReason::IntegrityCheck)
			)
		);
	}

	async fn binding_ids(core: &Core) -> Vec<crate::AccountBindingId> {
		let QueryResult::AccountBindings(list) = core
			.query(
				&actor(),
				Query::AccountBindings {
					observation: crate::CapabilityObservation::LastObserved,
				},
			)
			.await
			.unwrap()
		else {
			panic!("expected an Account binding list");
		};
		list.bindings
			.into_iter()
			.map(|status| status.binding.binding_id)
			.collect()
	}

	/// An Account binding unbound after the snapshot was taken is in the
	/// Deletion ledger, and restoring the snapshot reapplies the ledger, so
	/// the binding does not come back with the store (ADR-0102).
	#[tokio::test]
	async fn an_unbinding_made_after_the_snapshot_survives_its_restoration() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start(&path, &clock).await;
		let unbound =
			bind_native_account(&core, crate::ProviderId("anthropic".into()))
				.await;
		let kept =
			bind_native_account(&core, crate::ProviderId("openai".into()))
				.await;
		let snapshot = core.snapshot_if_due().await.unwrap().unwrap();
		core.execute(
			&actor(),
			request(Command::UnbindAccount {
				binding_id: unbound,
			}),
		)
		.await
		.unwrap();
		core.close().await;
		drop(core);
		damage(&path);

		let core = start(&path, &clock).await;
		core.execute(
			&actor(),
			request(Command::RestoreRecoverySnapshot {
				snapshot: snapshot.name,
			}),
		)
		.await
		.unwrap();

		assert_eq!(
			(core.recovery_mode(), binding_ids(&core).await),
			(RecoveryMode::Serving, vec![kept])
		);
	}

	/// With the ledger gone, the snapshot may predate a deletion nothing
	/// else remembers, so the restoration is refused and the damaged store
	/// is left exactly where it was (ADR-0102).
	#[tokio::test]
	async fn a_corrupt_deletion_ledger_keeps_the_plane_in_recovery_mode() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start(&path, &clock).await;
		let binding =
			bind_native_account(&core, crate::ProviderId("anthropic".into()))
				.await;
		let snapshot = core.snapshot_if_due().await.unwrap().unwrap();
		core.execute(
			&actor(),
			request(Command::UnbindAccount {
				binding_id: binding,
			}),
		)
		.await
		.unwrap();
		core.close().await;
		drop(core);
		std::fs::remove_file(
			dir.path().join("recovery").join("plane.sqlite3.deletions"),
		)
		.unwrap();
		damage(&path);

		let core = start(&path, &clock).await;
		let error = core
			.execute(
				&actor(),
				request(Command::RestoreRecoverySnapshot {
					snapshot: snapshot.name,
				}),
			)
			.await
			.unwrap_err();

		assert_eq!(
			(
				error.code.as_str(),
				error.category,
				core.recovery_mode(),
				std::fs::read_dir(dir.path())
					.unwrap()
					.filter_map(|entry| entry
						.unwrap()
						.file_name()
						.into_string()
						.ok())
					.filter(|name| name.contains(".damaged-"))
					.count(),
			),
			(
				"recovery.deletion_ledger_corrupt",
				ErrorCategory::Unavailable,
				RecoveryMode::ReadOnly(IntegrityFailureReason::IntegrityCheck),
				0,
			)
		);
	}
}
