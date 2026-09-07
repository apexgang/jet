//! Interrupt turn, Stop Run, and request cancellation through the real
//! daemon, Craft, helper, and native processes (ADR-0083, ADR-0095).
#[path = "support/run_fixture.rs"]
mod fixture;
#[path = "support/run_assertions.rs"]
mod run_assertions;
mod support;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use support::{connect, connect_raw, start_jetd};
use uuid::Uuid;

#[tokio::test]
async fn interrupting_a_turn_cancels_it_natively_and_keeps_the_run_working() {
	tokio::time::timeout(Duration::from_secs(30), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
		let daemon = start_jetd(&home).await;
		let owner = Uuid::new_v4();
		let client = connect(&daemon, owner).await;
		let root = support::init_repository(&dir.path().join("repo"));
		std::fs::write(root.join("queue"), "enabled").unwrap();
		let id = local_conversation(&client, &root).await;
		let mut wire = connect_raw(&daemon, owner).await;
		let run_id = start_run(&mut wire, id).await;
		wait_file(&root.join("current-turn"), "initial").await;

		// The next input waits behind the turn that is about to be cancelled.
		wire.send(&submit(id, "Work after the interruption")).await;
		let queued: Value = wire.receive().await;
		let queued_turn = queued["result"]["turn"]["turn_id"].clone();

		// Withdrawal reaches queued input only: the active turn is not its
		// business, and neither is the Run's (ADR-0095).
		let active = active_turn(&client, id).await;
		wire.send(&json!({"kind":"command","id":3,"command_id":Uuid::now_v7(),
			"command":{"type":"withdraw_turn","conversation_id":id,"turn_id":active}}))
			.await;
		let refused: Value = wire.receive().await;
		assert_eq!(
			refused["error"]["code"], "turn.withdraw_denied",
			"{refused}"
		);

		wire.send(&control(run_id, "interrupt_turn")).await;
		let accepted: Value = wire.receive().await;
		assert_eq!(accepted["kind"], "command_result", "{accepted}");
		assert_eq!(accepted["result"]["control"], "interrupt_turn");

		let execution = wait_for_termination(&mut wire, run_id).await;
		assert_eq!(
			execution["termination"],
			json!({"control":"interrupt_turn","stage":"native_cancellation"})
		);
		// The Run itself never stopped, so the queue moves on to the input
		// that was waiting behind the cancelled turn.
		assert_eq!(execution["run"]["lifecycle"], "active");
		wait_file(&root.join("current-turn"), queued_turn.as_str().unwrap())
			.await;
		let queue = client.turn_queue(id).await.unwrap();
		assert_eq!(
			queue
				.turns
				.iter()
				.map(|turn| turn.state)
				.collect::<Vec<_>>(),
			vec![jet_protocol::TurnState::Active]
		);
		// The cancelled turn keeps its own history and its partial changes.
		let diff = client
			.change_diff(run_id, jet_protocol::DiffScope::Turn { turn: 1 })
			.await
			.unwrap();
		assert_eq!(diff.outcome, Some(jet_protocol::TurnOutcome::Interrupted));
		std::fs::remove_file(root.join("queue")).unwrap();
	})
	.await
	.unwrap();
}

#[tokio::test]
async fn stopping_a_run_escalates_to_kill_and_keeps_what_it_produced() {
	tokio::time::timeout(Duration::from_secs(60), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
		let daemon = start_jetd(&home).await;
		let owner = Uuid::new_v4();
		let client = connect(&daemon, owner).await;
		let root = support::init_repository(&dir.path().join("repo"));
		std::fs::write(root.join("deaf"), "ignores signals").unwrap();
		let id = local_conversation(&client, &root).await;
		let mut wire = connect_raw(&daemon, owner).await;
		let run_id = start_run(&mut wire, id).await;
		wait_file(&root.join("deaf-ready"), "").await;

		wire.send(&control(run_id, "stop_run")).await;
		let accepted: Value = wire.receive().await;
		assert_eq!(accepted["kind"], "command_result", "{accepted}");
		assert_eq!(accepted["result"]["control"], "stop_run");
		assert_eq!(accepted["result"]["run"]["lifecycle"], "stopping");

		let execution = wait_for_termination(&mut wire, run_id).await;
		assert_eq!(
			execution["termination"],
			json!({"control":"stop_run","stage":"kill"})
		);
		// Ending on request is not failing, and a killed process reports no
		// status of its own.
		assert_eq!(execution["run"]["lifecycle"], "canceled");
		assert_eq!(execution["exit_code"], Value::Null);
		assert!(
			execution["processes"]
				.as_array()
				.unwrap()
				.iter()
				.all(|process| process["running"] == false),
			"{execution}"
		);
		// Partial output and Workspace changes outlive the stop.
		let events = run_assertions::all_events(&client).await;
		let output: Vec<&jet_protocol::Event> = events
			.events
			.iter()
			.filter(|event| event.kind == "run.output")
			.collect();
		assert!(
			output.iter().any(|event| event.payload["native_json"]
				.as_str()
				.is_some_and(|json| json.contains("Working before the stop"))),
			"{output:?}"
		);
		assert_eq!(
			std::fs::read_to_string(root.join("result.txt")).unwrap(),
			"Harness work\n"
		);
		let control_requested = events
			.events
			.iter()
			.find(|event| event.kind == "run.control_requested")
			.expect("the request itself is journalled");
		assert_eq!(control_requested.payload["control"], "stop_run");
		// A Run that already ended cannot be controlled again.
		wire.send(&control(run_id, "stop_run")).await;
		let refused: Value = wire.receive().await;
		assert_eq!(
			refused["error"]["code"], "run.not_controllable",
			"{refused}"
		);
	})
	.await
	.unwrap();
}

#[tokio::test]
async fn a_query_bound_is_the_clients_own_and_never_binds_a_command() {
	tokio::time::timeout(Duration::from_secs(15), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let daemon = start_jetd(&dir.path().join("jet")).await;
		let owner = Uuid::new_v4();
		let mut wire = connect_raw(&daemon, owner).await;
		for bound in [json!(0), json!(60_001)] {
			wire.send(&json!({"kind":"query","id":1,"query":{"type":"status"},"timeout_ms":bound}))
				.await;
			let refused: Value = wire.receive().await;
			assert_eq!(
				refused["error"]["code"], "request.invalid_timeout",
				"{refused}"
			);
		}
		wire.send(&json!({"kind":"query","id":2,"query":{"type":"status"},"timeout_ms":5000}))
			.await;
		let answered: Value = wire.receive().await;
		assert_eq!(answered["kind"], "query_result", "{answered}");

		// A Command that was durably accepted survives losing the client
		// that asked for it, and its identity still answers with the
		// original receipt (ADR-0093, ADR-0095).
		let command_id = Uuid::now_v7();
		let request = json!({"kind":"command","id":3,"command_id":command_id,
			"command":{"type":"create_conversation","retention":"retain"}});
		wire.send(&request).await;
		drop(wire);
		let client = connect(&daemon, owner).await;
		let conversations = loop {
			let list = client.conversations().await.unwrap();
			if !list.conversations.is_empty() {
				break list;
			}
			tokio::time::sleep(Duration::from_millis(10)).await;
		};
		assert_eq!(conversations.conversations.len(), 1);
		let mut wire = connect_raw(&daemon, owner).await;
		wire.send(&request).await;
		let replayed: Value = wire.receive().await;
		assert_eq!(
			replayed["result"]["conversation_id"],
			json!(conversations.conversations[0].conversation_id)
		);
	})
	.await
	.unwrap();
}

async fn local_conversation(client: &jet_client::Client, root: &Path) -> Uuid {
	let project = client
		.register_project(Uuid::now_v7(), root.to_str().unwrap())
		.await
		.unwrap();
	client
		.create_conversation_in(
			Uuid::now_v7(),
			jet_protocol::RetentionPolicy::Retain,
			jet_protocol::WorkingTreeRequest::LocalCheckout {
				project_id: project.project_id,
			},
		)
		.await
		.unwrap()
		.conversation_id
}

async fn start_run(wire: &mut support::RawConnection, id: Uuid) -> Uuid {
	wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),
		"command":{"type":"start_run","conversation_id":id,"craft":"fake","prompt":"Make a change"}}))
		.await;
	let started: Value = wire.receive().await;
	assert_eq!(started["kind"], "command_result", "{started}");
	Uuid::parse_str(started["result"]["run_id"].as_str().unwrap()).unwrap()
}

fn control(run_id: Uuid, kind: &str) -> Value {
	json!({"kind":"command","id":4,"command_id":Uuid::now_v7(),
		"command":{"type":kind,"run_id":run_id}})
}

fn submit(id: Uuid, prompt: &str) -> Value {
	json!({"kind":"command","id":2,"command_id":Uuid::now_v7(),
		"command":{"type":"submit_turn","conversation_id":id,"prompt":prompt}})
}

async fn active_turn(client: &jet_client::Client, id: Uuid) -> Uuid {
	loop {
		let queue = client.turn_queue(id).await.unwrap();
		if let Some(turn) = queue
			.turns
			.iter()
			.find(|turn| turn.state == jet_protocol::TurnState::Active)
		{
			return turn.turn_id;
		}
		tokio::time::sleep(Duration::from_millis(10)).await;
	}
}

async fn wait_for_termination(
	wire: &mut support::RawConnection,
	run_id: Uuid,
) -> Value {
	loop {
		wire.send(&json!({"kind":"query","id":5,"query":{"type":"run_execution","run_id":run_id}}))
			.await;
		let response: Value = wire.receive().await;
		assert_eq!(response["kind"], "query_result", "{response}");
		if !response["result"]["termination"].is_null() {
			return response["result"].clone();
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
}

async fn wait_file(path: &Path, value: &str) {
	while std::fs::read_to_string(path)
		.ok()
		.is_none_or(|found| !value.is_empty() && found != value)
	{
		tokio::time::sleep(Duration::from_millis(10)).await;
	}
}
