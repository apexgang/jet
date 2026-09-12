//! The interactive Recovery purge: a verified snapshot taken after every
//! recorded deletion, and the removal of every older snapshot that may
//! still hold what was deleted (ADR-0102).
//!
//! Routine retention keeps a deleted identity in bounded snapshots until
//! their disclosed expiry; the purge is how an owner ends that early. It
//! belongs to the person: Harnesses, Crafts, schedules, Utility models,
//! and Autodelete rules issue no Commands, so nothing but an interactive
//! client can reach it, and it is recorded in the Security audit as the
//! destructive decision it is.

use super::{RecoveryMode, read_only, restore::deletion_ledger_corrupt};
use crate::{
	Actor, Core,
	audit::{self, AuditDecision, AuditSubject, Decision},
	command::CommandOutcome,
	error::CoreError,
	security::SecurityClass,
};
use jet_store::DeletionLedger;

impl Core {
	/// Takes a verified snapshot of the store as it is now and removes
	/// every older snapshot taken at or before the newest deletion the
	/// ledger records, then records the purge in the Security audit.
	///
	/// This Command runs outside the receipt pipeline, because copying the
	/// store needs the one connection a receipt transaction would hold. It
	/// is not idempotent: a retry takes another snapshot and finds nothing
	/// older to remove.
	///
	/// # Errors
	///
	/// Returns an `unavailable` [`CoreError`] while the Plane is in
	/// Recovery mode or its Deletion ledger cannot be trusted, a
	/// `conflict` one while the Security audit is degraded, and a store
	/// category when the snapshot cannot be taken or verified.
	pub(crate) async fn purge_recovery_snapshots(
		&self,
		actor: &Actor,
	) -> Result<CommandOutcome, CoreError> {
		if let RecoveryMode::ReadOnly(reason) = self.recovery_mode() {
			return Err(read_only(reason));
		}
		actor.authorize(&self.remote_sessions)?;
		// ASVS 16.2.1: a destructive decision waits for an audit the Plane
		// can vouch for (ADR-0105).
		self.security.read().await.admit(SecurityClass::Guarded)?;
		if let DeletionLedger::Corrupt(detail) = self.store.deletion_ledger()? {
			return Err(deletion_ledger_corrupt(detail));
		}
		let now_unix_ms = self.now_unix_ms();
		let purge = self.store.purge_recovery_snapshots(now_unix_ms).await?;
		self.store
			.write(async |tx| {
				audit::record(
					tx,
					actor,
					Decision::succeeded(
						AuditDecision::RecoverySnapshotsPurged,
						AuditSubject::Plane,
					),
					now_unix_ms,
				)
				.await
			})
			.await?;
		Ok(CommandOutcome::RecoverySnapshotsPurged(purge))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		Command, ErrorCategory, Query, QueryResult, RecoveryStatus,
		SecurityState, SnapshotPurge,
		test_support::{
			FixedProbe, ManualClock, actor, bind_native_account, equipped,
			request, start_core_with,
		},
	};
	use jet_store::{DeletedIdentityKind, DeletionRecord};
	use pretty_assertions::assert_eq;
	use std::{
		sync::Arc,
		time::{Duration, UNIX_EPOCH},
	};

	const NOW: Duration = Duration::from_millis(1_700_000_000_000);
	const DAY: Duration = Duration::from_secs(24 * 60 * 60);

	async fn recovery(core: &Core) -> RecoveryStatus {
		let QueryResult::Status(status) =
			core.query(&actor(), Query::Status).await.unwrap()
		else {
			panic!("expected a status snapshot");
		};
		status.recovery
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

	/// The snapshot from before the unbinding goes, the one from after it
	/// stays beside the purge's own, the ledger still names the deletion,
	/// and the audit holds the decision (ADR-0102).
	#[tokio::test]
	async fn a_purge_removes_the_snapshots_that_predate_a_deletion() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start_core_with(
			&path,
			Arc::clone(&clock) as Arc<dyn crate::clock::Clock>,
			FixedProbe::new(equipped()),
		)
		.await;
		let binding =
			bind_native_account(&core, crate::ProviderId("anthropic".into()))
				.await;
		let before = core.snapshot_if_due().await.unwrap().unwrap();
		clock.advance(DAY);
		core.execute(
			&actor(),
			request(Command::UnbindAccount {
				binding_id: binding,
			}),
		)
		.await
		.unwrap();
		clock.advance(DAY);
		let after = core.snapshot_if_due().await.unwrap().unwrap();
		clock.advance(DAY);

		let outcome = core
			.execute(&actor(), request(Command::PurgeRecoverySnapshots))
			.await
			.unwrap();

		let status = recovery(&core).await;
		assert_eq!(
			(
				outcome,
				status
					.snapshots
					.iter()
					.map(|snapshot| snapshot.name.clone())
					.collect::<Vec<_>>(),
				status.deletions,
				decisions(&core).await,
			),
			(
				CommandOutcome::RecoverySnapshotsPurged(SnapshotPurge {
					snapshot: format!(
						"plane-{}-maintenance.sqlite3",
						NOW.as_millis() + 3 * DAY.as_millis()
					),
					removed: vec![before.name],
				}),
				vec![
					format!(
						"plane-{}-maintenance.sqlite3",
						NOW.as_millis() + 3 * DAY.as_millis()
					),
					after.name,
				],
				DeletionLedger::Verified(vec![DeletionRecord {
					sequence: 1,
					deleted_at_unix_ms: i64::try_from(
						NOW.as_millis() + DAY.as_millis()
					)
					.unwrap(),
					kind: DeletedIdentityKind::AccountBinding,
					identity: binding.0,
				}]),
				vec![
					"account.bound".to_string(),
					"account.unbound".to_string(),
					"recovery.snapshots_purged".to_string(),
				],
			)
		);
	}

	/// A degraded audit refuses the purge before anything is copied or
	/// removed, the way it refuses every destructive decision (ADR-0105).
	#[tokio::test]
	async fn a_purge_waits_for_an_audit_the_plane_can_vouch_for() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start_core_with(
			&path,
			Arc::clone(&clock) as Arc<dyn crate::clock::Clock>,
			FixedProbe::new(equipped()),
		)
		.await;
		let binding =
			bind_native_account(&core, crate::ProviderId("anthropic".into()))
				.await;
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
		std::fs::remove_file(path.with_file_name("plane.sqlite3.audit-head"))
			.unwrap();
		let core = start_core_with(
			&path,
			Arc::clone(&clock) as Arc<dyn crate::clock::Clock>,
			FixedProbe::new(equipped()),
		)
		.await;

		let error = core
			.execute(&actor(), request(Command::PurgeRecoverySnapshots))
			.await
			.unwrap_err();

		assert_eq!(
			(
				error.category,
				error.code.as_str(),
				recovery(&core).await.snapshots,
				matches!(core.security().await, SecurityState::Degraded(_)),
			),
			(
				ErrorCategory::Conflict,
				"security.audit_degraded",
				vec![],
				true
			)
		);
	}
}
