//! Admission and child controls through real daemon/helper processes.
#[path = "support/run_fixture.rs"]
mod fixture;
mod support;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use uuid::Uuid;

#[tokio::test]
async fn energy_changes_preserve_pending_background_turns_and_live_helpers() {
	tokio::time::timeout(Duration::from_secs(30), async {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let home = dir.path().join("jet");
        fixture::install_at_minor(&home, 9);
        let manifest = home.join("crafts/fake.json");
        let mut installed: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        installed["specification"]["features"].as_array_mut().unwrap().push(json!({"name":"subagents_limit"}));
        std::fs::write(&manifest, installed.to_string()).unwrap();
        let daemon = support::start_jetd(&home).await;
        let owner = Uuid::new_v4();
        let client = support::connect(&daemon, owner).await;
        let mut wire = support::connect_raw(&daemon, owner).await;
        set(&mut wire, "energy.constrained", json!({"type":"flag","value":true})).await;
        let root = support::init_repository(&dir.path().join("repo"));
        std::fs::write(root.join("queue"), "enabled").unwrap();
        let project = client.register_project(Uuid::now_v7(), root.to_str().unwrap()).await.unwrap();
        let conversation = client.create_conversation_in(Uuid::now_v7(), jet_protocol::RetentionPolicy::Retain,
            jet_protocol::WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap();
        let id = conversation.conversation_id;
        command(&mut wire, json!({"type":"start_run","conversation_id":id,"craft":"fake","prompt":"Make a change"})).await;
        wait_file(&root.join("current-turn"), "initial").await;
        wait_file(&manifest.with_extension("energy"), "0").await;
        set(&mut wire, "energy.low_power_concurrency", json!({"type":"count","value":0})).await;
        command(&mut wire, json!({"type":"submit_turn","conversation_id":id,"source":"schedule","prompt":"Scheduled continuation"})).await;
        let before = client.turn_queue(id).await.unwrap();
        let pending = before.turns[1].turn_id;
        std::fs::write(root.join("continue-initial"), "go").unwrap();
        loop {
            let queue = client.turn_queue(id).await.unwrap();
            if queue.turns.len() == 1 {
                assert_eq!(queue.turns, vec![before.turns[1].clone()]);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(std::fs::read_to_string(root.join("current-turn")).unwrap(), "initial");
        set(&mut wire, "energy.low_power_concurrency", json!({"type":"count","value":1})).await;
        wait_file(&root.join("current-turn"), &pending.to_string()).await;
        std::fs::remove_file(root.join("queue")).unwrap();
        std::fs::write(root.join(format!("continue-{pending}")), "go").unwrap();
        assert_eq!(std::fs::read_to_string(root.join("delivered-turns")).unwrap().lines().count(), 1);
    }).await.unwrap();
}
async fn command(wire: &mut support::RawConnection, command: Value) {
	wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":command})).await;
	let result: Value = wire.receive().await;
	assert_eq!(result["kind"], "command_result", "{result}");
}
async fn set(wire: &mut support::RawConnection, key: &str, value: Value) {
	command(wire, json!({"type":"set_setting","scope":{"type":"plane"},"key":key,"value":value})).await;
}
async fn wait_file(path: &Path, expected: &str) {
	let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
	while std::fs::read_to_string(path).ok().as_deref() != Some(expected) {
		assert!(
			tokio::time::Instant::now() < deadline,
			"{} did not contain {expected}",
			path.display()
		);
		tokio::time::sleep(Duration::from_millis(10)).await;
	}
}
