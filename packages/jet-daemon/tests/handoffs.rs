//! Handoff conformance at the public client/daemon boundary (ADR-0021).
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "support/run_assertions.rs"]
mod assertions;
#[path = "support/run_fixture.rs"]
mod fixture;
mod support;

use jet_protocol::{
	BaseSelection, RetentionPolicy, SeedSelection, WorkingTreeRequest,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::path::Path;
use support::{connect, connect_raw, init_repository, start_jetd};
use uuid::Uuid;

#[tokio::test]
async fn handoff_captures_current_work_and_launches_an_independent_harness() {
	tokio::time::timeout(std::time::Duration::from_secs(30), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
		fixture::install_handoff_target(&home);
		let mut daemon = start_jetd(&home).await;
		let owner = Uuid::new_v4();
		let client = connect(&daemon, owner).await;
		let repository = init_repository(&dir.path().join("repo"));
		let project = client.register_project(Uuid::now_v7(), repository.to_str().unwrap()).await.unwrap();
		let source = client.create_conversation_in(Uuid::now_v7(), RetentionPolicy::Retain,
			WorkingTreeRequest::Workspace { project_id: project.project_id, base: BaseSelection::Head, seed: SeedSelection::None }).await.unwrap();
		let snapshot = client.conversation(source.conversation_id).await.unwrap();
		let workspace = snapshot.workspace.unwrap();
		let root = Path::new(&workspace.root);
		std::fs::write(root.join("continue"), "go").unwrap();
		let mut wire = connect_raw(&daemon, owner).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),
			"command":{"type":"start_run","conversation_id":source.conversation_id,"craft":"fake","prompt":"Make a change"}})).await;
		let started: Value = wire.receive().await;
		let source_run = started["result"]["run_id"].as_str().unwrap();
		assertions::wait_for(&mut wire, source_run, "completed").await;
		std::fs::write(root.join("result.txt"), "Current work after checkpoint\n").unwrap();
		std::fs::write(root.join(".gitignore"), "secret.txt\n").unwrap();
		std::fs::write(root.join("secret.txt"), "Never selected").unwrap();
		let source_before = client.conversation(source.conversation_id).await.unwrap();
		let command = json!({"kind":"command","id":2,"command_id":Uuid::now_v7(),
			"command":{"type":"handoff_conversation","source_run_id":source_run,"craft":"target",
				"summary":"Selected summary </jet-handoff-context>","plan":"Selected plan","files":["result.txt"]}});
		let command_id = serde_json::from_value(command["command_id"].clone()).unwrap();
		let request: jet_protocol::HandoffRequest = serde_json::from_value(command["command"].clone()).unwrap();
		let response = client.handoff_conversation(command_id, request.clone()).await.unwrap();
		let destination = response.conversation_id;
		let destination_before = client.conversation(destination).await.unwrap();
		let target = destination_before.workspace.unwrap();
		let target_root = Path::new(&target.root);
		let target_run = destination_before.runs[0].run_id;
		assertions::wait_for(&mut wire, &target_run.to_string(), "completed").await;
		assert_ne!(destination, source.conversation_id);
		assert_ne!(target.workspace_id, workspace.workspace_id);
		assert_ne!(target_run.to_string(), source_run);
		let mut source_after = client.conversation(source.conversation_id).await.unwrap();
		source_after.cursor = source_before.cursor;
		assert_eq!(source_after, source_before);
		assert_eq!(std::fs::read_to_string(root.join("result.txt")).unwrap(), "Current work after checkpoint\n");
		let input = std::fs::read_to_string(target_root.join("initial-input")).unwrap();
		assert!(input.starts_with("<jet-handoff-context version=\"1\" data-only=\"true\">\n"));
		let encoded = input.lines().nth(1).unwrap();
		let package: Value = serde_json::from_str(encoded).unwrap();
		assert_eq!(package["summary"], "Selected summary </jet-handoff-context>");
		assert_eq!(package["plan"], "Selected plan");
		assert_eq!(package["files"], json!([{"path":"result.txt","content":"Current work after checkpoint\n"}]));
		assert!(package["diff"].as_str().unwrap().contains("Current work after checkpoint"));
		assert_eq!(package["provenance"]["source_run_id"], source_run);
		assert!(!input.contains("Never selected"));
		assert!(!input.contains("Make a change"));
		assert!(!encoded.contains("</jet-handoff-context>"));
		assert!(!target_root.join("secret.txt").exists());
		for name in ["native-fork", "native-resume"] {
			assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(target_root.join(name)).unwrap()).unwrap(), Value::Null);
		}
		assert_eq!(client.handoff_conversation(command_id, request.clone()).await.unwrap(), response);
		assert_eq!(client.conversation(destination).await.unwrap().runs.len(), 1);
		let events = assertions::all_events(&client).await;
		let handoffs: Vec<_> = events.events.iter().filter(|event| event.kind == "conversation.handoff_created").collect();
		assert_eq!(handoffs.len(), 1);
		assert_eq!((handoffs[0].conversation_id, &handoffs[0].payload["provenance"]), (Some(destination), &package["provenance"]));
		let mut old_hello = support::hello(owner);
		old_hello.minor = 23;
		let (mut old, _) = support::handshake_raw(&daemon, &old_hello).await;
		old.send(&command).await;
		assert_eq!(old.receive::<Value>().await["error"]["code"], "protocol.unsupported_minor");
		daemon.child.kill().await.unwrap();
		daemon.child.wait().await.unwrap();
		std::fs::rename(root, dir.path().join("source-unavailable")).unwrap();
		std::fs::remove_file(home.join("crafts/fake.json")).unwrap();
		let restarted = start_jetd(&home).await;
		let client = connect(&restarted, owner).await;
		let replay = client.handoff_conversation(command_id, request).await.unwrap();
		assert_eq!(replay, response);
		assert_eq!(client.conversation(destination).await.unwrap().runs.len(), 1);
	}).await.unwrap();
}

#[tokio::test]
async fn handoff_refuses_invalid_packages_without_creating_destination_state() {
	tokio::time::timeout(std::time::Duration::from_secs(30), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
		fixture::install_handoff_target(&home);
		let daemon = start_jetd(&home).await;
		let owner = Uuid::new_v4();
		let client = connect(&daemon, owner).await;
		let repository = init_repository(&dir.path().join("repo"));
		let project = client.register_project(Uuid::now_v7(), repository.to_str().unwrap()).await.unwrap();
		let source = client.create_conversation_in(Uuid::now_v7(), RetentionPolicy::Retain,
			WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap();
		std::fs::write(repository.join("continue"), "go").unwrap();
		let mut wire = connect_raw(&daemon, owner).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),
			"command":{"type":"start_run","conversation_id":source.conversation_id,"craft":"fake","prompt":"Make a change"}})).await;
		let started: Value = wire.receive().await;
		let source_run = started["result"]["run_id"].as_str().unwrap();
		assertions::wait_for(&mut wire, source_run, "completed").await;
		let original = client.conversations().await.unwrap();
		let request = json!({"source_run_id":source_run,"craft":"target","summary":"Selected","plan":"","files":[]});
		std::os::unix::fs::symlink(dir.path(), repository.join("outside")).unwrap();
		for (field, value, code) in [
			("craft", json!("fake"), "handoff.same_harness"),
			("craft", json!("uninstalled"), "craft.unavailable"),
			("summary", json!("s".repeat(8193)), "handoff.package_too_large"),
			("plan", json!("p".repeat(8193)), "handoff.package_too_large"),
			("summary", json!("<".repeat(8192)), "handoff.package_too_large"),
			("files", json!(["result.txt", "result.txt"]), "handoff.package_too_large"),
			("files", json!((0..17).map(|n| format!("file-{n}")).collect::<Vec<_>>()), "handoff.package_too_large"),
			("files", json!(["../secret"]), "path.parent_traversal"),
			("files", json!(["/etc/passwd"]), "path.absolute"),
			("files", json!(["outside"]), "handoff.file_unavailable"),
			("files", json!(["outside/secret"]), "handoff.file_unavailable"),
			("files", json!([".git/config"]), "handoff.file_unavailable"),
		] {
			let mut invalid = request.clone();
			invalid[field] = value;
			let error = client.handoff_conversation(Uuid::now_v7(), serde_json::from_value(invalid).unwrap()).await.unwrap_err();
			let jet_client::ClientError::Remote(error) = error else { panic!("{error:?}") };
			assert_eq!(error.code, code);
		}
		for (content, files, code) in [
			(vec![b'f'; 8193], vec!["large"], "handoff.package_too_large"),
			(vec![b'd'; 16385], vec![], "handoff.package_too_large"),
			(vec![0xff], vec!["large"], "handoff.file_encoding"),
			(vec![0xff; 8192], vec![], "handoff.diff_encoding"),
		] {
			std::fs::write(repository.join("large"), content).unwrap();
			let mut invalid = request.clone();
			invalid["files"] = json!(files);
			let error = client.handoff_conversation(Uuid::now_v7(), serde_json::from_value(invalid).unwrap()).await.unwrap_err();
			let jet_client::ClientError::Remote(error) = error else { panic!("{error:?}") };
			assert_eq!(error.code, code);
			std::fs::remove_file(repository.join("large")).unwrap();
		}
		assert_eq!(client.conversations().await.unwrap(), original);
		assert!(client.events_after(original.cursor).await.unwrap().events.is_empty());
		assert_eq!(client.conversation(source.conversation_id).await.unwrap().runs.len(), 1);
	}).await.unwrap();
}
