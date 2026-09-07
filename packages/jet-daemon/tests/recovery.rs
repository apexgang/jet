//! Crash recovery through public daemon, Craft, and helper boundaries.
#[path = "support/run_assertions.rs"]
mod assertions;
#[path = "support/run_fixture.rs"]
mod fixture;
mod support;
use assertions::{all_events, wait_for};

use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::{connect, connect_raw, init_repository, start_jetd};
use uuid::Uuid;

#[tokio::test]
async fn active_run_survives_daemon_restart_without_repeating_native_work() {
	tokio::time::timeout(std::time::Duration::from_secs(30), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
        std::fs::write(home.join("crafts/fake.drop-ack"), "drop first committed acknowledgement").unwrap();
        std::fs::write(home.join("crafts/fake.mid-batch"), "interrupt dense source").unwrap();
		let mut daemon = start_jetd(&home).await;
		let client_id = Uuid::new_v4();
		let client = connect(&daemon, client_id).await;
		let root = init_repository(&dir.path().join("repo"));
        std::fs::write(root.join("dense"), "dense source").unwrap();
		std::fs::write(root.join("partial"), "test parser recovery").unwrap();
		let project = client.register_project(Uuid::now_v7(), root.to_str().unwrap()).await.unwrap();
		let conversation = client.create_conversation_in(Uuid::now_v7(), jet_protocol::RetentionPolicy::Retain,
			jet_protocol::WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap();
		let mut wire = connect_raw(&daemon, client_id).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":conversation.conversation_id,"craft":"fake","prompt":"Make a change"}})).await;
		let admitted: Value = wire.receive().await;
		let run_id = admitted["result"]["run_id"].as_str().unwrap();
		let waiting = wait_for(&mut wire, run_id, "waiting_for_approval").await;
		while !root.join("parser-acknowledged").exists() { tokio::time::sleep(std::time::Duration::from_millis(10)).await; }
        let before = all_events(&client).await;
        let dense = before.events.iter().filter_map(|e| e.payload["native_json"].as_str()).filter_map(|raw| serde_json::from_str::<Value>(raw).ok()).filter_map(|v| v["dense"].as_u64()).collect::<Vec<_>>();
        assert_eq!(dense, (0..100).collect::<Vec<_>>());
        assert!(!home.join("crafts/fake.mid-batch").exists(), "dense record fault was exercised");
		daemon.child.kill().await.unwrap();
		std::fs::write(root.join("continue"), "go").unwrap();
		let daemon = start_jetd(&home).await;
		let client = connect(&daemon, client_id).await;
		let mut wire = connect_raw(&daemon, client_id).await;
		let completed = wait_for(&mut wire, run_id, "completed").await;
		let mut expected_processes = waiting["processes"].clone();
		for process in expected_processes.as_array_mut().unwrap() { process["running"] = json!(false); }
		assert_eq!((completed["processes"].clone(), completed["exit_code"].clone()), (expected_processes, json!(0)));
		let after = all_events(&client).await;
		let outputs = |page: &jet_protocol::EventPage| page.events.iter().filter(|e| e.kind == "run.output").count();
		assert_eq!(outputs(&after), outputs(&before) + 2);
        assert_eq!(after.events.iter().filter(|e| e.payload["native_json"] == "{\"text\":\"partial\"}").count(), 1);
		assert_eq!(client.conversation(conversation.conversation_id).await.unwrap().runs.len(), 1);
	}).await.unwrap();
}

#[tokio::test]
async fn dead_executions_become_lost_and_resume_only_as_new_runs() {
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
			jet_protocol::WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap();
		let mut wire = connect_raw(&daemon, client_id).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":conversation.conversation_id,"craft":"fake","prompt":"Make a change"}})).await;
		let admitted: Value = wire.receive().await;
		let run_id = admitted["result"]["run_id"].as_str().unwrap();
		let waiting = wait_for(&mut wire, run_id, "waiting_for_approval").await;
		let before = all_events(&client).await;
		daemon.child.kill().await.unwrap();
        let descriptor_path = home.join("runtime").join(Uuid::parse_str(run_id).unwrap().simple().to_string()).join("descriptor.json");
        let mut descriptor: Value = serde_json::from_slice(&std::fs::read(&descriptor_path).unwrap()).unwrap();
        descriptor["sha256"] = json!("00".repeat(32));
        std::fs::write(&descriptor_path, serde_json::to_vec(&descriptor).unwrap()).unwrap();
        let mut daemon = start_jetd(&home).await;
        let mut wire = connect_raw(&daemon, client_id).await;
        wire.send(&json!({"kind":"query","id":8,"query":{"type":"orphaned_executions"}})).await;
        assert_eq!(wire.receive::<Value>().await["result"]["executions"][0]["execution_id"], run_id);
        daemon.child.kill().await.unwrap();
		for process in waiting["processes"].as_array().unwrap().iter().rev() {
            let pid = rustix::process::Pid::from_raw(process["pid"].as_u64().unwrap() as i32).unwrap();
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
        }
        let daemon = start_jetd(&home).await;
        let client = connect(&daemon, client_id).await;
        let mut wire = connect_raw(&daemon, client_id).await;
        let lost = wait_for(&mut wire, run_id, "lost").await;
        assert_eq!((lost["activity"].clone(), lost["exit_code"].clone()), (Value::Null, Value::Null));
        assert!(lost["processes"].as_array().unwrap().iter().all(|p| p["running"] == false));
        assert_eq!(client.conversation(conversation.conversation_id).await.unwrap().runs.len(), 1);
        assert_eq!(std::fs::read_to_string(root.join("result.txt")).unwrap(), "Harness work\n");
        std::fs::write(root.join("continue"), "go").unwrap();
        wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":conversation.conversation_id,"craft":"fake","prompt":"Make a change"}})).await;
        let resumed: Value = wire.receive().await;
        let new_id = resumed["result"]["run_id"].as_str().unwrap();
        assert_ne!(new_id, run_id);
        wait_for(&mut wire, new_id, "completed").await;
        assert_eq!(client.conversation(conversation.conversation_id).await.unwrap().runs.len(), 2);
        assert!(!before.events.is_empty());
    }).await.unwrap();
}

#[tokio::test]
async fn craft_crashes_retry_at_the_pinned_digest_with_a_bound() {
	tokio::time::timeout(std::time::Duration::from_secs(30), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
		let daemon = start_jetd(&home).await;
		let client_id = Uuid::new_v4();
		let client = connect(&daemon, client_id).await;
		let root = init_repository(&dir.path().join("repo"));
		let project = client.register_project(Uuid::now_v7(), root.to_str().unwrap()).await.unwrap();
		let conversation = client.create_conversation_in(Uuid::now_v7(), jet_protocol::RetentionPolicy::Retain,
			jet_protocol::WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap();
		let mut wire = connect_raw(&daemon, client_id).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":conversation.conversation_id,"craft":"fake","prompt":"Make a change"}})).await;
		let admitted: Value = wire.receive().await;
		let run_id = admitted["result"]["run_id"].as_str().unwrap();
		let waiting = wait_for(&mut wire, run_id, "waiting_for_approval").await;
		let before = all_events(&client).await;
		let starts_path = home.join("crafts/fake.starts");
        let craft_pid = std::fs::read_to_string(&starts_path).unwrap().lines().next().unwrap().parse::<i32>().unwrap();
        std::fs::write(home.join("crafts/fake.crash"), "crash on recovery").unwrap();
        rustix::process::kill_process(rustix::process::Pid::from_raw(craft_pid).unwrap(), rustix::process::Signal::KILL).unwrap();
        loop {
            wire.send(&json!({"kind":"query","id":8,"query":{"type":"orphaned_executions"}})).await;
            let result: Value = wire.receive().await;
            if result["result"]["executions"][0]["execution_id"] == run_id { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let starts = std::fs::read_to_string(&starts_path).unwrap();
        assert!(starts.lines().count() <= 4, "restarts must be bounded: {starts}");
        std::fs::remove_file(home.join("crafts/fake.crash")).unwrap();
        let descriptor: Value = serde_json::from_slice(&std::fs::read(home.join("runtime").join(Uuid::parse_str(run_id).unwrap().simple().to_string()).join("descriptor.json")).unwrap()).unwrap();
        wire.send(&json!({"kind":"command","id":9,"command_id":Uuid::now_v7(),"command":{"type":"resolve_execution","execution_id":run_id,"instance":descriptor["instance"],"action":"adopt"}})).await;
        assert_eq!(wire.receive::<Value>().await["kind"], "command_result");
        let recovered = wait_for(&mut wire, run_id, "waiting_for_approval").await;
        assert_eq!(recovered["processes"], waiting["processes"]);
        std::fs::write(root.join("continue"), "go").unwrap();
		let completed = wait_for(&mut wire, run_id, "completed").await;
		let mut expected_processes = waiting["processes"].clone();
		for process in expected_processes.as_array_mut().unwrap() { process["running"] = json!(false); }
		assert_eq!((completed["processes"].clone(), completed["exit_code"].clone()), (expected_processes, json!(0)));
		let after = all_events(&client).await;
		let outputs = |page: &jet_protocol::EventPage| page.events.iter().filter(|e| e.kind == "run.output").count();
		assert_eq!(outputs(&after), outputs(&before) + 1);
		assert_eq!(client.conversation(conversation.conversation_id).await.unwrap().runs.len(), 1);
	}).await.unwrap();
}

#[tokio::test]
async fn an_unmatched_live_helper_requires_explicit_termination() {
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
			jet_protocol::WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap();
		let mut wire = connect_raw(&daemon, client_id).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":conversation.conversation_id,"craft":"fake","prompt":"Make a change"}})).await;
		let admitted: Value = wire.receive().await;
		let run_id = admitted["result"]["run_id"].as_str().unwrap();
		let waiting = wait_for(&mut wire, run_id, "waiting_for_approval").await;
		let before = all_events(&client).await;
		daemon.child.kill().await.unwrap();
		// Fault injection: restore authority without this Run, preserving runtime.
		for name in ["plane.sqlite3", "plane.sqlite3-wal", "plane.sqlite3-shm", "plane.sqlite3.audit-head"] {
			let path = home.join(name);
			if path.exists() { std::fs::rename(&path, home.join(format!("old-{name}"))).unwrap(); }
		}
		let daemon = start_jetd(&home).await;
		let mut wire = connect_raw(&daemon, client_id).await;
		wire.send(&json!({"kind":"query","id":8,"query":{"type":"orphaned_executions"}})).await;
		let orphaned: Value = wire.receive().await;
		assert_eq!(orphaned["result"]["executions"][0]["execution_id"], run_id);
		let instance = orphaned["result"]["executions"][0]["metadata"]["instance"].clone();
		let resolution = |action: &str, instance: Value| json!({"kind":"command","id":9,"command_id":Uuid::now_v7(),"command":{"type":"resolve_execution","execution_id":run_id,"instance":instance,"action":action}});
		for request in [resolution("adopt", instance.clone()), resolution("terminate", json!(Uuid::new_v4()))] {
			wire.send(&request).await;
			assert_eq!(wire.receive::<Value>().await["kind"], "error");
		}
		wire.send(&resolution("terminate", instance)).await;
		assert_eq!(wire.receive::<Value>().await["kind"], "command_result");
		loop {
			wire.send(&json!({"kind":"query","id":8,"query":{"type":"orphaned_executions"}})).await;
			if wire.receive::<Value>().await["result"]["executions"] == json!([]) { break; }
			tokio::time::sleep(std::time::Duration::from_millis(20)).await;
		}
        assert!(!home.join("runtime").join(Uuid::parse_str(run_id).unwrap().simple().to_string()).join("jetfueld").exists());
		for process in waiting["processes"].as_array().unwrap() {
			let pid = rustix::process::Pid::from_raw(process["pid"].as_u64().unwrap() as i32).unwrap();
			assert_eq!(rustix::process::test_kill_process(pid), Err(rustix::io::Errno::SRCH));
		}
		assert!(!before.events.is_empty());
	}).await.unwrap();
}

#[tokio::test]
async fn unsafe_helpers_stay_orphaned_until_interactive_adoption() {
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
			jet_protocol::WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap();
		let mut wire = connect_raw(&daemon, client_id).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":conversation.conversation_id,"craft":"fake","prompt":"Make a change"}})).await;
		let admitted: Value = wire.receive().await;
		let run_id = admitted["result"]["run_id"].as_str().unwrap();
		let waiting = wait_for(&mut wire, run_id, "waiting_for_approval").await;
		let before = all_events(&client).await;
		daemon.child.kill().await.unwrap();
		let descriptor_path = home.join("runtime").join(Uuid::parse_str(run_id).unwrap().simple().to_string()).join("descriptor.json");
		let descriptor = std::fs::read(&descriptor_path).unwrap();
		let mut changed: Value = serde_json::from_slice(&descriptor).unwrap();
		changed["process_start"] = json!("corrupted live start identity");
		std::fs::write(&descriptor_path, serde_json::to_vec(&changed).unwrap()).unwrap();
		let mut daemon = start_jetd(&home).await;
		let mut wire = connect_raw(&daemon, client_id).await;
		wire.send(&json!({"kind":"query","id":8,"query":{"type":"orphaned_executions"}})).await;
		let orphaned: Value = wire.receive().await;
		assert_eq!(orphaned["kind"], "query_result", "{orphaned}");
		assert_eq!(orphaned["result"]["executions"][0]["execution_id"], run_id);
		assert!(descriptor_path.exists(), "a live mismatch must not remove its descriptor");
        wire.send(&json!({"kind":"query","id":2,"query":{"type":"run_execution","run_id":run_id}})).await;
        assert_eq!(wire.receive::<Value>().await["result"]["run"]["lifecycle"], "active");
        let instance = Value::Null;
		let resolution = |action: &str| json!({"kind":"command","id":9,"command_id":Uuid::now_v7(),"command":{"type":"resolve_execution","execution_id":run_id,"instance":instance,"action":action}});
		wire.send(&resolution("adopt")).await;
		let refused: Value = wire.receive().await;
		assert_eq!(refused["kind"], "error", "{refused}");
		wire.send(&resolution("leave")).await;
		assert_eq!(wire.receive::<Value>().await["kind"], "command_result");
		daemon.child.kill().await.unwrap();
		std::fs::write(&descriptor_path, descriptor).unwrap();
		let daemon = start_jetd(&home).await;
		let client = connect(&daemon, client_id).await;
		let mut wire = connect_raw(&daemon, client_id).await;
		wire.send(&json!({"kind":"query","id":8,"query":{"type":"orphaned_executions"}})).await;
		assert_eq!(wire.receive::<Value>().await["result"]["executions"][0]["execution_id"], run_id);
        let instance = loop {
            wire.send(&json!({"kind":"query","id":8,"query":{"type":"orphaned_executions"}})).await;
            let fresh: Value = wire.receive().await;
            let instance = fresh["result"]["executions"][0]["metadata"]["instance"].clone();
            if !instance.is_null() { break instance; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        wire.send(&json!({"kind":"command","id":9,"command_id":Uuid::now_v7(),"command":{"type":"resolve_execution","execution_id":run_id,"instance":instance,"action":"adopt"}})).await;
		let adopted: Value = wire.receive().await;
		assert_eq!(adopted["kind"], "command_result", "{adopted}");
		std::fs::write(root.join("continue"), "go").unwrap();
		let completed = wait_for(&mut wire, run_id, "completed").await;
		let mut expected_processes = waiting["processes"].clone();
		for process in expected_processes.as_array_mut().unwrap() { process["running"] = json!(false); }
		assert_eq!((completed["processes"].clone(), completed["exit_code"].clone()), (expected_processes, json!(0)));
		let after = all_events(&client).await;
		let outputs = |page: &jet_protocol::EventPage| page.events.iter().filter(|e| e.kind == "run.output").count();
		assert_eq!(outputs(&after), outputs(&before) + 1);
		assert_eq!(client.conversation(conversation.conversation_id).await.unwrap().runs.len(), 1);
	}).await.unwrap();
}

#[tokio::test]
async fn incomplete_source_batches_commit_promptly_without_acknowledging_source()
 {
	tokio::time::timeout(std::time::Duration::from_secs(30), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
        std::fs::write(home.join("crafts/fake.stall"), "withhold source boundary").unwrap();
		let daemon = start_jetd(&home).await;
		let client_id = Uuid::new_v4();
		let client = connect(&daemon, client_id).await;
		let root = init_repository(&dir.path().join("repo"));
		let project = client.register_project(Uuid::now_v7(), root.to_str().unwrap()).await.unwrap();
		let conversation = client.create_conversation_in(Uuid::now_v7(), jet_protocol::RetentionPolicy::Retain,
			jet_protocol::WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap();
		let mut wire = connect_raw(&daemon, client_id).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":conversation.conversation_id,"craft":"fake","prompt":"Make a change"}})).await;
		let admitted: Value = wire.receive().await;
		let run_id = admitted["result"]["run_id"].as_str().unwrap();

        let active = wait_for(&mut wire, run_id, "active").await;
        let descriptor: Value = serde_json::from_slice(&std::fs::read(home.join("runtime").join(Uuid::parse_str(run_id).unwrap().simple().to_string()).join("descriptor.json")).unwrap()).unwrap();
        assert_eq!(descriptor["replay"]["acknowledged"], 0);
        assert_eq!(active["processes"][0]["pid"], descriptor["pid"]);
        std::fs::remove_file(home.join("crafts/fake.stall")).unwrap();
        let starts = std::fs::read_to_string(home.join("crafts/fake.starts")).unwrap();
        let pid = rustix::process::Pid::from_raw(starts.lines().next().unwrap().parse().unwrap()).unwrap();
        rustix::process::kill_process(pid, rustix::process::Signal::KILL).unwrap();
        let waiting = wait_for(&mut wire, run_id, "waiting_for_approval").await;
        assert_eq!(waiting["processes"][0]["pid"], descriptor["pid"]);
        std::fs::write(root.join("continue"), "go").unwrap();
        wait_for(&mut wire, run_id, "completed").await;
        let events = all_events(&client).await;
        assert_eq!(events.events.iter().filter(|e| e.kind == "run.lifecycle_changed" && e.payload["to"] == "active").count(), 1);
    }).await.unwrap();
}
