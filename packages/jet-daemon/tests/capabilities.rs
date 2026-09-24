//! Black-box Capability conformance tests at the public Jet protocol
//! boundary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_client::ClientError;
use jet_protocol::{
	CapabilityObservation, CapabilitySnapshot, DegradedCondition,
	ErrorCategory, ExternalTool, Platform, SettingKey, SettingScope,
	SettingValue, ToolAvailability, WireError,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::{connect, start_jetd, start_jetd_without_external_tools};
use uuid::Uuid;

#[tokio::test]
async fn a_plane_reports_the_machine_it_runs_on() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;

	// ADR-0086 has the Plane report at startup as well as on demand, so the
	// line that says jetd can serve already carries a whole snapshot.
	let at_startup: CapabilitySnapshot =
		serde_json::from_value(daemon.ready["capabilities"].clone()).unwrap();
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
			at_startup.platform == observed.platform,
			at_startup.external_tools == observed.external_tools,
		),
		(
			Platform {
				operating_system: std::env::consts::OS.into(),
				architecture: std::env::consts::ARCH.into(),
			},
			vec![
				ExternalTool::Git,
				ExternalTool::GitLfs,
				ExternalTool::Ssh,
				ExternalTool::Tailscale
			],
			// A fresh Plane has installed no Craft, so it says plainly that
			// it can run no Harness.
			vec![],
			vec![],
			true,
			true,
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
	// No Project can be registered without Git, and none needs to be: a
	// Command is answered by what the Plane can do before it reaches the
	// scope it names, so the refusal is the Capability's and not the
	// unknown Project's.
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
			error,
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
			WireError {
				category: ErrorCategory::Unavailable,
				code: "capability.unavailable".into(),
				retryable: false,
				message: "this Plane cannot use the git command-line tool \
				          right now"
					.into(),
				revision_conflict: None,
				restart: None,
				recovery_actions: vec![],
			},
			vec![ToolAvailability::Missing; 4],
			true,
		)
	);
}

/// ADR-0076 requires one create/read/delete round trip before durable
/// Pairing is enabled, so a client can ask the Plane for exactly that and
/// read what the Plane saw. The store here is the test's own Secret
/// Service; macOS would speak to the Keychain of the machine running the
/// test.
#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn a_client_can_have_the_credential_store_verified() {
	use jet_protocol::{
		CredentialStoreKind, CredentialStoreStatus, CredentialStoreVerification,
	};
	use support::start_jetd_with_credential_store;

	let dir = tempfile::tempdir().unwrap();
	let daemon =
		start_jetd_with_credential_store(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;

	let verified = client.verify_credential_store().await.unwrap();
	let observed = client
		.capabilities(CapabilityObservation::Fresh)
		.await
		.unwrap()
		.credential_store;

	assert_eq!(
		(verified, observed),
		(
			CredentialStoreVerification::Verified {
				kind: CredentialStoreKind::SecretService
			},
			CredentialStoreStatus::Available {
				kind: CredentialStoreKind::SecretService
			}
		)
	);
}

/// A client that negotiated a minor without the verification Query is
/// answered with a stable refusal rather than a probe it did not ask for
/// (ADR-0019).
#[tokio::test]
async fn a_client_below_the_verification_minor_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut older = support::hello(Uuid::new_v4());
	older.minor = jet_protocol::CREDENTIAL_STORE_VERIFICATION_MINOR - 1;
	let (mut connection, _) = support::handshake_raw(&daemon, &older).await;

	connection
		.send(&json!({
			"kind":"query",
			"id":1,
			"query":{"type":"verify_credential_store"}
		}))
		.await;
	let refused: Value = connection.receive().await;

	assert_eq!(
		(
			refused["error"]["code"].as_str(),
			refused["error"]["message"].as_str()
		),
		(
			Some("protocol.unsupported_minor"),
			Some(
				format!(
					"the credential store verification Query needs protocol \
					 minor {}",
					jet_protocol::CREDENTIAL_STORE_VERIFICATION_MINOR
				)
				.as_str()
			)
		)
	);
}
