//! Black-box Capability conformance tests at the public Jet protocol
//! boundary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_client::ClientError;
use jet_protocol::{
	CapabilityObservation, DegradedCondition, ErrorCategory, ExternalTool,
	Platform, SettingKey, SettingScope, SettingValue, ToolAvailability,
};
use pretty_assertions::assert_eq;
use support::{connect, start_jetd, start_jetd_without_external_tools};
use uuid::Uuid;

#[tokio::test]
async fn a_plane_reports_the_machine_it_runs_on() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;

	let observed = client
		.capabilities(CapabilityObservation::Fresh)
		.await
		.unwrap();
	let last = client
		.capabilities(CapabilityObservation::LastObserved)
		.await
		.unwrap();

	assert_eq!(
		(
			observed.platform.clone(),
			observed
				.external_tools
				.iter()
				.map(|status| status.tool)
				.collect::<Vec<_>>(),
			observed.crafts.clone(),
			observed.harnesses.clone(),
			observed
				.degraded
				.contains(&DegradedCondition::NoHarnessAvailable),
			last == observed,
		),
		(
			Platform {
				operating_system: std::env::consts::OS.into(),
				architecture: std::env::consts::ARCH.into(),
			},
			vec![
				ExternalTool::Git,
				ExternalTool::Ssh,
				ExternalTool::Tailscale
			],
			// Craft discovery arrives with the Craft issues, so a Plane
			// reports none and says plainly that it can run no Harness.
			vec![],
			vec![],
			true,
			true,
		)
	);
}

/// ADR-0086 has every Command revalidate what it depends on before it
/// commits, so turning on automatic Git delivery fails safely on a Plane
/// that has no Git.
#[tokio::test]
async fn a_command_that_needs_a_missing_tool_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let daemon =
		start_jetd_without_external_tools(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let scope = SettingScope::Project {
		project_id: Uuid::now_v7(),
	};

	let refused = client
		.set_setting(
			Uuid::now_v7(),
			SettingKey::GitAutoCommit,
			scope,
			SettingValue::Flag(true),
		)
		.await
		.unwrap_err();
	let observed = client
		.capabilities(CapabilityObservation::LastObserved)
		.await
		.unwrap();

	let ClientError::Remote(error) = refused else {
		panic!("expected a stable remote error, got {refused:?}");
	};
	assert_eq!(
		(
			error.category,
			error.code.as_str(),
			error.message.as_str(),
			observed
				.external_tools
				.iter()
				.map(|status| status.availability.clone())
				.collect::<Vec<_>>(),
			observed.degraded.contains(
				&DegradedCondition::MissingExternalTool {
					tool: ExternalTool::Git
				}
			),
		),
		(
			ErrorCategory::Unavailable,
			"capability.unavailable",
			"this Plane cannot use the git command-line tool right now",
			vec![ToolAvailability::Missing; 3],
			true,
		)
	);
}
