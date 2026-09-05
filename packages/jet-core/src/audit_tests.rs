use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::test_support::{
	FixedProbe, ManualClock, actor, equipped, request, start_core_with,
};
use crate::{
	AuditDecision, AuditEntry, AuditEpoch, AuditOutcome, AuditPage, AuditRisk,
	AuditSequence, AuditTarget, ClientId, Command, CommandOutcome, Core,
	CredentialSource, PlaneId, ProjectId, Query, QueryResult, SettingKey,
	SettingScope, SettingValue,
};

/// A fixed instant, so a recorded decision has an exact time rather than
/// whatever the machine's clock said.
const NOW: Duration = Duration::from_millis(1_700_000_000_000);

async fn start(dir: &tempfile::TempDir) -> Core {
	start_core_with(
		&dir.path().join("plane.sqlite3"),
		ManualClock::at(UNIX_EPOCH + NOW),
		FixedProbe::new(equipped()),
	)
	.await
}

async fn audit(core: &Core, after: AuditSequence) -> AuditPage {
	let result = core
		.query(&actor(), Query::SecurityAudit { after })
		.await
		.unwrap();
	let QueryResult::SecurityAudit(page) = result else {
		panic!("unexpected result {result:?}");
	};
	page
}

/// What each recorded decision says, without the identities a test cannot
/// predict.
fn decisions(page: &AuditPage) -> Vec<(&str, AuditRisk, AuditOutcome)> {
	page.entries
		.iter()
		.map(|entry| (entry.decision.as_str(), entry.risk, entry.outcome))
		.collect()
}

async fn bind(core: &Core, label: &str) -> Uuid {
	let outcome = core
		.execute(
			&actor(),
			request(Command::BindAccount {
				provider: crate::ProviderId("anthropic".into()),
				label: label.into(),
				provider_account: None,
				credential_source: CredentialSource::PlatformStore,
			}),
		)
		.await
		.unwrap();
	let CommandOutcome::AccountBound(binding) = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	binding.binding_id.0
}

#[tokio::test]
async fn binding_an_account_is_recorded_as_an_elevated_decision() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let plane_id = match core.query(&actor(), Query::Status).await.unwrap() {
		QueryResult::Status(status) => status.plane_id,
		result => panic!("unexpected result {result:?}"),
	};

	let binding_id = bind(&core, "Work account").await;

	let page = audit(&core, AuditSequence(0)).await;
	let [entry] = page.entries.as_slice() else {
		panic!("unexpected audit {page:?}");
	};
	assert_eq!(
		page,
		AuditPage {
			cursor: AuditSequence(1),
			entries: vec![AuditEntry {
				sequence: AuditSequence(1),
				epoch: AuditEpoch(1),
				record_id: entry.record_id,
				recorded_at: UNIX_EPOCH + NOW,
				plane_id: PlaneId(plane_id.0),
				actor: actor(),
				target: AuditTarget {
					kind: "account_binding".into(),
					reference: entry.target.reference,
					identity: Some(binding_id.to_string()),
				},
				decision: AuditDecision::AccountBound.as_str().into(),
				risk: AuditRisk::Elevated,
				outcome: AuditOutcome::Succeeded,
			}],
		}
	);
}

#[tokio::test]
async fn decisions_about_one_binding_share_its_opaque_reference() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;

	let kept = bind(&core, "Kept").await;
	let removed = bind(&core, "Removed").await;
	core.execute(
		&actor(),
		request(Command::UnbindAccount {
			binding_id: crate::AccountBindingId(removed),
		}),
	)
	.await
	.unwrap();

	let page = audit(&core, AuditSequence(0)).await;
	let [kept_bound, removed_bound, removed_unbound] = page.entries.as_slice()
	else {
		panic!("unexpected audit {page:?}");
	};
	assert_eq!(
		(
			decisions(&page),
			removed_bound.target.reference == removed_unbound.target.reference,
			kept_bound.target.reference == removed_bound.target.reference,
			kept.to_string() == kept_bound.target.identity.clone().unwrap(),
		),
		(
			vec![
				(
					"account.bound",
					AuditRisk::Elevated,
					AuditOutcome::Succeeded
				),
				(
					"account.bound",
					AuditRisk::Elevated,
					AuditOutcome::Succeeded
				),
				(
					"account.unbound",
					AuditRisk::Elevated,
					AuditOutcome::Succeeded
				),
			],
			true,
			false,
			true,
		)
	);
}

#[tokio::test]
async fn git_automation_is_a_policy_decision_and_naming_is_a_preference() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let project = SettingScope::Project {
		project_id: ProjectId(Uuid::now_v7()),
	};

	for command in [
		Command::SetSetting {
			key: SettingKey::GitAutoCommit,
			scope: project,
			value: SettingValue::Flag(true),
		},
		Command::SetSetting {
			key: SettingKey::UtilityAutomaticNaming,
			scope: project,
			value: SettingValue::Flag(false),
		},
		Command::SetSetting {
			key: SettingKey::GitAutoCommit,
			scope: project,
			value: SettingValue::Flag(false),
		},
		Command::ClearSetting {
			key: SettingKey::GitAutoCommit,
			scope: project,
		},
	] {
		core.execute(&actor(), request(command)).await.unwrap();
	}

	let page = audit(&core, AuditSequence(0)).await;
	assert_eq!(
		(decisions(&page), page.entries[0].target.kind.as_str()),
		(
			vec![
				(
					"policy.git_automation_enabled",
					AuditRisk::Elevated,
					AuditOutcome::Succeeded
				),
				(
					"policy.git_automation_disabled",
					AuditRisk::Routine,
					AuditOutcome::Succeeded
				),
				(
					"policy.git_automation_cleared",
					AuditRisk::Elevated,
					AuditOutcome::Succeeded
				),
			],
			"project"
		)
	);
}

#[tokio::test]
async fn the_audit_keeps_no_text_a_client_supplied() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let secret_looking_label = "sk-live-not-actually-a-secret";

	bind(&core, secret_looking_label).await;

	let page = audit(&core, AuditSequence(0)).await;
	let recorded = format!("{page:?}");
	assert_eq!(recorded.contains(secret_looking_label), false);
}

#[tokio::test]
async fn an_audit_page_resumes_after_the_position_it_is_given() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;

	bind(&core, "First").await;
	bind(&core, "Second").await;

	let page = audit(&core, AuditSequence(1)).await;
	assert_eq!(
		(page.cursor, page.entries.len(), page.entries[0].sequence),
		(AuditSequence(2), 1, AuditSequence(2))
	);
}

#[tokio::test]
async fn an_actor_is_recorded_with_the_decision_it_made() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let client = crate::Actor::InteractiveClient {
		client_id: ClientId(Uuid::nil()),
	};

	bind(&core, "Work account").await;

	let page = audit(&core, AuditSequence(0)).await;
	assert_eq!(page.entries[0].actor, client);
}

/// The clock a Command reads once is the clock the audit records, so a
/// decision and the Event beside it never disagree about when they
/// happened.
#[tokio::test]
async fn a_decision_is_recorded_at_the_time_its_command_read() {
	let dir = tempfile::tempdir().unwrap();
	let clock = ManualClock::at(UNIX_EPOCH + NOW);
	let core = start_core_with(
		&dir.path().join("plane.sqlite3"),
		Arc::clone(&clock) as Arc<dyn crate::clock::Clock>,
		FixedProbe::new(equipped()),
	)
	.await;

	bind(&core, "First").await;
	clock.advance(Duration::from_secs(60));
	bind(&core, "Second").await;

	let page = audit(&core, AuditSequence(0)).await;
	assert_eq!(
		page.entries
			.iter()
			.map(|entry| entry.recorded_at)
			.collect::<Vec<SystemTime>>(),
		vec![UNIX_EPOCH + NOW, UNIX_EPOCH + NOW + Duration::from_secs(60)]
	);
}
