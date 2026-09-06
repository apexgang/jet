use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use crate::capability::{
	CredentialStoreKind, DegradedCondition, ExternalTool, HarnessId,
};
use crate::test_support::{
	FixedProbe, ManualClock, actor, equipped, register_repository, request,
	start_core_with, stripped,
};
use crate::{
	CORE_VERSION, CapabilityObservation, CapabilitySnapshot, Command, Core,
	CoreError, ErrorCategory, Query, QueryResult, ResolvedSetting, SettingKey,
	SettingScope, SettingSelection, SettingSource, SettingValue,
};

/// The one instant every observation in these tests is taken at.
fn observed_at() -> SystemTime {
	UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// What the core reports for a Plane that has everything.
fn equipped_snapshot() -> CapabilitySnapshot {
	let observed = equipped();
	CapabilitySnapshot {
		observed_at: observed_at(),
		core_version: CORE_VERSION,
		platform: observed.platform,
		external_tools: observed.external_tools,
		credential_store: observed.credential_store,
		crafts: observed.crafts,
		harnesses: vec![HarnessId("codex".into())],
		degraded: vec![],
	}
}

/// What it reports once Git, the Craft, and the session bus are gone.
/// Tailscale is missing too, but only some features need it.
fn stripped_snapshot() -> CapabilitySnapshot {
	let observed = stripped();
	CapabilitySnapshot {
		external_tools: observed.external_tools,
		credential_store: observed.credential_store,
		crafts: observed.crafts,
		harnesses: vec![],
		degraded: vec![
			DegradedCondition::MissingExternalTool {
				tool: ExternalTool::Git,
			},
			DegradedCondition::NoHarnessAvailable,
			DegradedCondition::CredentialStoreUnavailable {
				kind: CredentialStoreKind::SecretService,
			},
		],
		..equipped_snapshot()
	}
}

async fn start(dir: &TempDir, probe: Arc<FixedProbe>) -> Core {
	start_core_with(
		&dir.path().join("plane.sqlite3"),
		ManualClock::at(observed_at()),
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

	let at_startup =
		capabilities(&core, CapabilityObservation::LastObserved).await;
	probe.answer_with(stripped());
	let after_losing_them =
		capabilities(&core, CapabilityObservation::Fresh).await;

	assert_eq!(
		(at_startup, after_losing_them),
		(equipped_snapshot(), stripped_snapshot())
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
		(stale, fresh, kept),
		(
			equipped_snapshot(),
			stripped_snapshot(),
			stripped_snapshot()
		)
	);
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
		project_id: register_repository(&core, &dir.path().join("repo")).await,
	};
	let observed = capabilities(&core, CapabilityObservation::Fresh).await;
	probe.answer_with(stripped());

	let refused = enable_auto_commit(&core, scope).await.unwrap_err();
	let unchanged = auto_commit(&core, scope).await;
	probe.answer_with(equipped());
	let accepted = enable_auto_commit(&core, scope).await;
	let stored = auto_commit(&core, scope).await;

	assert_eq!(
		(observed, refused, unchanged, accepted, stored),
		(
			equipped_snapshot(),
			CoreError {
				category: ErrorCategory::Unavailable,
				code: "capability.unavailable".into(),
				retryable: false,
				message: "this Plane cannot use the git command-line tool \
				          right now"
					.into(),
				detail: None,
				revision_conflict: None,
				recovery_actions: vec![],
			},
			ResolvedSetting {
				key: SettingKey::GitAutoCommit,
				value: SettingValue::Flag(false),
				source: SettingSource::BuiltIn,
			},
			Ok(()),
			ResolvedSetting {
				key: SettingKey::GitAutoCommit,
				value: SettingValue::Flag(true),
				source: SettingSource::Scope(scope),
			},
		)
	);
}
