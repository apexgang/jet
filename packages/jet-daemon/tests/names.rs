//! Black-box naming conformance at the public Jet protocol seam.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "support/run_assertions.rs"]
mod assertions;
#[path = "support/run_fixture.rs"]
mod fixture;
mod support;

use jet_protocol::{NameSource, RetentionPolicy, SearchField};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::{connect, connect_raw, init_repository, start_jetd};
use uuid::Uuid;

#[tokio::test]
async fn manual_conversation_and_run_names_are_independent_and_durable() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();

	let mut first = start_jetd(&home).await;
	let client = connect(&first, client_id).await;
	let conversation = client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();
	let run = client
		.create_run(Uuid::now_v7(), conversation.conversation_id)
		.await
		.unwrap();
	assert_eq!(
		(
			&conversation.name.as_ref().unwrap().source,
			&run.name.as_ref().unwrap().source
		),
		(&NameSource::Deterministic, &NameSource::Deterministic)
	);
	assert!(!conversation.name.as_ref().unwrap().value.is_empty());
	assert!(!run.name.as_ref().unwrap().value.is_empty());
	for text in [
		conversation.name.as_ref().unwrap().value.as_str(),
		run.name.as_ref().unwrap().value.as_str(),
	] {
		let search = client.search(text).await.unwrap();
		assert!(search.hits.iter().any(|hit| {
			hit.conversation_id == conversation.conversation_id
				&& hit.field == SearchField::Name
		}));
	}
	let visible_cursor_before_names =
		client.events_after(0).await.unwrap().cursor;

	let named_conversation = client
		.set_conversation_name(
			Uuid::now_v7(),
			conversation.conversation_id,
			conversation.revision.unwrap(),
			"Durable Conversation",
		)
		.await
		.unwrap();
	assert_eq!(
		named_conversation.revision,
		Some(conversation.revision.unwrap() + 1)
	);
	let named_run = client
		.set_run_name(
			Uuid::now_v7(),
			run.run_id,
			run.revision,
			"Independent Run",
		)
		.await
		.unwrap();
	assert_eq!(named_run.revision, run.revision + 1);
	for text in ["Durable Conversation", "Independent Run"] {
		let search = client.search(text).await.unwrap();
		assert!(search.hits.iter().any(|hit| {
			hit.conversation_id == conversation.conversation_id
				&& hit.field == SearchField::Name
		}));
	}
	let stale_conversation = client
		.set_conversation_name(
			Uuid::now_v7(),
			conversation.conversation_id,
			conversation.revision.unwrap(),
			"Stale Conversation",
		)
		.await
		.unwrap_err();
	let stale_run = client
		.set_run_name(Uuid::now_v7(), run.run_id, run.revision, "Stale Run")
		.await
		.unwrap_err();
	for (error, code, revision) in [
		(
			stale_conversation,
			"conversation.revision_conflict",
			named_conversation.revision.unwrap(),
		),
		(stale_run, "run.revision_conflict", named_run.revision),
	] {
		let jet_client::ClientError::Remote(error) = error else {
			panic!("expected a remote Revision conflict")
		};
		assert_eq!(error.code, code);
		assert_eq!(error.revision_conflict.unwrap().current_revision, revision);
	}
	for invalid in [" padded ".into(), "é".repeat(129)] {
		let error = client
			.set_run_name(
				Uuid::now_v7(),
				run.run_id,
				named_run.revision,
				invalid,
			)
			.await
			.unwrap_err();
		let jet_client::ClientError::Remote(error) = error else {
			panic!("expected a core validation error")
		};
		assert_eq!(error.code, "name.invalid");
	}

	let mut legacy_hello = support::hello(client_id);
	legacy_hello.minor = jet_protocol::NAMES_MINOR - 1;
	let (mut legacy, _) = support::handshake_raw(&first, &legacy_hello).await;
	legacy
		.send(&json!({
			"kind":"query",
			"id":1,
			"query":{
				"type":"events",
				"after":visible_cursor_before_names.to_string()
			}
		}))
		.await;
	let hidden_only: Value = legacy.receive().await;
	assert_eq!(hidden_only["result"]["events"], json!([]));
	assert_eq!(
		hidden_only["result"]["cursor"],
		json!(visible_cursor_before_names.to_string())
	);
	legacy
		.send(&json!({
			"kind":"query",
			"id":2,
			"query":{"type":"search","text":"Durable Conversation"}
		}))
		.await;
	let legacy_search: Value = legacy.receive().await;
	assert_eq!(legacy_search["result"]["hits"], json!([]));

	let later_conversation = client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();
	legacy
		.send(&json!({
			"kind":"query",
			"id":3,
			"query":{
				"type":"events",
				"after":visible_cursor_before_names.to_string()
			}
		}))
		.await;
	let visible_after_hidden: Value = legacy.receive().await;
	let visible_events =
		visible_after_hidden["result"]["events"].as_array().unwrap();
	assert_eq!(visible_events.len(), 1);
	assert_eq!(visible_events[0]["kind"], "conversation.created");
	assert_eq!(
		visible_events[0]["conversation_id"],
		later_conversation.conversation_id.to_string()
	);
	assert!(visible_events[0]["payload"].get("name").is_none());

	let before_restart = client
		.conversation(conversation.conversation_id)
		.await
		.unwrap();
	let events = client.events_after(0).await.unwrap();
	first.child.kill().await.unwrap();

	let second = start_jetd(&home).await;
	let after_restart = connect(&second, client_id)
		.await
		.conversation(conversation.conversation_id)
		.await
		.unwrap();

	assert_eq!(after_restart, before_restart);
	assert_eq!(
		(
			&after_restart.conversation.name,
			&after_restart.runs[0].name
		),
		(&named_conversation.name, &named_run.name)
	);
	assert_eq!(
		named_conversation.name.as_ref().unwrap().source,
		NameSource::Manual
	);
	assert_eq!(named_run.name.as_ref().unwrap().source, NameSource::Manual);
	assert_eq!(
		events
			.events
			.iter()
			.filter(|event| event.kind.ends_with(".name_changed"))
			.map(|event| (
				event.kind.as_str(),
				event.payload["name"]["value"].as_str().unwrap(),
				event.payload["name"]["source"].as_str().unwrap(),
			))
			.collect::<Vec<_>>(),
		vec![
			(
				"conversation.name_changed",
				"Durable Conversation",
				"manual"
			),
			("run.name_changed", "Independent Run", "manual"),
		]
	);
}

#[tokio::test]
async fn native_titles_obey_manual_precedence_and_process_isolation() {
	tokio::time::timeout(std::time::Duration::from_secs(30), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
		let mut daemon = start_jetd(&home).await;
		let client_id = Uuid::new_v4();
		let client = connect(&daemon, client_id).await;
		let root = init_repository(&dir.path().join("repo"));
		let project = client
			.register_project(Uuid::now_v7(), root.to_str().unwrap())
			.await
			.unwrap();
		let conversation = client
			.create_conversation_in(
				Uuid::now_v7(),
				RetentionPolicy::Retain,
				jet_protocol::WorkingTreeRequest::Workspace {
					project_id: project.project_id,
					base: jet_protocol::BaseSelection::Head,
					seed: jet_protocol::SeedSelection::None,
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
		std::fs::write(
			std::path::Path::new(&workspace.root).join("name-events"),
			"enabled",
		)
		.unwrap();

		let mut wire = connect_raw(&daemon, client_id).await;
		wire.send(&json!({
			"kind":"command",
			"id":1,
			"command_id":Uuid::now_v7(),
			"command":{
				"type":"start_run",
				"conversation_id":conversation.conversation_id,
				"craft":"fake",
				"prompt":"Make a change"
			}
		}))
		.await;
		let admitted: Value = wire.receive().await;
		assert_eq!(admitted["kind"], "command_result", "{admitted}");
		let run_id =
			Uuid::parse_str(admitted["result"]["run_id"].as_str().unwrap())
				.unwrap();
		let waiting = assertions::wait_for(
			&mut wire,
			&run_id.to_string(),
			"waiting_for_approval",
		)
		.await;
		let first = client
			.conversation(conversation.conversation_id)
			.await
			.unwrap();
		assert_eq!(
			(
				first.conversation.name.as_ref().unwrap().value.as_str(),
				first.conversation.name.as_ref().unwrap().source,
				waiting["run"]["name"]["value"].as_str().unwrap(),
				&waiting["run"]["name"]["source"],
			),
			(
				"Harness Conversation",
				NameSource::HarnessNative,
				"Harness Run",
				&json!("harness_native"),
			)
		);

		client
			.set_conversation_name(
				Uuid::now_v7(),
				conversation.conversation_id,
				first.conversation.revision.unwrap(),
				"Manual Conversation",
			)
			.await
			.unwrap();
		client
			.set_run_name(
				Uuid::now_v7(),
				run_id,
				first.runs[0].revision,
				"Manual Run",
			)
			.await
			.unwrap();
		std::fs::write(
			std::path::Path::new(&workspace.root).join("continue"),
			"go",
		)
		.unwrap();
		let completed =
			assertions::wait_for(&mut wire, &run_id.to_string(), "completed")
				.await;
		let after = client
			.conversation(conversation.conversation_id)
			.await
			.unwrap();
		assert_eq!(
			(
				after.conversation.name.as_ref().unwrap().value.as_str(),
				after.runs[0].name.as_ref().unwrap().value.as_str(),
			),
			("Manual Conversation", "Manual Run")
		);
		let harness = completed["processes"]
			.as_array()
			.unwrap()
			.iter()
			.find(|process| process["role"] == "harness")
			.unwrap();
		let helper = completed["processes"]
			.as_array()
			.unwrap()
			.iter()
			.find(|process| process["role"] == "helper")
			.unwrap();
		assert_eq!(harness["label"], "Late Harness Process");
		assert_eq!(helper["label"], Value::Null);

		let mut legacy_hello = support::hello(client_id);
		legacy_hello.minor = jet_protocol::NAMES_MINOR - 1;
		let (mut legacy, _) =
			support::handshake_raw(&daemon, &legacy_hello).await;
		legacy
			.send(&json!({
				"kind":"query",
				"id":3,
				"query":{
					"type":"conversation",
					"conversation_id":conversation.conversation_id
				}
			}))
			.await;
		let legacy_conversation: Value = legacy.receive().await;
		assert_eq!(
			legacy_conversation["result"]["conversation"]["name"],
			Value::Null
		);
		assert_eq!(
			legacy_conversation["result"]["runs"][0]["name"],
			Value::Null
		);
		legacy
			.send(&json!({
				"kind":"query",
				"id":4,
				"query":{"type":"run_execution","run_id":run_id}
			}))
			.await;
		let legacy_execution: Value = legacy.receive().await;
		assert!(
			legacy_execution["result"]["processes"]
				.as_array()
				.unwrap()
				.iter()
				.all(|process| process.get("label").is_none())
		);
		legacy
			.send(&json!({
				"kind":"query",
				"id":5,
				"query":{"type":"events","after":"0"}
			}))
			.await;
		let legacy_events: Value = legacy.receive().await;
		assert!(
			legacy_events["result"]["events"]
				.as_array()
				.unwrap()
				.iter()
				.all(|event| !event["kind"]
					.as_str()
					.unwrap()
					.ends_with(".name_changed"))
		);
		legacy
			.send(&json!({
				"kind":"command",
				"id":6,
				"command_id":Uuid::now_v7(),
					"command":{
						"type":"set_run_name",
						"run_id":run_id,
						"expected_revision":completed["run"]["revision"],
						"name":"Unsupported"
				}
			}))
			.await;
		let refused: Value = legacy.receive().await;
		assert_eq!(refused["error"]["code"], "protocol.unsupported_minor");

		daemon.child.kill().await.unwrap();
		let restarted = start_jetd(&home).await;
		let durable_client = connect(&restarted, client_id).await;
		let durable = durable_client
			.conversation(conversation.conversation_id)
			.await
			.unwrap();
		assert_eq!(durable, after);
		let mut durable_wire = connect_raw(&restarted, client_id).await;
		durable_wire
			.send(&json!({
				"kind":"query",
				"id":2,
				"query":{"type":"run_execution","run_id":run_id}
			}))
			.await;
		let response: Value = durable_wire.receive().await;
		assert_eq!(response["result"]["processes"], completed["processes"]);
	})
	.await
	.unwrap();
}
