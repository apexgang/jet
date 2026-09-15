//! Black-box Usage record conformance at the public Jet protocol seam.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "support/run_fixture.rs"]
mod fixture;
mod support;

use std::path::Path;
use std::time::Duration;

use jet_protocol::{
	BaseSelection, PlaneUsage, QuotaScope, QuotaUnit, RetentionPolicy,
	SeedSelection, UsageEstimation, UsageFinality, UsageFreshness,
	UsageHistory, UsageHistoryRange, UsageHistorySelection, UsageResolution,
	UsageSelection, VisaRunRequest, WorkingTreeRequest,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::{connect, init_repository, start_jetd_with_credential_store};
use uuid::Uuid;

/// The bundled fake Craft, declared at the minor that reports Usage and
/// under a Harness whose native Provider a Visa Run can select.
fn install(home: &Path) {
	fixture::install_at_minor(home, 7);
	let path = home.join("crafts/fake.json");
	let mut manifest: Value =
		serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
	manifest["specification"]["harness"] = json!("codex");
	std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

/// Reads the Plane's Usage until the Harness's report has been committed,
/// so the test waits for durable state rather than for a duration.
async fn recorded(client: &jet_client::Client) -> PlaneUsage {
	for _ in 0..300 {
		let usage = client.usage(UsageSelection::Plane).await.unwrap();
		if usage.consumption.measurements > 0 && !usage.quota_windows.is_empty()
		{
			return *usage;
		}
		tokio::time::sleep(Duration::from_millis(50)).await;
	}
	panic!("the Plane never recorded the Harness's Usage report")
}

/// Reads the last day of the Plane's Usage history until the maintenance
/// sweep has counted the Harness's report into its hour.
async fn aggregated(
	client: &jet_client::Client,
	observed_at_unix_ms: i64,
) -> UsageHistory {
	for _ in 0..300 {
		let history = client
			.usage_history(
				UsageHistorySelection::Plane,
				UsageHistoryRange {
					from_unix_ms: observed_at_unix_ms - 86_400_000,
					until_unix_ms: observed_at_unix_ms + 86_400_000,
				},
				UsageResolution::Hour,
			)
			.await
			.unwrap();
		if !history.series.is_empty() {
			return *history;
		}
		tokio::time::sleep(Duration::from_millis(50)).await;
	}
	panic!("the Plane never aggregated the Harness's Usage report")
}

/// What a Harness reports about its own consumption and about its
/// Provider's quota windows becomes durable Plane state, attributed to the
/// Account binding its Run selected, and read back with the freshness the
/// Plane can vouch for (ADR-0023).
#[tokio::test]
async fn a_harness_report_becomes_a_queryable_usage_record() {
	tokio::time::timeout(Duration::from_secs(60), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		install(&home);
		let daemon = start_jetd_with_credential_store(&home).await;
		let client = connect(&daemon, Uuid::new_v4()).await;
		let binding = client
			.bind_account(
				Uuid::now_v7(),
				"openai",
				"Destination",
				None,
				jet_protocol::CredentialSource::HarnessNative,
			)
			.await
			.unwrap();
		let project = client
			.register_project(
				Uuid::now_v7(),
				init_repository(&dir.path().join("repo")).to_str().unwrap(),
			)
			.await
			.unwrap();
		let conversation = client
			.create_conversation_in(
				Uuid::now_v7(),
				RetentionPolicy::Retain,
				WorkingTreeRequest::Workspace {
					project_id: project.project_id,
					base: BaseSelection::Head,
					seed: SeedSelection::None,
				},
			)
			.await
			.unwrap();
		let workspace = client
			.conversation(conversation.conversation_id)
			.await
			.unwrap()
			.workspace
			.unwrap();
		let root = std::path::PathBuf::from(&workspace.root);
		std::fs::write(root.join("usage-events"), "enabled").unwrap();

		let run = client
			.start_visa_run(
				Uuid::now_v7(),
				VisaRunRequest {
					conversation_id: conversation.conversation_id,
					destination_plane_id: client
						.status()
						.await
						.unwrap()
						.plane_id,
					account_binding_id: binding.binding_id,
					craft: "fake".into(),
					prompt: "Make a change".into(),
				},
			)
			.await
			.unwrap();
		let usage = recorded(&client).await;
		std::fs::write(root.join("continue"), "go").unwrap();

		let window = usage.quota_windows.first().expect("one window");
		assert_eq!(
			(
				usage.plane_id,
				usage.consumption.tokens,
				usage.consumption.measurements,
				usage.consumption.estimated,
				usage.consumption.interim,
				usage
					.consumption
					.models
					.first()
					.and_then(|model| model.model.clone()),
			),
			(
				client.status().await.unwrap().plane_id,
				jet_protocol::UsageTokens {
					input: 120,
					cached_input: 30,
					output: 45,
					reasoning: 0,
				},
				1,
				0,
				0,
				Some("fake-model".into()),
			)
		);
		assert_eq!(
			(
				window.binding_id,
				window.provider.as_str(),
				window.window.as_str(),
				window.scope.clone(),
				window.measure.unit,
				window.measure.used,
				window.measure.limit,
				window.window_seconds,
				window.estimation,
				window.finality,
				window.freshness.clone(),
			),
			(
				binding.binding_id,
				"openai",
				"five_hour",
				QuotaScope::ProviderAccount,
				QuotaUnit::Share,
				4_200,
				Some(10_000),
				Some(18_000),
				UsageEstimation::Measured,
				UsageFinality::Interim,
				UsageFreshness::Fresh,
			)
		);
		// The window's reset is a time this Plane read from its own clock,
		// never one the Craft asserted.
		assert!(window.resets_at_unix_ms.unwrap() > window.observed_at_unix_ms);
		// One Run's consumption is the same consumption, read through the
		// Conversation that owns it; a Conversation leaves the account's
		// windows to the account.
		let conversation_usage = client
			.usage(UsageSelection::Conversation {
				conversation_id: conversation.conversation_id,
			})
			.await
			.unwrap();
		let run_usage = client
			.usage(UsageSelection::Run { run_id: run.run_id })
			.await
			.unwrap();
		assert_eq!(
			(
				conversation_usage.consumption.clone(),
				conversation_usage.quota_windows.clone(),
				run_usage.consumption.clone(),
			),
			(usage.consumption.clone(), vec![], usage.consumption.clone())
		);
		// The maintenance loop counts the same measurement into its hour,
		// and the history Query answers it at the resolution it asked for
		// (ADR-0045).
		let history = aggregated(
			&client,
			usage
				.consumption
				.last_observed_at_unix_ms
				.expect("observed"),
		)
		.await;
		let point = history
			.series
			.first()
			.and_then(|series| series.points.first())
			.expect("one point");
		assert_eq!(
			(
				history.plane_id,
				history.resolution,
				history.series.len(),
				history.series[0].model.clone(),
				history.series[0].points.len(),
				point.tokens,
				point.measurements,
				point.start_unix_ms % 3_600_000,
			),
			(
				usage.plane_id,
				UsageResolution::Hour,
				1,
				Some("fake-model".into()),
				1,
				usage.consumption.tokens,
				1,
				0,
			)
		);
	})
	.await
	.expect("the Plane records and answers within the bound");
}

/// A client that negotiated a minor without Usage records is answered with
/// a stable refusal rather than a snapshot it cannot read (ADR-0019).
#[tokio::test]
async fn a_client_below_the_usage_minor_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();
	let daemon = start_jetd_with_credential_store(&home).await;
	let mut older = support::hello(client_id);
	older.minor = jet_protocol::USAGE_RECORDS_MINOR - 1;
	let (mut connection, _) = support::handshake_raw(&daemon, &older).await;
	connection
		.send(&json!({
			"kind":"query",
			"id":1,
			"query":{"type":"usage","selection":{"scope":"plane"}}
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
					"the Usage Query needs protocol minor {}",
					jet_protocol::USAGE_RECORDS_MINOR
				)
				.as_str()
			)
		)
	);
}

/// A client that negotiated a minor without Usage history is answered with
/// a stable refusal rather than a series it cannot read (ADR-0019).
#[tokio::test]
async fn a_client_below_the_usage_history_minor_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();
	let daemon = start_jetd_with_credential_store(&home).await;
	let mut older = support::hello(client_id);
	older.minor = jet_protocol::USAGE_HISTORY_MINOR - 1;
	let (mut connection, _) = support::handshake_raw(&daemon, &older).await;
	connection
		.send(&json!({
			"kind":"query",
			"id":1,
			"query":{
				"type":"usage_history",
				"selection":{"scope":"plane"},
				"range":{"from_unix_ms":0,"until_unix_ms":1},
				"resolution":"day"
			}
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
					"the Usage history Query needs protocol minor {}",
					jet_protocol::USAGE_HISTORY_MINOR
				)
				.as_str()
			)
		)
	);
}
