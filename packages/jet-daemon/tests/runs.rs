//! Managed Run conformance through a real daemon, Craft, helper, and Harness.
#[path = "support/run_fixture.rs"]
mod fixture;
mod support;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::{connect, connect_raw, init_repository, start_jetd};
use uuid::Uuid;

#[tokio::test]
async fn managed_run_reports_attention_output_and_durable_completion() {
	tokio::time::timeout(std::time::Duration::from_secs(30), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
		let mut daemon = start_jetd(&home).await;
		let client_id = Uuid::new_v4();
		let client = connect(&daemon, client_id).await;
		let root = init_repository(&dir.path().join("repo"));
		let project = client.register_project(Uuid::now_v7(), root.to_str().unwrap()).await.unwrap();
		let conversation = client.create_conversation_in(Uuid::now_v7(), jet_protocol::RetentionPolicy::Retain,
			jet_protocol::WorkingTreeRequest::Workspace { project_id: project.project_id, base: jet_protocol::BaseSelection::Head, seed: jet_protocol::SeedSelection::None }).await.unwrap();
		let workspace = client.conversation(conversation.conversation_id).await.unwrap().workspace.unwrap();
		let mut wire = connect_raw(&daemon, client_id).await;
		let start = json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":conversation.conversation_id,"craft":"fake","prompt":"Make a change"}});
		wire.send(&start).await;
		let admitted: Value = wire.receive().await;
		assert_eq!(admitted["kind"], "command_result", "{admitted}");
		let run_id = admitted["result"]["run_id"].as_str().unwrap();
		let waiting = wait_for(&mut wire, run_id, "waiting_for_approval").await;
		assert_eq!(waiting["run"]["lifecycle"], "active");
		assert_eq!(waiting["processes"].as_array().unwrap().len(), 2);
		let pids: Vec<u64> = waiting["processes"].as_array().unwrap().iter().map(|p| p["pid"].as_u64().unwrap()).collect();
		assert!(pids[0] != pids[1] && pids.iter().all(|p| *p != u64::from(daemon.child.id().unwrap())));
		std::fs::write(std::path::Path::new(&workspace.root).join("continue"), "go").unwrap();
		let completed = wait_for(&mut wire, run_id, "completed").await;
		assert_eq!((completed["activity"].clone(), completed["exit_code"].clone()), (Value::Null, json!(0)));
		assert!(completed["processes"].as_array().unwrap().iter().all(|p| p["running"] == false));
		let journal = all_events(&client).await;
		let output = journal.events.iter().find(|e| e.kind == "run.output").unwrap();
        assert_eq!(output.origin, Some(jet_protocol::EventOrigin::Harness { run_id: Uuid::parse_str(run_id).unwrap() }));
        assert!(journal.events.iter().any(|e| e.payload["native_json"] == "{ \"text\": \"Finished\", \"native_integer\": 9007199254740993 }"));
        let activities: Vec<Value> = journal.events.iter().filter(|e| e.kind == "run.activity_changed").map(|e| e.payload["activity"].clone()).collect();
        assert_eq!(activities, vec![json!("working"), json!("waiting_for_user"), json!("waiting_for_auth"), json!("waiting_for_quota"), json!("reconnecting"), json!("waiting_for_approval"), Value::Null]);
        // The prior minor's closed Actor and typed lifecycle payload remain readable.
        let mut hello = support::hello(client_id);
        hello.minor = 12;
        let (mut old, _) = support::handshake_raw(&daemon, &hello).await;
        old.send(&json!({"kind":"query","id":3,"query":{"type":"events","after":"0"}})).await;
        let old_page: Value = old.receive().await;
        assert!(old_page["result"]["events"].as_array().unwrap().len() < journal.events.len());
        for value in old_page["result"]["events"].as_array().unwrap() {
            let actor: jet_protocol::Actor = serde_json::from_value(value["actor"].clone()).unwrap();
            assert_eq!(actor, jet_protocol::Actor::InteractiveClient { client_id });
            if value["kind"] == "run.lifecycle_changed" {
                let lifecycle: LegacyLifecycle = serde_json::from_value(value["payload"].clone()).unwrap();
                assert_ne!(lifecycle.from, lifecycle.to);
            }
        }

        // Search at minor 12 still traverses managed Run Events, while Run
        // requests require minor 13 and cannot execute through an older peer.
        old.send(&json!({"kind":"query","id":4,"query":{"type":"search","text":"workspaces"}})).await;
        let searched: Value = old.receive().await;
        assert_eq!(searched["kind"], "query_result");
        assert_eq!(searched["result"]["indexed_through"], journal.cursor.to_string());
        assert!(searched["result"]["hits"].as_array().unwrap().iter().any(|hit| hit["conversation_id"] == conversation.conversation_id.to_string()));
        let mut unsupported_start = start.clone();
        unsupported_start["command_id"] = json!(Uuid::now_v7());
        for request in [
            json!({"kind":"query","id":5,"query":{"type":"run_execution","run_id":run_id}}),
            unsupported_start,
        ] {
            old.send(&request).await;
            let refused: Value = old.receive().await;
            assert_eq!((refused["kind"].clone(), refused["error"]["code"].clone()), (json!("error"), json!("protocol.unsupported_minor")));
        }
        let raw_search = client.search("native_integer").await.unwrap();
        assert_eq!((raw_search.indexed_through, raw_search.hits), (journal.cursor, vec![]));

		assert_eq!(std::fs::read_to_string(std::path::Path::new(&workspace.root).join("result.txt")).unwrap(), "Harness work\n");
		assert!(!root.join("result.txt").exists());
		daemon.child.kill().await.unwrap();
		let daemon = start_jetd(&home).await;
		let client = connect(&daemon, client_id).await;
		let mut wire = connect_raw(&daemon, client_id).await;
		wire.send(&start).await;
		assert_eq!(wire.receive::<Value>().await, admitted);
		assert_eq!(wait_for(&mut wire, run_id, "completed").await, completed);
		assert_eq!(all_events(&client).await, journal);
        let restarted_search = client.search("workspaces").await.unwrap();
        let expected_search: jet_protocol::SearchResult = serde_json::from_value(searched["result"].clone()).unwrap();
        assert_eq!(restarted_search, expected_search);
	}).await.unwrap();
}

async fn wait_for(
	wire: &mut support::RawConnection,
	run_id: &str,
	state: &str,
) -> Value {
	loop {
		wire.send(&json!({"kind":"query","id":2,"query":{"type":"run_execution","run_id":run_id}})).await;
		let response: Value = wire.receive().await;
		assert_eq!(response["kind"], "query_result", "{response}");
		let result = &response["result"];
		if result["activity"] == state || result["run"]["lifecycle"] == state {
			return result.clone();
		}
		tokio::time::sleep(std::time::Duration::from_millis(20)).await;
	}
}

#[tokio::test]
async fn definite_launch_failures_finish_and_release_admission() {
	tokio::time::timeout(std::time::Duration::from_secs(30), async {
        for mode in ["craft", "harness", "exit"] {
            let dir = tempfile::tempdir_in("/tmp").unwrap();
            let home = dir.path().join("jet");
            fixture::install(&home);
            if mode == "craft" {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(home.join("crafts/fake-craft"), std::fs::Permissions::from_mode(0o600)).unwrap();
            }
            let daemon = start_jetd(&home).await;
            let client_id = Uuid::new_v4();
            let client = connect(&daemon, client_id).await;
            let root = init_repository(&dir.path().join("repo"));
            let project = client.register_project(Uuid::now_v7(), root.to_str().unwrap()).await.unwrap();
            let conversation = client.create_conversation_in(Uuid::now_v7(), jet_protocol::RetentionPolicy::Retain, jet_protocol::WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap();
            let mut wire = connect_raw(&daemon, client_id).await;
            for _ in 0..2 {
                wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":conversation.conversation_id,"craft":"fake","prompt":if mode == "exit" { "Fail after spawn" } else { "Fail native launch" }}})).await;
                let admitted: Value = wire.receive().await;
                assert_eq!(admitted["kind"], "command_result", "{admitted}");
                let run_id = admitted["result"]["run_id"].as_str().unwrap();
                let failed = wait_for(&mut wire, run_id, "failed").await;
                assert_eq!(failed["activity"], Value::Null);
                if mode == "exit" {
                    assert_eq!(failed["exit_code"], 7);
                    assert_eq!(failed["processes"].as_array().unwrap().len(), 2);
                    assert!(failed["processes"].as_array().unwrap().iter().all(|p| p["running"] == false));
                } else { assert_eq!(failed["processes"], json!([])); }
                if mode != "craft" {
                    let socket = home.join("runtime").join(Uuid::parse_str(run_id).unwrap().simple().to_string()).join("h.sock");
                    while socket.exists() { tokio::time::sleep(std::time::Duration::from_millis(10)).await; }
                }
            }
            assert!(!root.join("result.txt").exists());
        }
    }).await.unwrap();
}

#[derive(serde::Deserialize)]
struct LegacyLifecycle {
	from: jet_protocol::RunLifecycle,
	to: jet_protocol::RunLifecycle,
}

async fn all_events(client: &jet_client::Client) -> jet_protocol::EventPage {
	let mut events = Vec::new();
	let mut after = 0;
	loop {
		let page = client.events_after(after).await.unwrap();
		if let Some(event) = page.events.last() {
			after = event.sequence;
		} else {
			assert_eq!(after, page.cursor, "page must make progress");
		}
		events.extend(page.events);
		if after == page.cursor {
			return jet_protocol::EventPage {
				cursor: after,
				events,
			};
		}
	}
}
