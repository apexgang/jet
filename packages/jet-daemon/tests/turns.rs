//! Turn queue conformance through authenticated public Commands and Queries.
#[path = "support/run_fixture.rs"]
mod fixture;
mod support;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::{connect, connect_raw, start_jetd};
use uuid::Uuid;

#[tokio::test]
async fn turns_execute_one_at_a_time_in_admission_order_across_restart() {
	tokio::time::timeout(std::time::Duration::from_secs(30), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
		let mut daemon = start_jetd(&home).await;
		let owner = Uuid::new_v4();
		let client = connect(&daemon, owner).await;
		let root = support::init_repository(&dir.path().join("repo"));
		std::fs::write(root.join("queue"), "enabled").unwrap();
		let project = client.register_project(Uuid::now_v7(), root.to_str().unwrap()).await.unwrap();
		let conversation = client.create_conversation_in(Uuid::now_v7(), jet_protocol::RetentionPolicy::Retain, jet_protocol::WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap();
		let id = conversation.conversation_id;
		let mut wire = connect_raw(&daemon, owner).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":id,"craft":"fake","prompt":"Make a change"}})).await;
		let started: Value = wire.receive().await;
		assert_eq!(started["kind"], "command_result", "{started}");
		wait_file(&root.join("current-turn"), "initial").await;
		let query = json!({"kind":"query","id":2,"query":{"type":"turn_queue","conversation_id":id}});
		wire.send(&query).await;
		let queue: Value = wire.receive().await;
		assert_eq!(queue["result"]["turns"].as_array().unwrap().len(), 1);
		let mut expected = Vec::new();
		for (source, prompt) in [("user", "One"), ("schedule", "Old"), ("user", "Two"), ("schedule", "Newest")] {
			wire.send(&submit(id, source, prompt)).await;
			let reply: Value = wire.receive().await;
			assert_eq!(reply["kind"], "command_result", "{reply}");
			if prompt != "Old" { expected.push(json!({"id":reply["result"]["turn"]["turn_id"],"text":prompt})); }
		}
		assert!(!root.join("delivered-turns").exists());
		daemon.child.kill().await.unwrap();
		let daemon = start_jetd(&home).await;
		let checkpoint_client = connect(&daemon, owner).await;
		let run_id = Uuid::parse_str(started["result"]["run_id"].as_str().unwrap()).unwrap();
		let mut previous = "initial".to_string();
		for (index, turn) in expected.iter().enumerate() {
			std::fs::write(root.join("README.md"), format!("Finished {previous}\n")).unwrap();
			std::fs::write(root.join(format!("continue-{previous}")), "go").unwrap();
			let next = turn["id"].as_str().unwrap();
			wait_file(&root.join("current-turn"), next).await;
			let diff = checkpoint_client.change_diff(run_id, jet_protocol::DiffScope::Turn { turn: index as u32 + 1 }).await.unwrap();
			assert_eq!(diff.outcome, Some(jet_protocol::TurnOutcome::Completed));
			assert!(diff.patch.contains(&format!("+Finished {previous}\n")), "{}", diff.patch);
			previous = next.into();
		}
		let delivered: Vec<Value> = std::fs::read_to_string(root.join("delivered-turns")).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
		assert_eq!(delivered, expected);
		let mut wire = connect_raw(&daemon, owner).await;
		wire.send(&submit(id, "user", "Must stay queued")).await;
		let admitted: Value = wire.receive().await;
		assert_eq!(admitted["kind"], "command_result");
		std::fs::write(root.join(format!("continue-{previous}")), "wrong").unwrap();
		let client = connect(&daemon, owner).await;
		loop {
			let queue = client.turn_queue(id).await.unwrap();
			if queue.turns[0].state == jet_protocol::TurnState::OutcomeUnknown {
				assert_eq!(queue.turns.iter().map(|turn| turn.state).collect::<Vec<_>>(), vec![jet_protocol::TurnState::OutcomeUnknown, jet_protocol::TurnState::Queued]);
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
		assert_eq!(std::fs::read_to_string(root.join("delivered-turns")).unwrap().lines().count(), expected.len());
		assert!(client.change_diff(run_id, jet_protocol::DiffScope::Turn { turn: expected.len() as u32 + 1 }).await.is_err());
		std::fs::remove_file(root.join("queue")).unwrap();
	}).await.unwrap();
}

async fn wait_file(path: &std::path::Path, value: &str) {
	while std::fs::read_to_string(path).ok().as_deref() != Some(value) {
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
	}
}

#[tokio::test]
async fn queued_schedule_starts_a_later_run_with_the_selected_craft() {
	tokio::time::timeout(std::time::Duration::from_secs(15), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
		let daemon = start_jetd(&home).await;
		let owner = Uuid::new_v4();
		let client = connect(&daemon, owner).await;
		let root = support::init_repository(&dir.path().join("repo"));
		std::fs::write(root.join("continue"), "go").unwrap();
		let project = client.register_project(Uuid::now_v7(), root.to_str().unwrap()).await.unwrap();
		let conversation = client.create_conversation_in(Uuid::now_v7(), jet_protocol::RetentionPolicy::Retain, jet_protocol::WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap();
		let id = conversation.conversation_id;
		let mut wire = connect_raw(&daemon, owner).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":id,"craft":"fake","prompt":"Make a change"}})).await;
		assert_eq!(wire.receive::<Value>().await["kind"], "command_result");
		loop {
			let snapshot = client.conversation(id).await.unwrap();
			if snapshot.runs[0].lifecycle == jet_protocol::RunLifecycle::Completed { break; }
			tokio::time::sleep(std::time::Duration::from_millis(20)).await;
		}
		wire.send(&submit(id, "schedule", "Make a change")).await;
		let admitted: Value = wire.receive().await;
		assert_eq!(admitted["kind"], "command_result");
		loop {
			let snapshot = client.conversation(id).await.unwrap();
			if snapshot.runs.len() == 2 && snapshot.runs[1].lifecycle == jet_protocol::RunLifecycle::Completed { break; }
			tokio::time::sleep(std::time::Duration::from_millis(20)).await;
		}
		wire.send(&json!({"kind":"query","id":2,"query":{"type":"turn_queue","conversation_id":id}})).await;
		assert_eq!(wire.receive::<Value>().await["result"]["turns"], json!([]));
		let resume: Value = serde_json::from_slice(&std::fs::read(root.join("native-resume")).unwrap()).unwrap();
		assert_eq!(resume, json!({"version":{"major":1,"minor":3},"native_conversation":"fake-native-1"}));
	}).await.unwrap();
}

#[tokio::test]
async fn concurrent_user_admissions_keep_order_and_receipts_after_restart() {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let home = dir.path().join("jet");
	let mut daemon = start_jetd(&home).await;
	let owner = Uuid::new_v4();
	let client = connect(&daemon, owner).await;
	let conversation = client
		.create_conversation(
			Uuid::now_v7(),
			jet_protocol::RetentionPolicy::Retain,
		)
		.await
		.unwrap();
	let mut first = connect_raw(&daemon, owner).await;
	let second_owner = Uuid::new_v4();
	let mut second = connect_raw(&daemon, second_owner).await;
	let request = submit(conversation.conversation_id, "user", "First");
	let other = submit(conversation.conversation_id, "user", "Second");
	tokio::join!(first.send(&request), second.send(&other));
	let (one, two) =
		tokio::join!(first.receive::<Value>(), second.receive::<Value>());
	assert_eq!(
		(&one["kind"], &two["kind"]),
		(&json!("command_result"), &json!("command_result"))
	);
	let mut entries =
		vec![one["result"]["turn"].clone(), two["result"]["turn"].clone()];
	entries.sort_by_key(|turn| {
		turn["sequence"].as_str().unwrap().parse::<u64>().unwrap()
	});
	assert_eq!(
		entries
			.iter()
			.map(|turn| turn["sequence"].clone())
			.collect::<Vec<_>>(),
		vec![json!("1"), json!("2")]
	);
	assert_ne!(entries[0]["turn_id"], entries[1]["turn_id"]);
	assert_eq!(one["result"]["turn"]["client_id"], json!(owner));
	assert_eq!(two["result"]["turn"]["client_id"], json!(second_owner));
	let query = json!({"kind":"query","id":2,"query":{"type":"turn_queue","conversation_id":conversation.conversation_id}});
	first.send(&query).await;
	let before: Value = first.receive().await;
	assert_eq!(before["result"]["turns"], json!(entries));
	daemon.child.kill().await.unwrap();
	let daemon = start_jetd(&home).await;
	let mut first = connect_raw(&daemon, owner).await;
	first.send(&request).await;
	assert_eq!(first.receive::<Value>().await, one);
	first.send(&query).await;
	assert_eq!(first.receive::<Value>().await, before);
}

fn submit(conversation_id: Uuid, source: &str, prompt: &str) -> Value {
	json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"submit_turn","conversation_id":conversation_id,"source":source,"prompt":prompt}})
}

#[tokio::test]
async fn background_slots_replace_independently_and_only_owners_withdraw_user_work()
 {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let daemon = start_jetd(&dir.path().join("jet")).await;
	let owner = Uuid::new_v4();
	let client = connect(&daemon, owner).await;
	let conversation = client
		.create_conversation(
			Uuid::now_v7(),
			jet_protocol::RetentionPolicy::Retain,
		)
		.await
		.unwrap();
	let id = conversation.conversation_id;
	let mut wire = connect_raw(&daemon, owner).await;
	let mut admitted = Vec::new();
	for source in [
		"user",
		"schedule",
		"auto_continue",
		"schedule",
		"auto_continue",
		"user",
	] {
		wire.send(&submit(id, source, source)).await;
		let result: Value = wire.receive().await;
		assert_eq!(result["kind"], "command_result", "{result}");
		admitted.push(result["result"]["turn"].clone());
	}
	let query = json!({"kind":"query","id":2,"query":{"type":"turn_queue","conversation_id":id}});
	wire.send(&query).await;
	assert_eq!(
		wire.receive::<Value>().await["result"]["turns"],
		json!([admitted[0], admitted[3], admitted[5]])
	);
	let withdraw = json!({"kind":"command","id":3,"command_id":Uuid::now_v7(),"command":{"type":"withdraw_turn","conversation_id":id,"turn_id":admitted[0]["turn_id"]}});
	let mut stranger = connect_raw(&daemon, Uuid::new_v4()).await;
	stranger.send(&withdraw).await;
	assert_eq!(
		stranger.receive::<Value>().await["error"]["code"],
		"turn.withdraw_denied"
	);
	wire.send(&withdraw).await;
	let withdrawn: Value = wire.receive().await;
	assert_eq!(withdrawn["result"]["turn"]["state"], "withdrawn");
	wire.send(&withdraw).await;
	assert_eq!(wire.receive::<Value>().await, withdrawn);
	wire.send(&query).await;
	assert_eq!(
		wire.receive::<Value>().await["result"]["turns"],
		json!([admitted[3], admitted[5]])
	);
	let events = client.events_after(0).await.unwrap();
	let outcomes: Vec<_> = events
		.events
		.iter()
		.filter(|e| e.kind == "turn.changed")
		.map(|e| e.payload["turn"]["state"].clone())
		.collect();
	assert_eq!(
		outcomes,
		json!([
			"queued",
			"queued",
			"queued",
			"superseded",
			"queued",
			"superseded",
			"queued",
			"canceled",
			"queued",
			"withdrawn"
		])
		.as_array()
		.unwrap()
		.clone()
	);
}

#[tokio::test]
async fn refused_input_preserves_the_queue_and_older_protocols_cannot_mutate_it()
 {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let daemon = start_jetd(&dir.path().join("jet")).await;
	let owner = Uuid::new_v4();
	let client = connect(&daemon, owner).await;
	let conversation = client
		.create_conversation(
			Uuid::now_v7(),
			jet_protocol::RetentionPolicy::Retain,
		)
		.await
		.unwrap();
	let id = conversation.conversation_id;
	let mut wire = connect_raw(&daemon, owner).await;
	let escaped = "\0💡".repeat(13_107) + "x";
	let mut first_turn = Value::Null;
	for (index, size) in [65_536; 15].into_iter().chain([65_535]).enumerate() {
		let prompt = if index == 0 {
			escaped.clone()
		} else {
			"x".repeat(size)
		};
		wire.send(&submit(id, "user", &prompt)).await;
		let result: Value = wire.receive().await;
		assert_eq!(result["kind"], "command_result", "{result}");
		if index == 0 {
			first_turn = result["result"]["turn"]["turn_id"].clone();
		}
	}
	let journal = client.events_after(0).await.unwrap();
	let restored = journal
		.events
		.iter()
		.filter(|event| {
			event.kind == "turn.input" && event.payload["turn_id"] == first_turn
		})
		.map(|event| event.payload["text"].as_str().unwrap())
		.collect::<String>();
	assert_eq!(restored, escaped);
	wire.send(&submit(id, "auto_continue", "x")).await;
	assert_eq!(wire.receive::<Value>().await["kind"], "command_result");
	let before = client.turn_queue(id).await.unwrap();
	for (prompt, code) in [
		("xx".into(), "turn.queue_full"),
		(String::new(), "turn.invalid_prompt"),
		("x".repeat(65_537), "turn.invalid_prompt"),
	] {
		let request = submit(id, "user", &prompt);
		wire.send(&request).await;
		let refused: Value = wire.receive().await;
		assert_eq!(refused["error"]["code"], code);
		wire.send(&request).await;
		assert_eq!(wire.receive::<Value>().await, refused);
		assert_eq!(client.turn_queue(id).await.unwrap(), before);
	}
	let mut hello = support::hello(owner);
	hello.minor = 15;
	let (mut old, _) = support::handshake_raw(&daemon, &hello).await;
	for request in [
		submit(id, "user", "Blocked"),
		json!({"kind":"query","id":2,"query":{"type":"turn_queue","conversation_id":id}}),
	] {
		old.send(&request).await;
		assert_eq!(
			old.receive::<Value>().await["error"]["code"],
			"protocol.unsupported_minor"
		);
	}
	assert_eq!(client.turn_queue(id).await.unwrap(), before);
}
