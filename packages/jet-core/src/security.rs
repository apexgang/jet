//! Security-degraded mode: what a Plane still does when it can no longer
//! vouch for its own Security audit (ADR-0105).
//!
//! Validation runs when the daemon starts. When it fails, the Plane does
//! not stop: reads, exports and Runs already under way carry on, because
//! refusing them would destroy the evidence and the work at once. What
//! waits is every change the audit would have recorded — trust, policy,
//! Craft, and anything destructive — since the whole point of recording
//! them is a record that can be relied on.
//!
//! The way out is deliberate and belongs to the person, not the daemon.
//! An owner exports the evidence and begins a new authority epoch, which
//! records where the old chain was last known to have reached and why it
//! stops being vouched for there. Nothing here recovers by itself.

use crate::{
	Actor,
	audit::{
		self, AuditDecision, AuditEpoch, AuditSequence, AuditSubject, Decision,
	},
	command::CommandOutcome,
	error::CoreError,
	event::{EventKind, EventSubject},
};
use jet_store::{
	AuditBreach, AuditGap, AuditHead, AuditIntegrity, AuditIntegrityFailure,
	WriteTransaction,
};

/// Whether the Plane can vouch for its own Security audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityState {
	/// The audit chain folds through the head kept outside the store.
	Trusted,
	/// It does not, and the Plane is in Security-degraded mode.
	Degraded(SecurityDegradation),
}

/// What validation found. It names positions and hashes and quotes no
/// record content, because the audit holds none; this is the evidence an
/// owner exports before deciding to carry on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecurityDegradation {
	/// What was found to be wrong.
	pub breach: AuditBreach,
	/// The authority epoch that failed to validate.
	pub epoch: AuditEpoch,
	/// The head published outside the store, when there still is one.
	pub head: Option<AuditHead>,
	/// The newest position the store itself holds.
	pub store_sequence: AuditSequence,
}

/// Whether a Command may run while the audit cannot be vouched for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecurityClass {
	/// Ordinary work. It neither widens trust nor destroys anything, so a
	/// doubtful audit is no reason to refuse it.
	Ordinary,
	/// A change the Security audit exists to record. It waits for an audit
	/// the Plane can vouch for.
	Guarded,
}

impl SecurityState {
	/// The state validation leaves the Plane in.
	pub(crate) fn of(integrity: AuditIntegrity) -> Self {
		match integrity {
			AuditIntegrity::Verified { .. } => Self::Trusted,
			AuditIntegrity::Failed(AuditIntegrityFailure {
				breach,
				epoch,
				head,
				store_sequence,
			}) => Self::Degraded(SecurityDegradation {
				breach,
				epoch: AuditEpoch(epoch),
				head,
				store_sequence: AuditSequence(store_sequence),
			}),
		}
	}

	/// Lets `class` through, or refuses it while the audit is in doubt.
	///
	/// # Errors
	///
	/// Returns a `conflict` [`CoreError`] naming what the owner has to do,
	/// because nothing the client retries will change the answer.
	pub(crate) fn admit(self, class: SecurityClass) -> Result<(), CoreError> {
		match (self, class) {
			(Self::Trusted, _)
			| (Self::Degraded(_), SecurityClass::Ordinary) => Ok(()),
			(Self::Degraded(_), SecurityClass::Guarded) => {
				Err(CoreError::conflict(
					"security.audit_degraded",
					"this Plane cannot vouch for its Security audit; export \
					 the evidence and begin a new audit epoch before \
					 changing trust, policy, or anything destructive",
				))
			}
		}
	}

	fn degradation(self) -> Option<SecurityDegradation> {
		match self {
			Self::Trusted => None,
			Self::Degraded(degradation) => Some(degradation),
		}
	}
}

/// Begins the authority epoch that succeeds a chain the Plane stopped
/// vouching for, and records that as the first decision in it.
///
/// The gap is taken from what validation found rather than from anything a
/// client sends: an owner decides to carry on, and the Plane decides what
/// carrying on has to admit to.
///
/// # Errors
///
/// Returns a `conflict` [`CoreError`] when the audit is not in doubt, an
/// `internal` one when the failure named no position to succeed, and a
/// store category when the epoch cannot be written.
pub(crate) async fn begin_epoch(
	tx: &mut WriteTransaction,
	actor: &Actor,
	security: SecurityState,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let Some(degradation) = security.degradation() else {
		return Err(CoreError::conflict(
			"security.audit_trusted",
			"this Plane vouches for its Security audit, so there is no gap \
			 to carry on from",
		));
	};
	let gap = degradation.gap(tx).await?;
	let epoch = AuditEpoch(tx.begin_audit_epoch(gap, now_unix_ms).await?);
	tx.append_event(EventKind::AuditEpochBegun { epoch }.to_record(
		actor,
		EventSubject::Plane,
		now_unix_ms,
	)?)
	.await?;
	// The first record of the new epoch says who began it, so the chain the
	// Plane now vouches for starts by admitting why it starts.
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::AuditEpochBegun,
			AuditSubject::Plane,
		),
		now_unix_ms,
	)
	.await?;
	Ok(CommandOutcome::AuditEpochBegun { epoch })
}

impl SecurityDegradation {
	/// Where the chain being left behind was last known to have reached.
	///
	/// The head is the better witness, because it lives outside the state
	/// that may have moved. When it is gone the store's own newest record
	/// is all there is, and the epoch records that instead.
	async fn gap(
		self,
		tx: &mut WriteTransaction,
	) -> Result<AuditGap, CoreError> {
		let reason = self.breach.as_str().to_owned();
		if let Some(head) = self.head {
			return Ok(AuditGap {
				sequence: head.sequence,
				entry_hash: head.entry_hash,
				reason,
			});
		}
		let Some(tip) = tx.audit_tip().await? else {
			return Err(CoreError::internal(
				"security.gap_unknown",
				"the audit failed to validate with neither a head nor a \
				 record to succeed",
			));
		};
		Ok(AuditGap {
			sequence: tip.sequence,
			entry_hash: tip.entry_hash,
			reason,
		})
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use std::path::Path;

	use jet_store::{AuditBreach, audit_head_path};
	use pretty_assertions::assert_eq;

	use crate::test_support::{
		actor, register_repository, request, start_core,
	};
	use crate::{
		AuditEpoch, AuditSequence, Command, CommandOutcome, Core, CoreError,
		CredentialSource, ErrorCategory, ProviderId, Query, QueryResult,
		RetentionPolicy, SecurityDegradation, SecurityState, SettingKey,
		SettingScope, SettingValue, WorkingTreeRequest,
	};

	async fn bind(core: &Core) -> Result<CommandOutcome, CoreError> {
		core.execute(
			&actor(),
			request(Command::BindAccount {
				provider: ProviderId("anthropic".into()),
				label: "Work account".into(),
				provider_account: None,
				credential_source: CredentialSource::PlatformStore,
			}),
		)
		.await
	}

	/// Starts a core on a Plane whose audit head was lost, which is what a
	/// store restored from a snapshot looks like from outside.
	async fn start_without_a_head(path: &Path) -> Core {
		let recorded = start_core(path).await;
		bind(&recorded).await.unwrap();
		recorded.close().await;
		drop(recorded);
		std::fs::remove_file(audit_head_path(path)).unwrap();
		start_core(path).await
	}

	/// Copies a closed store the way a Recovery snapshot does, taking the
	/// write-ahead log with it when one is still there.
	fn copy_store(from: &Path, to: &Path) {
		std::fs::copy(from, to).unwrap();
		for suffix in ["-wal", "-shm"] {
			let mut source = from.as_os_str().to_owned();
			source.push(suffix);
			let mut target = to.as_os_str().to_owned();
			target.push(suffix);
			let (source, target) = (Path::new(&source), Path::new(&target));
			if source.exists() {
				std::fs::copy(source, target).unwrap();
			} else if target.exists() {
				std::fs::remove_file(target).unwrap();
			}
		}
	}

	fn degradation(state: SecurityState) -> SecurityDegradation {
		match state {
			SecurityState::Degraded(degradation) => degradation,
			SecurityState::Trusted => panic!("the Plane vouches for its audit"),
		}
	}

	#[tokio::test]
	async fn an_audit_that_cannot_be_validated_degrades_the_plane() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");

		let core = start_without_a_head(&path).await;

		assert_eq!(
			degradation(core.security().await),
			SecurityDegradation {
				breach: AuditBreach::HeadMissing,
				epoch: AuditEpoch(1),
				head: None,
				store_sequence: AuditSequence(1),
			}
		);
	}

	#[tokio::test]
	async fn a_degraded_plane_refuses_to_change_trust_and_keeps_working() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let core = start_without_a_head(&path).await;

		let refused = bind(&core).await.unwrap_err();
		let ordinary = core
			.execute(
				&actor(),
				request(Command::CreateConversation {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRequest::NoProject,
				}),
			)
			.await;
		let readable = core
			.query(
				&actor(),
				Query::SecurityAudit {
					after: AuditSequence(0),
				},
			)
			.await
			.unwrap();
		let QueryResult::SecurityAudit(page) = readable else {
			panic!("expected QueryResult::SecurityAudit");
		};

		assert_eq!(
			(
				(refused.category, refused.code.as_str()),
				matches!(ordinary, Ok(CommandOutcome::ConversationCreated(_))),
				page.entries.len(),
			),
			(
				(ErrorCategory::Conflict, "security.audit_degraded"),
				true,
				1,
			)
		);
	}

	#[tokio::test]
	async fn a_degraded_plane_refuses_a_policy_change_but_not_a_preference() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		// Registering a Project is itself a decision the audit guards, so the
		// Project is registered while the Plane still vouches for its audit.
		let trusted = start_core(&path).await;
		let project = SettingScope::Project {
			project_id: register_repository(&trusted, &dir.path().join("repo"))
				.await,
		};
		trusted.close().await;
		drop(trusted);
		std::fs::remove_file(audit_head_path(&path)).unwrap();
		let core = start_core(&path).await;

		let policy = core
			.execute(
				&actor(),
				request(Command::SetSetting {
					key: SettingKey::GitAutoCommit,
					scope: project,
					value: SettingValue::Flag(true),
				}),
			)
			.await
			.unwrap_err();
		let preference = core
			.execute(
				&actor(),
				request(Command::SetSetting {
					key: SettingKey::UtilityAutomaticNaming,
					scope: project,
					value: SettingValue::Flag(false),
				}),
			)
			.await;

		assert_eq!(
			(policy.code.as_str(), preference.is_ok()),
			("security.audit_degraded", true)
		);
	}

	#[tokio::test]
	async fn beginning_an_epoch_records_the_gap_and_restores_trust() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let core = start_without_a_head(&path).await;

		let begun = core
			.execute(&actor(), request(Command::BeginAuditEpoch))
			.await
			.unwrap();
		let bound = bind(&core).await;

		let QueryResult::SecurityAudit(page) = core
			.query(
				&actor(),
				Query::SecurityAudit {
					after: AuditSequence(0),
				},
			)
			.await
			.unwrap()
		else {
			panic!("unexpected result");
		};
		assert_eq!(
			(
				begun,
				core.security().await,
				matches!(bound, Ok(CommandOutcome::AccountBound(_))),
				page.entries
					.iter()
					.map(|entry| (entry.epoch, entry.decision.as_str()))
					.collect::<Vec<_>>(),
			),
			(
				CommandOutcome::AuditEpochBegun {
					epoch: AuditEpoch(2)
				},
				SecurityState::Trusted,
				true,
				vec![
					(AuditEpoch(1), "account.bound"),
					(AuditEpoch(2), "audit.epoch_begun"),
					(AuditEpoch(2), "account.bound"),
				],
			)
		);
	}

	#[tokio::test]
	async fn a_plane_that_vouches_for_its_audit_has_no_gap_to_carry_on_from() {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core(&dir.path().join("plane.sqlite3")).await;

		let refused = core
			.execute(&actor(), request(Command::BeginAuditEpoch))
			.await
			.unwrap_err();

		assert_eq!(
			(core.security().await, refused.code.as_str()),
			(SecurityState::Trusted, "security.audit_trusted")
		);
	}

	/// A store put back from a snapshot is the failure Security-degraded mode
	/// exists for, and one epoch is what the owner is told it costs.
	#[tokio::test]
	async fn one_epoch_is_enough_to_carry_on_from_a_restored_store() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let snapshot = dir.path().join("snapshot.sqlite3");

		let core = start_core(&path).await;
		bind(&core).await.unwrap();
		core.close().await;
		drop(core);
		copy_store(&path, &snapshot);

		let core = start_core(&path).await;
		bind(&core).await.unwrap();
		core.close().await;
		drop(core);
		copy_store(&snapshot, &path);

		let core = start_core(&path).await;
		let breach = degradation(core.security().await).breach;
		core.execute(&actor(), request(Command::BeginAuditEpoch))
			.await
			.unwrap();
		let recovered = core.security().await;
		let bound = bind(&core).await;

		assert_eq!(
			(
				breach,
				recovered,
				matches!(bound, Ok(CommandOutcome::AccountBound(_)))
			),
			(AuditBreach::HeadNotInStore, SecurityState::Trusted, true)
		);
	}

	/// A retry is not a new mutation. A guarded Command that already committed
	/// replays its recorded outcome even after the audit fell into doubt
	/// (ADR-0093).
	#[tokio::test]
	async fn a_guarded_command_that_already_committed_still_replays() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let recorded = start_core(&path).await;
		let envelope = request(Command::BindAccount {
			provider: ProviderId("anthropic".into()),
			label: "Work account".into(),
			provider_account: None,
			credential_source: CredentialSource::PlatformStore,
		});
		let committed =
			recorded.execute(&actor(), envelope.clone()).await.unwrap();
		recorded.close().await;
		drop(recorded);
		std::fs::remove_file(audit_head_path(&path)).unwrap();
		let core = start_core(&path).await;

		let replayed = core.execute(&actor(), envelope).await;

		assert_eq!(
			(
				matches!(core.security().await, SecurityState::Degraded(_)),
				replayed
			),
			(true, Ok(committed))
		);
	}

	#[tokio::test]
	async fn a_degraded_plane_refuses_auto_continue_policy_changes() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let trusted = start_core(&path).await;
		let binding = crate::test_support::bind_native_account(
			&trusted,
			crate::ProviderId("anthropic".into()),
		)
		.await;
		trusted.close().await;
		drop(trusted);
		std::fs::remove_file(audit_head_path(&path)).unwrap();
		let core = start_core(&path).await;
		let error = core
			.execute(
				&actor(),
				request(Command::SetAutoContinue {
					target: crate::AutoContinueTarget::AccountBinding(binding),
					policy: crate::AutoContinuePolicy::Off,
				}),
			)
			.await
			.unwrap_err();
		assert_eq!(error.code, "security.audit_degraded");
	}
}
