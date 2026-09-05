use std::path::Path;

use jet_store::{AuditBreach, audit_head_path};
use pretty_assertions::assert_eq;

use crate::test_support::{actor, register_repository, request, start_core};
use crate::{
	AuditEpoch, AuditSequence, Command, CommandOutcome, Core, CoreError,
	CredentialSource, ErrorCategory, ProviderId, Query, QueryResult,
	RetentionPolicy, SecurityDegradation, SecurityState, SettingKey,
	SettingScope, SettingValue,
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
		panic!("unexpected result {readable:?}");
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
	let committed = recorded.execute(&actor(), envelope.clone()).await.unwrap();
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
