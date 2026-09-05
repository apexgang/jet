use std::path::Path;

use jet_store::{AuditBreach, audit_head_path};
use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::test_support::{actor, request, start_core};
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
	let core = start_without_a_head(&path).await;
	let project = SettingScope::Project {
		project_id: crate::ProjectId(Uuid::now_v7()),
	};

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
