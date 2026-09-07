//! Upgrade and OS restart fixtures, exercised through the public daemon boundary.
#[path = "support/run_fixture.rs"]
mod fixture;
mod support;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::{connect, connect_raw, start_jetd};
use uuid::Uuid;

#[tokio::test]
async fn pending_input_continues_a_legacy_run_after_boot_loss_with_partial_source()
 {
	tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let home = dir.path().join("jet");
        fixture::install(&home);
        let mut daemon = start_jetd(&home).await;
        let owner = Uuid::new_v4();
        let client = connect(&daemon, owner).await;
        let root = support::init_repository(&dir.path().join("repo"));
        std::fs::write(root.join("continue"), "go").unwrap();
        let project = client.register_project(Uuid::now_v7(), root.to_str().unwrap()).await.unwrap();
        let conversation = client.create_conversation_in(Uuid::now_v7(), jet_protocol::RetentionPolicy::Retain, jet_protocol::WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap();
        let id = conversation.conversation_id;
        let mut wire = connect_raw(&daemon, owner).await;
        let request = json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"submit_turn","conversation_id":id,"source":"schedule","prompt":"Make a change"}});
        wire.send(&request).await;
        let admitted: Value = wire.receive().await;
        assert_eq!(admitted["kind"], "command_result");
        daemon.child.kill().await.unwrap();

        // Seed the durable format written before Turn identities existed. The
        // prior boot proves process death even when source ended mid-batch.
        let installed: Value = serde_json::from_slice(&std::fs::read(home.join("crafts/fake.json")).unwrap()).unwrap();
        let specification: jet_protocol::CraftSpecification = serde_json::from_value(installed["specification"].clone()).unwrap();
        let contract = json!({"version":1,"boot_identity":"previous OS boot","craft_protocol":{"major":1,"minor":2},"helper_protocol":{"major":1,"minor":1},"specification":specification});
        let plan = json!({"version":1,"root":root,"project_root":root,"craft":{"executable":installed["executable"],"sha256":installed["sha256"],"adapter_state":contract.to_string()},"prompt":"previous input","client_id":owner});
        let legacy = Uuid::now_v7();
        let store = jet_store::Store::open(&home.join("plane.sqlite3")).await.unwrap();
        store.write(async |tx| {
            tx.insert_run(jet_store::NewRun { run_id: legacy, conversation_id: id, created_at_unix_ms: 1 }).await?;
            tx.update_run_lifecycle(legacy, jet_store::RunLifecycle::Active, 2).await?;
            tx.insert_run_execution(legacy, &jet_store::RunExecutionRecord {
                plan: plan.to_string(),
                state: json!({"activity":"working","processes":[],"native_conversation":"legacy-native-9","exit_code":null,"partial_source":{"count":1,"digest":"committed-prefix"}}).to_string(),
            }).await
        }).await.unwrap();
        store.close().await;

        let mut daemon = start_jetd(&home).await;
        let client = connect(&daemon, owner).await;
        let next = loop {
            let snapshot = client.conversation(id).await.unwrap();
            if snapshot.runs.len() == 2 && matches!(snapshot.runs[1].lifecycle, jet_protocol::RunLifecycle::Completed | jet_protocol::RunLifecycle::Failed | jet_protocol::RunLifecycle::Lost) {
                assert_eq!(snapshot.runs.iter().map(|run| run.lifecycle).collect::<Vec<_>>(), vec![jet_protocol::RunLifecycle::Lost, jet_protocol::RunLifecycle::Completed]);
                break snapshot.runs[1].run_id;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert!(client.turn_queue(id).await.unwrap().turns.is_empty());
        let mut wire = connect_raw(&daemon, owner).await;
        wire.send(&request).await;
        assert_eq!(wire.receive::<Value>().await, admitted);
        let resume: Value = serde_json::from_slice(&std::fs::read(root.join("native-resume")).unwrap()).unwrap();
        assert_eq!(resume, json!({"version":{"major":1,"minor":2},"native_conversation":"legacy-native-9"}));
        daemon.child.kill().await.unwrap();
        let store = jet_store::Store::open(&home.join("plane.sqlite3")).await.unwrap();
        let record = store.read(async |tx| tx.run_execution(next).await).await.unwrap().unwrap();
        let mut actual: Value = serde_json::from_str(&record.plan).unwrap();
        let mut renewed: Value = serde_json::from_str(actual["craft"]["adapter_state"].as_str().unwrap()).unwrap();
        assert_eq!(renewed["boot_identity"], jet_runtime::execution_boot_identity().unwrap());
        renewed["boot_identity"] = contract["boot_identity"].clone();
        actual["craft"]["adapter_state"] = json!(renewed.to_string());
        assert_eq!(actual["craft"], plan["craft"]);
        store.close().await;
    }).await.unwrap();
}

#[tokio::test]
async fn an_unavailable_queued_craft_does_not_block_another_conversation() {
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let home = dir.path().join("jet");
        fixture::install(&home);
        let daemon = start_jetd(&home).await;
        let owner = Uuid::new_v4();
        let client = connect(&daemon, owner).await;
        let mut wire = connect_raw(&daemon, owner).await;
        let mut conversations = Vec::new();
        for name in ["first", "second"] {
            let root = support::init_repository(&dir.path().join(name));
            std::fs::write(root.join("continue"), "go").unwrap();
            let project = client.register_project(Uuid::now_v7(), root.to_str().unwrap()).await.unwrap();
            conversations.push(client.create_conversation_in(Uuid::now_v7(), jet_protocol::RetentionPolicy::Retain, jet_protocol::WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap().conversation_id);
        }
        for (index, id) in conversations.iter().enumerate() {
            wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":id,"craft":"fake","prompt":"Make a change"}})).await;
            assert_eq!(wire.receive::<Value>().await["kind"], "command_result");
            loop {
                let snapshot = client.conversation(*id).await.unwrap();
                if snapshot.runs[0].lifecycle == jet_protocol::RunLifecycle::Completed { break; }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            if index == 0 {
                // The owner installs a new artifact under the same Craft name;
                // existing executions retain the unavailable accepted digest/path.
                std::fs::remove_file(home.join("crafts/fake-craft")).unwrap();
                let replacement = dir.path().join("replacement");
                fixture::install(&replacement);
                std::fs::copy(replacement.join("crafts/fake.json"), home.join("crafts/fake.json")).unwrap();
                wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"submit_turn","conversation_id":id,"source":"schedule","prompt":"Keep pending"}})).await;
                assert_eq!(wire.receive::<Value>().await["kind"], "command_result");
            }
        }
        let queue = client.turn_queue(conversations[0]).await.unwrap();
        assert_eq!(queue.turns.iter().map(|turn| turn.state).collect::<Vec<_>>(), vec![jet_protocol::TurnState::Queued]);
    }).await.unwrap();
}
