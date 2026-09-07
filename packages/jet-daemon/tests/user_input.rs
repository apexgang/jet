//! Direct user edits and structured review submissions at the public protocol seam.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::os::unix::fs::symlink;
use support::{
	connect, connect_raw, handshake_raw, hello, init_repository, start_jetd,
};
use uuid::Uuid;

#[tokio::test]
async fn user_edits_are_revision_checked_attributed_and_durable() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let root = init_repository(&dir.path().join("repo"));
	std::fs::write(root.join("notes.md"), "before\n").unwrap();
	symlink(dir.path(), root.join("escape")).unwrap();
	let client_id = Uuid::new_v4();
	let mut first = start_jetd(&home).await;
	let client = connect(&first, client_id).await;
	let project = client
		.register_project(Uuid::now_v7(), root.to_str().unwrap())
		.await
		.unwrap();
	let target = json!({"kind":"project","project_id":project.project_id});
	let mut wire = connect_raw(&first, client_id).await;

	wire.send(&json!({"kind":"query","id":1,"query":{
		"type":"editable_file","target":target,"path":"notes.md"
	}}))
	.await;
	let opened: Value = wire.receive().await;
	assert_eq!(opened["kind"], "query_result", "{opened}");
	assert_eq!(opened["result"]["content"], "before\n");
	let before = opened["result"]["revision"].clone();

	let command_id = Uuid::now_v7();
	let edit = json!({"kind":"command","id":2,"command_id":command_id,"command":{
		"type":"apply_user_edit","target":target,"path":"notes.md",
		"expected_revision":before,"content":"after\n"
	}});
	wire.send(&edit).await;
	let applied: Value = wire.receive().await;
	assert_eq!(applied["kind"], "command_result", "{applied}");
	assert_ne!(applied["result"]["revision"], before);
	assert_eq!(
		std::fs::read_to_string(root.join("notes.md")).unwrap(),
		"after\n"
	);
	wire.send(&json!({"kind":"query","id":30,"query":{
		"type":"editable_file","target":target,"path":"created.md"
	}}))
	.await;
	let missing: Value = wire.receive().await;
	assert_eq!(missing["result"]["content"], Value::Null);
	assert_eq!(missing["result"]["revision"]["mode"], "000000");
	wire.send(
		&json!({"kind":"command","id":31,"command_id":Uuid::now_v7(),"command":{
			"type":"apply_user_edit","target":target,"path":"created.md",
			"expected_revision":missing["result"]["revision"],"content":"created\n"
		}}),
	)
	.await;
	let created: Value = wire.receive().await;
	assert_eq!(created["kind"], "command_result", "{created}");
	assert_eq!(
		std::fs::read_to_string(root.join("created.md")).unwrap(),
		"created\n"
	);

	wire.send(
		&json!({"kind":"command","id":3,"command_id":Uuid::now_v7(),"command":{
			"type":"apply_user_edit","target":target,"path":"notes.md",
			"expected_revision":before,"content":"stale\n"
		}}),
	)
	.await;
	let stale: Value = wire.receive().await;
	assert_eq!(stale["error"]["code"], "user_edit.stale_revision");
	assert_eq!(
		stale["error"]["recovery_actions"],
		json!([{
			"type":"refresh_file","target":target,"path":"notes.md",
			"current_revision":applied["result"]["revision"]
		}])
	);
	assert_eq!(
		std::fs::read_to_string(root.join("notes.md")).unwrap(),
		"after\n"
	);
	wire.send(
		&json!({"kind":"command","id":32,"command_id":Uuid::now_v7(),"command":{
			"type":"apply_user_edit","target":target,"path":"notes.md",
			"expected_revision":before,"content":"after\n"
		}}),
	)
	.await;
	let same_content_stale: Value = wire.receive().await;
	assert_eq!(
		same_content_stale["error"]["code"],
		"user_edit.stale_revision"
	);

	wire.send(
		&json!({"kind":"command","id":4,"command_id":Uuid::now_v7(),"command":{
			"type":"apply_user_edit","target":target,"path":"escape/outside.md",
			"expected_revision":before,"content":"escaped\n"
		}}),
	)
	.await;
	let escaped: Value = wire.receive().await;
	assert_eq!(escaped["error"]["code"], "path.escapes_root");
	assert!(!dir.path().join("outside.md").exists());
	for (path, code) in [
		("/absolute.md", "path.absolute"),
		("../outside.md", "path.parent_traversal"),
	] {
		wire.send(
			&json!({"kind":"command","id":40,"command_id":Uuid::now_v7(),"command":{
				"type":"apply_user_edit","target":target,"path":path,
				"expected_revision":before,"content":"invalid\n"
			}}),
		)
		.await;
		let invalid: Value = wire.receive().await;
		assert_eq!(invalid["error"]["code"], code);
	}

	let events = client.events_after(0).await.unwrap();
	let event = events
		.events
		.iter()
		.find(|event| event.kind == "user_edit.applied")
		.expect("user edit event");
	assert_eq!(
		event.actor,
		jet_protocol::Actor::InteractiveClient { client_id }
	);
	assert_eq!(event.origin, None);
	assert_eq!(event.payload["path"], "notes.md");

	first.child.kill().await.unwrap();
	let second = start_jetd(&home).await;
	let mut wire = connect_raw(&second, client_id).await;
	wire.send(&edit).await;
	let replayed: Value = wire.receive().await;
	assert_eq!(replayed["result"], applied["result"]);
	wire.send(&json!({"kind":"query","id":5,"query":{
		"type":"editable_file","target":target,"path":"notes.md"
	}}))
	.await;
	let reopened: Value = wire.receive().await;
	assert_eq!(reopened["result"]["content"], "after\n");
	assert_eq!(
		reopened["result"]["revision"],
		applied["result"]["revision"]
	);
}

#[tokio::test]
async fn structured_reviews_enter_one_user_turn_and_survive_restart() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();
	let mut first = start_jetd(&home).await;
	let client = connect(&first, client_id).await;
	let conversation = client
		.create_conversation(
			Uuid::now_v7(),
			jet_protocol::RetentionPolicy::Retain,
		)
		.await
		.unwrap();
	let comments = json!([
		{"path":"src/lib.rs","line":12,"comment":"Keep this error structured."},
		{"path":"tests/user.rs","line":7,"comment":"Cover the restart path.\nDo not lose this line."}
	]);
	let command_id = Uuid::now_v7();
	let submission = json!({"kind":"command","id":1,"command_id":command_id,"command":{
		"type":"submit_review","conversation_id":conversation.conversation_id,
		"comments":comments
	}});
	let mut wire = connect_raw(&first, client_id).await;
	wire.send(&submission).await;
	let submitted: Value = wire.receive().await;
	assert_eq!(submitted["kind"], "command_result", "{submitted}");
	assert_eq!(submitted["result"]["type"], "turn_admitted");
	assert_eq!(submitted["result"]["turn"]["source"], "user");
	assert_eq!(submitted["result"]["turn"]["client_id"], json!(client_id));

	let queue = client
		.turn_queue(conversation.conversation_id)
		.await
		.unwrap();
	assert_eq!(queue.turns.len(), 1);
	assert_eq!(
		json!(queue.turns[0].turn_id),
		submitted["result"]["turn"]["turn_id"]
	);
	let events = client.events_after(0).await.unwrap();
	let event = events
		.events
		.iter()
		.find(|event| event.kind == "review.submitted")
		.expect("structured review event");
	assert_eq!(
		event.actor,
		jet_protocol::Actor::InteractiveClient { client_id }
	);
	assert_eq!(event.origin, None);
	assert_eq!(event.conversation_id, Some(conversation.conversation_id));
	assert_eq!(
		event.payload["turn_id"],
		submitted["result"]["turn"]["turn_id"]
	);
	assert_eq!(event.payload["comments"], comments);
	let restored_prompt = events
		.events
		.iter()
		.filter(|event| {
			event.kind == "turn.input"
				&& event.payload["turn_id"]
					== submitted["result"]["turn"]["turn_id"]
		})
		.map(|event| event.payload["text"].as_str().unwrap())
		.collect::<String>();
	assert_eq!(
		restored_prompt,
		"Review comments:\n\nsrc/lib.rs:12\nKeep this error structured.\n\ntests/user.rs:7\nCover the restart path.\nDo not lose this line.\n"
	);

	wire.send(
		&json!({"kind":"command","id":2,"command_id":Uuid::now_v7(),"command":{
			"type":"submit_review","conversation_id":conversation.conversation_id,
			"comments":[{"path":"../outside","line":1,"comment":"No"}]
		}}),
	)
	.await;
	let invalid: Value = wire.receive().await;
	assert_eq!(invalid["error"]["code"], "path.parent_traversal");
	wire.send(
		&json!({"kind":"command","id":3,"command_id":Uuid::now_v7(),"command":{
			"type":"submit_review","conversation_id":conversation.conversation_id,
			"comments":[
				{"path":"a","line":1,"comment":"\0".repeat(8192)},
				{"path":"b","line":1,"comment":"\0".repeat(8192)}
			]
		}}),
	)
	.await;
	let too_large: Value = wire.receive().await;
	assert_eq!(too_large["error"]["code"], "review.too_large");
	assert_eq!(
		client
			.turn_queue(conversation.conversation_id)
			.await
			.unwrap(),
		queue
	);

	first.child.kill().await.unwrap();
	let second = start_jetd(&home).await;
	let client = connect(&second, client_id).await;
	let mut wire = connect_raw(&second, client_id).await;
	wire.send(&submission).await;
	assert_eq!(wire.receive::<Value>().await["result"], submitted["result"]);
	let after_restart = client
		.turn_queue(conversation.conversation_id)
		.await
		.unwrap();
	assert_eq!(after_restart.turns, queue.turns);
	let event = client
		.events_after(0)
		.await
		.unwrap()
		.events
		.into_iter()
		.find(|event| event.kind == "review.submitted")
		.expect("review survives restart");
	assert_eq!(event.payload["comments"], comments);
}

#[tokio::test]
async fn older_protocols_cannot_read_or_submit_direct_user_input() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut older = hello(Uuid::new_v4());
	older.minor = jet_protocol::WORKSPACE_TERMINALS_MINOR;
	let (mut wire, _) = handshake_raw(&daemon, &older).await;
	let target = json!({"kind":"project","project_id":Uuid::new_v4()});

	wire.send(&json!({"kind":"query","id":1,"query":{
		"type":"editable_file","target":target,"path":"notes.md"
	}}))
	.await;
	let query: Value = wire.receive().await;
	assert_eq!(query["error"]["code"], "protocol.unsupported_minor");
	wire.send(
		&json!({"kind":"command","id":2,"command_id":Uuid::now_v7(),"command":{
			"type":"submit_review","conversation_id":Uuid::new_v4(),
			"comments":[{"path":"notes.md","line":1,"comment":"Review"}]
		}}),
	)
	.await;
	let command: Value = wire.receive().await;
	assert_eq!(command["error"]["code"], "protocol.unsupported_minor");
}

#[tokio::test]
async fn workspace_edits_use_the_registered_workspace_root() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let daemon = start_jetd(&home).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let repository = init_repository(&dir.path().join("repo"));
	let project = client
		.register_project(Uuid::now_v7(), repository.to_str().unwrap())
		.await
		.unwrap();
	let conversation = client
		.create_conversation_in(
			Uuid::now_v7(),
			jet_protocol::RetentionPolicy::Retain,
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
	let target = jet_protocol::FileTarget::Workspace {
		workspace_id: workspace.workspace_id,
	};
	let missing = client.editable_file(target, "workspace.md").await.unwrap();
	let revision = client
		.apply_user_edit(
			Uuid::now_v7(),
			target,
			"workspace.md",
			missing.revision,
			"workspace content\n",
		)
		.await
		.unwrap();
	let opened = client.editable_file(target, "workspace.md").await.unwrap();
	assert_eq!(opened.revision, revision);
	assert_eq!(opened.content.as_deref(), Some("workspace content\n"));
	assert_eq!(
		std::fs::read_to_string(
			std::path::Path::new(&workspace.root).join("workspace.md")
		)
		.unwrap(),
		"workspace content\n"
	);
	assert!(!repository.join("workspace.md").exists());
}
