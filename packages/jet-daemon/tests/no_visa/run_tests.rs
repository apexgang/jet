//! Origin Run, live destination Pairing, and direct SSH transport conformance.
use super::{fixture, support};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path};
use uuid::Uuid;

fn executable(path: &Path, content: &str) {
	std::fs::write(path, content).unwrap();
	std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
		.unwrap();
}
fn quote(text: &str) -> String {
	format!("'{}'", text.replace('\'', "'\\''"))
}

pub(super) async fn workspace(
	client: &jet_client::Client,
	path: &Path,
) -> (jet_protocol::Conversation, jet_protocol::Workspace) {
	let root = support::init_repository(path);
	let project = client
		.register_project(Uuid::now_v7(), root.to_str().unwrap())
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
	(conversation, workspace)
}

#[tokio::test]
async fn origin_run_keeps_native_processes_local_and_brokers_only_selected_destinations()
 {
	tokio::time::timeout(std::time::Duration::from_secs(25), async {
        let temp = tempfile::tempdir_in("/tmp").unwrap();
        let origin_home = temp.path().join("origin");
        let destination_home = temp.path().join("destination");
        let bin = temp.path().join("bin"); std::fs::create_dir(&bin).unwrap();
        fixture::install_at_minor(&origin_home, 6);
        let manifest = origin_home.join("crafts/fake.json");
        let mut data:Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        data["specification"]["harness"] = json!("codex");
        data["specification"]["features"].as_array_mut().unwrap().push(json!({"name":"remote_tools"}));
        data["specification"]["broker_permissions"] = json!(["remote_tools"]);
        std::fs::write(manifest, serde_json::to_vec(&data).unwrap()).unwrap();
        let client_id = Uuid::new_v4();
        let signer = bin.join("signer");
        executable(&signer, &format!("#!/bin/sh\nset -eu\nexport JET_TEST_SIGNATURE=$(mktemp)\ntrap 'rm -f \"$JET_TEST_SIGNATURE\"' EXIT\n{} --ignored --exact fake_platform_signer >/dev/null\ncat \"$JET_TEST_SIGNATURE\"\n", quote(std::env::current_exe().unwrap().to_str().unwrap())));
        executable(&bin.join("ssh"), &format!("#!/bin/sh\nfor arg; do [ \"$arg\" != unavailable-destination ] || exit 1; done\nexec {} connect --stdio --home {}\n", quote(env!("CARGO_BIN_EXE_jetd")), quote(destination_home.to_str().unwrap())));
        let destination = support::start_jetd(&destination_home).await;
        let owner = support::connect(&destination, Uuid::new_v4()).await;
        let (_, target) = workspace(&owner, &temp.path().join("destination-repo")).await;
        let paired = support::connect(&destination, client_id).await;
        support::pairing::pair(&owner, &paired, client_id, &ed25519_dalek::SigningKey::from_bytes(&[38; 32])).await;
        let origin = support::start_jetd_process(support::jetd(&origin_home)
            .args(["--identity-signer", signer.to_str().unwrap(), "--identity-client-id", &client_id.to_string()])
            .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()))
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/jet-test-bus")).await;
        let client = support::connect(&origin, client_id).await;
        let (conversation, source) = workspace(&client, &temp.path().join("origin-repo")).await;
        let binding = client.bind_account(Uuid::now_v7(), "openai", "Origin", None, jet_protocol::CredentialSource::HarnessNative).await.unwrap();
        let mut raw = support::connect_raw(&origin, client_id).await;
        raw.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_no_visa_run",
            "conversation_id":conversation.conversation_id,"origin_plane_id":client.status().await.unwrap().plane_id,"account_binding_id":binding.binding_id,
            "craft":"fake","prompt":"Make a change","destinations":[{"plane_id":owner.status().await.unwrap().plane_id,"workspace_id":target.workspace_id,"ssh_endpoint":"paired-destination"},{"plane_id":Uuid::new_v4(),"workspace_id":Uuid::new_v4(),"ssh_endpoint":"unavailable-destination"}]
        }})).await;
        let admitted:Value = raw.receive().await;
        assert_eq!(admitted["result"]["type"], "run_created", "{admitted}");
        let run_id:Uuid = serde_json::from_value(admitted["result"]["run_id"].clone()).unwrap();
        let source = Path::new(&source.root);
        let waiting = std::time::Instant::now();
        while !source.join("remote-results").exists() {
            if waiting.elapsed() > std::time::Duration::from_secs(18) {
                raw.send(&json!({"kind":"query","id":5,"query":{"type":"run_execution","run_id":run_id}})).await;
                let snapshot:Value = raw.receive().await;
                panic!("origin broker stalled: {snapshot}");
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
        let results:Value = serde_json::from_slice(&std::fs::read(source.join("remote-results")).unwrap()).unwrap();
        assert_eq!(results[0]["outcome"]["error"]["code"], "no_visa.destination_unavailable");
        assert_eq!(results[1]["outcome"]["error"]["code"], "no_visa.destination_denied");
        assert_eq!(results[2]["outcome"], json!({"type":"completed","result":{"type":"written"}}));
        assert_eq!(std::fs::read_to_string(Path::new(&target.root).join("from-origin.txt")).unwrap(), "origin Harness requested destination work");
        assert!(!source.join("from-origin.txt").exists());
        assert!(source.join("result.txt").exists(), "the native Harness ran on the origin");
        assert!(!Path::new(&target.root).join("result.txt").exists());
        raw.send(&json!({"kind":"query","id":2,"query":{"type":"run_execution","run_id":run_id}})).await;
        let reply:Value = raw.receive().await;
        let execution:jet_protocol::RunExecution = serde_json::from_value(reply["result"].clone()).unwrap();
        assert_eq!(execution.visa, None);
        assert_eq!(execution.no_visa.unwrap().native_unavailable, vec!["remote_checkpoints", "tool_discovery", "extensions", "sandbox_internals", "persistent_terminals"]);
        assert_eq!(client.conversation(conversation.conversation_id).await.unwrap().conversation.conversation_id, conversation.conversation_id);
        std::fs::write(source.join("continue"), "go").unwrap();
    }).await.unwrap();
}
