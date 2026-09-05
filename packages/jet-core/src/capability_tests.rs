use std::sync::Arc;

use pretty_assertions::assert_eq;
use tempfile::TempDir;
use uuid::Uuid;

use crate::capability::{
	CredentialStoreKind, DegradedCondition, ExternalTool, HarnessId, Platform,
};
use crate::clock::SystemClock;
use crate::test_support::{
	FixedProbe, actor, equipped, request, start_core_with, stripped,
};
use crate::{
	CapabilityObservation, CapabilitySnapshot, Command, Core, CoreError,
	ErrorCategory, ProjectId, Query, QueryResult, ResolvedSetting, SettingKey,
	SettingScope, SettingSelection, SettingSource, SettingValue,
};

async fn start(dir: &TempDir, probe: Arc<FixedProbe>) -> Core {
	start_core_with(
		&dir.path().join("plane.sqlite3"),
		Arc::new(SystemClock),
		probe,
	)
	.await
}

async fn capabilities(
	core: &Core,
	observation: CapabilityObservation,
) -> CapabilitySnapshot {
	let result = core
		.query(&actor(), Query::Capabilities { observation })
		.await
		.unwrap();
	let QueryResult::Capabilities(snapshot) = result else {
		panic!("unexpected result {result:?}");
	};
	snapshot
}

async fn auto_commit(core: &Core, scope: SettingScope) -> ResolvedSetting {
	let result = core
		.query(
			&actor(),
			Query::Settings {
				scope,
				selection: SettingSelection::Key(SettingKey::GitAutoCommit),
			},
		)
		.await
		.unwrap();
	let QueryResult::Settings(mut snapshot) = result else {
		panic!("unexpected result {result:?}");
	};
	snapshot.settings.remove(0)
}

async fn enable_auto_commit(
	core: &Core,
	scope: SettingScope,
) -> Result<(), CoreError> {
	core.execute(
		&actor(),
		request(Command::SetSetting {
			key: SettingKey::GitAutoCommit,
			scope,
			value: SettingValue::Flag(true),
		}),
	)
	.await
	.map(|_| ())
}

#[tokio::test]
async fn a_snapshot_reports_the_plane_and_what_leaves_it_degraded() {
	let dir = tempfile::tempdir().unwrap();
	let probe = FixedProbe::new(equipped());
	let core = start(&dir, Arc::clone(&probe)).await;

	let equipped_snapshot =
		capabilities(&core, CapabilityObservation::LastObserved).await;
	probe.answer_with(stripped());
	let stripped_snapshot =
		capabilities(&core, CapabilityObservation::Fresh).await;

	assert_eq!(
		(
			equipped_snapshot.harnesses,
			equipped_snapshot.degraded,
			equipped_snapshot.platform,
			equipped_snapshot.core_version,
			stripped_snapshot.harnesses,
			stripped_snapshot.degraded,
		),
		(
			vec![HarnessId("codex".into())],
			vec![],
			Platform {
				operating_system: "linux",
				architecture: "aarch64"
			},
			crate::CORE_VERSION,
			vec![],
			vec![
				// Tailscale is missing too, but only some features need it.
				DegradedCondition::MissingExternalTool {
					tool: ExternalTool::Git
				},
				DegradedCondition::NoHarnessAvailable,
				DegradedCondition::CredentialStoreUnavailable {
					kind: CredentialStoreKind::SecretService
				},
			],
		)
	);
}

/// ADR-0086 keeps `jetd` from polling, so a client reads the last
/// observation until something asks the Plane again.
#[tokio::test]
async fn the_last_observation_stands_until_the_plane_is_observed_again() {
	let dir = tempfile::tempdir().unwrap();
	let probe = FixedProbe::new(equipped());
	let core = start(&dir, Arc::clone(&probe)).await;
	probe.answer_with(stripped());

	let stale = capabilities(&core, CapabilityObservation::LastObserved).await;
	let fresh = capabilities(&core, CapabilityObservation::Fresh).await;
	let kept = capabilities(&core, CapabilityObservation::LastObserved).await;

	assert_eq!(
		(stale.degraded.is_empty(), fresh.degraded, kept.degraded),
		(true, stripped_degraded(), stripped_degraded())
	);
}

fn stripped_degraded() -> Vec<DegradedCondition> {
	vec![
		DegradedCondition::MissingExternalTool {
			tool: ExternalTool::Git,
		},
		DegradedCondition::NoHarnessAvailable,
		DegradedCondition::CredentialStoreUnavailable {
			kind: CredentialStoreKind::SecretService,
		},
	]
}

/// A client prepares a Command against the Capabilities it just read. The
/// Plane may change before the Command runs, so the Command is answered by
/// what the Plane can do now, and it changes nothing when the answer is no
/// (ADR-0086).
#[tokio::test]
async fn a_command_whose_capability_disappeared_commits_nothing() {
	let dir = tempfile::tempdir().unwrap();
	let probe = FixedProbe::new(equipped());
	let core = start(&dir, Arc::clone(&probe)).await;
	let scope = SettingScope::Project {
		project_id: ProjectId(Uuid::now_v7()),
	};
	let observed = capabilities(&core, CapabilityObservation::Fresh).await;
	probe.answer_with(stripped());

	let refused = enable_auto_commit(&core, scope).await.unwrap_err();
	let unchanged = auto_commit(&core, scope).await;
	probe.answer_with(equipped());
	let accepted = enable_auto_commit(&core, scope).await;
	let stored = auto_commit(&core, scope).await;

	assert_eq!(
		(
			observed.degraded.is_empty(),
			(refused.category, refused.code.as_str(), refused.retryable),
			refused.message.as_str(),
			unchanged.value,
			unchanged.source,
			accepted,
			stored.value,
			stored.source,
		),
		(
			true,
			(ErrorCategory::Unavailable, "capability.unavailable", false),
			"this Plane cannot use the git command-line tool right now",
			SettingValue::Flag(false),
			SettingSource::BuiltIn,
			Ok(()),
			SettingValue::Flag(true),
			SettingSource::Scope(scope),
		)
	);
}
