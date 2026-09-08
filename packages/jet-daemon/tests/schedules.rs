//! Public schedule Commands, restart recovery and native Turn dispatch.
#[path = "support/run_fixture.rs"]
mod fixture;
mod support;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::{connect, connect_raw, start_jetd};
use uuid::Uuid;

#[tokio::test]
async fn persisted_firing_survives_changed_zone_rules_and_waits_for_busy_run() {
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
        assert_eq!(started["kind"], "command_result");
        wait_file(&root.join("current-turn"), "initial").await;
        wire.send(&json!({"kind":"command","id":2,"command_id":Uuid::now_v7(),"command":{"type":"create_schedule","conversation_id":id,"time_zone":"UTC","local_time":"23:59:59","prompt":"Scheduled continuation"}})).await;
        let created: Value = wire.receive().await;
        assert_eq!(created["kind"], "command_result", "{created}");
        let mut task = created["result"]["task"].clone();
        let sid = Uuid::parse_str(task["schedule_id"].as_str().unwrap()).unwrap();
        let firing = task["next"]["firing_id"].as_str().unwrap().to_owned();
        daemon.child.kill().await.unwrap();

        // A previous tzdb selected an instant different from today's rules.
        // Seed that persisted selection through the store's durable record API;
        // recovery must use it instead of resolving the local time again.
        let due = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64 - 1000;
        task["next"]["due_at_unix_ms"] = json!(due);
        let store = jet_store::Store::open(&home.join("plane.sqlite3")).await.unwrap();
        store.write(async |tx| tx.save_schedule(sid, id, due, &task.to_string()).await).await.unwrap();
        store.close().await;

        let mut daemon = start_jetd(&home).await;
        let client = connect(&daemon, owner).await;
        let queue = client.turn_queue(id).await.unwrap();
        assert_eq!(queue.turns.iter().map(|t| t.source).collect::<Vec<_>>(), vec![jet_protocol::TurnSource::User, jet_protocol::TurnSource::Schedule]);
        assert!(matches!(queue.turns[0].state, jet_protocol::TurnState::Active | jet_protocol::TurnState::OutcomeUnknown));
        assert_eq!(queue.turns[1].state, jet_protocol::TurnState::Queued);
        assert_eq!(queue.turns[1].turn_id.to_string(), firing);
        assert_eq!(std::fs::read_to_string(root.join("current-turn")).unwrap(), "initial");
        std::fs::write(root.join("continue-initial"), "go").unwrap();
        wait_file(&root.join("current-turn"), &firing).await;
        assert_eq!(std::fs::read_to_string(root.join("delivered-turns")).unwrap().lines().count(), 1);
        daemon.child.kill().await.unwrap();
        let daemon = start_jetd(&home).await;
        let client = connect(&daemon, owner).await;
        assert_eq!(client.turn_queue(id).await.unwrap().turns.iter().map(|t| t.turn_id.to_string()).collect::<Vec<_>>(), vec![firing.clone()]);
        let mut wire = connect_raw(&daemon, owner).await;
        wire.send(&json!({"kind":"query","id":3,"query":{"type":"events","after":"0"}})).await;
        let events: Value = wire.receive().await;
        let outcomes = events["result"]["events"].as_array().unwrap().iter().filter(|event| event["kind"] == "schedule.fired").collect::<Vec<_>>();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0]["origin"], json!({"type":"scheduled_task","schedule_id":sid}));
        assert_eq!(outcomes[0]["payload"]["firing"], task["next"]);
        std::fs::remove_file(root.join("queue")).unwrap();
        std::fs::write(root.join(format!("continue-{firing}")), "go").unwrap();
    }).await.unwrap();
}
async fn wait_file(path: &std::path::Path, expected: &str) {
	while std::fs::read_to_string(path).ok().as_deref() != Some(expected) {
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
	}
}
