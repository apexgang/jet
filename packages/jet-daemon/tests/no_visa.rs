//! No-Visa tools through authenticated Jet connections and real registered roots.
#[path = "support/run_fixture.rs"]
mod fixture;
#[path = "no_visa/run_tests.rs"]
mod run_tests;
mod support;
#[path = "no_visa/terminal_cleanup_tests.rs"]
mod terminal_cleanup_tests;
#[path = "no_visa/tool_tests.rs"]
mod tool_tests;

use ed25519_dalek::{Signer, SigningKey};
use jet_protocol::{
	Frame, FrameReader, FrameWriter, StreamId, decode_control, encode_control,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

async fn paired_wire(
	daemon: &support::Daemon,
) -> (
	tokio::process::Child,
	FrameReader<tokio::process::ChildStdout>,
	FrameWriter<tokio::process::ChildStdin>,
	Uuid,
) {
	let client_id = Uuid::new_v4();
	let signing = SigningKey::from_bytes(&[38; 32]);
	let owner = support::connect(daemon, Uuid::new_v4()).await;
	let local = support::connect(daemon, client_id).await;
	support::pairing::pair(&owner, &local, client_id, &signing).await;
	let (bridge, reader, writer) = reconnect(daemon, client_id, &signing).await;
	(bridge, reader, writer, client_id)
}

#[tokio::test]
async fn paired_tools_read_only_the_selected_registered_workspace() {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let daemon = support::start_jetd(&dir.path().join("jet")).await;
	let owner = support::connect(&daemon, Uuid::new_v4()).await;
	let root = support::init_repository(&dir.path().join("repo"));
	let project = owner
		.register_project(Uuid::now_v7(), root.to_str().unwrap())
		.await
		.unwrap();
	let conversation = owner
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
	let workspace = owner
		.conversation(conversation.conversation_id)
		.await
		.unwrap()
		.workspace
		.unwrap();
	std::fs::write(
		std::path::Path::new(&workspace.root).join("message.txt"),
		"destination content",
	)
	.unwrap();
	let (_bridge, mut reader, mut writer, _client_id) =
		paired_wire(&daemon).await;
	let request = json!({"kind":"remote_tool","id":1,"request":{
		"operation_id":Uuid::now_v7(), "origin":{"plane_id":Uuid::new_v4(),"conversation_id":Uuid::new_v4(),"run_id":Uuid::new_v4()},
		"destination_plane_id":owner.status().await.unwrap().plane_id,"workspace_id":workspace.workspace_id,
		"permissions":["remote_tools"],"action":{"type":"read_file","path":"message.txt"}
	}});
	writer
		.write(&Frame::stream_control(
			StreamId::new(1).unwrap(),
			encode_control(&request).unwrap(),
		))
		.await
		.unwrap();
	let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
		panic!()
	};
	let reply: Value = decode_control(&payload).unwrap();
	assert_eq!(
		reply,
		json!({"kind":"remote_tool_result","id":1,"result":{"type":"file","content":"destination content"}})
	);
}

#[tokio::test]
async fn remote_writes_keep_their_result_across_restart_without_repeating_the_write()
 {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let home = dir.path().join("jet");
	let mut daemon = support::start_jetd(&home).await;
	let owner_id = Uuid::new_v4();
	let owner = support::connect(&daemon, owner_id).await;
	let root = support::init_repository(&dir.path().join("repo"));
	let project = owner
		.register_project(Uuid::now_v7(), root.to_str().unwrap())
		.await
		.unwrap();
	let conversation = owner
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
	let workspace = owner
		.conversation(conversation.conversation_id)
		.await
		.unwrap()
		.workspace
		.unwrap();
	let (_bridge, mut reader, mut writer, client_id) =
		paired_wire(&daemon).await;
	let request = json!({"kind":"remote_tool","id":1,"request":{
		"operation_id":Uuid::now_v7(), "origin":{"plane_id":Uuid::new_v4(),"conversation_id":Uuid::new_v4(),"run_id":Uuid::new_v4()},
		"destination_plane_id":owner.status().await.unwrap().plane_id,"workspace_id":workspace.workspace_id,
		"permissions":["remote_tools"],"action":{"type":"write_file","path":"written.txt","content":"remote work"}
	}});
	writer
		.write(&Frame::stream_control(
			StreamId::new(1).unwrap(),
			encode_control(&request).unwrap(),
		))
		.await
		.unwrap();
	let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
		panic!()
	};
	let reply: Value = decode_control(&payload).unwrap();
	assert_eq!(
		reply,
		json!({"kind":"remote_tool_result","id":1,"result":{"type":"written"}})
	);
	let written = std::path::Path::new(&workspace.root).join("written.txt");
	assert_eq!(std::fs::read_to_string(&written).unwrap(), "remote work");
	std::fs::write(&written, "later user edit").unwrap();
	daemon.child.kill().await.unwrap();
	let daemon = support::start_jetd(&home).await;
	// Keep the same paired installation across restart.
	let signing = SigningKey::from_bytes(&[38; 32]);
	let remote = reconnect(&daemon, client_id, &signing).await;
	let (_bridge, mut reader, mut writer) = remote;
	writer
		.write(&Frame::stream_control(
			StreamId::new(1).unwrap(),
			encode_control(&request).unwrap(),
		))
		.await
		.unwrap();
	let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
		panic!()
	};
	assert_eq!(decode_control::<Value>(&payload).unwrap(), reply);
	assert_eq!(std::fs::read_to_string(written).unwrap(), "later user edit");
}

async fn reconnect(
	daemon: &support::Daemon,
	client_id: Uuid,
	signing: &SigningKey,
) -> (
	tokio::process::Child,
	FrameReader<tokio::process::ChildStdout>,
	FrameWriter<tokio::process::ChildStdin>,
) {
	let home = daemon.socket.parent().unwrap().parent().unwrap();
	let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_jetd"))
		.args(["connect", "--stdio", "--home"])
		.arg(home)
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.kill_on_drop(true)
		.spawn()
		.unwrap();
	let mut input = child.stdin.take().unwrap();
	input.write_all(jet_protocol::PREFACE).await.unwrap();
	let mut reader = FrameReader::new(child.stdout.take().unwrap());
	let mut writer = FrameWriter::new(input);
	let hello = support::hello(client_id);
	writer
		.write(&Frame::control(encode_control(&hello).unwrap()))
		.await
		.unwrap();
	let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
		panic!()
	};
	let jet_protocol::ServerHello::Challenge { nonce } =
		decode_control(&payload).unwrap()
	else {
		panic!()
	};
	let transcript =
		jet_protocol::connection_signing_bytes(&hello, &nonce).unwrap();
	writer
		.write(&Frame::control(
			encode_control(&jet_protocol::ConnectionProof {
				signature: signing.sign(&transcript).to_bytes(),
			})
			.unwrap(),
		))
		.await
		.unwrap();
	let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
		panic!()
	};
	assert!(matches!(
		decode_control::<jet_protocol::ServerHello>(&payload).unwrap(),
		jet_protocol::ServerHello::Welcome { .. }
	));
	reader.enable_multiplexing();
	writer.enable_multiplexing();
	(child, reader, writer)
}

#[tokio::test]
async fn shell_requires_exact_review_and_revocation_forces_it_to_stop() {
	tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let home = dir.path().join("jet");
        fixture::install(&home);
        let manifest = home.join("crafts/fake.json");
        let mut declaration:Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        declaration["specification"]["harness"] = json!("codex");
        std::fs::write(manifest, serde_json::to_vec(&declaration).unwrap()).unwrap();
        let daemon = support::start_jetd_with_credential_store(&home).await;
        let owner_id = Uuid::new_v4();
        let owner = support::connect(&daemon, owner_id).await;
        let root = support::init_repository(&dir.path().join("repo"));
        let project = owner.register_project(Uuid::now_v7(), root.to_str().unwrap()).await.unwrap();
        let conversation = owner.create_conversation_in(Uuid::now_v7(), jet_protocol::RetentionPolicy::Retain,
            jet_protocol::WorkingTreeRequest::Workspace { project_id:project.project_id, base:jet_protocol::BaseSelection::Head, seed:jet_protocol::SeedSelection::None }).await.unwrap();
        let workspace = owner.conversation(conversation.conversation_id).await.unwrap().workspace.unwrap();
        let (visa_conversation, visa_workspace) = run_tests::workspace(&owner, &dir.path().join("visa-repo")).await;
        let binding = owner.bind_account(Uuid::now_v7(), "openai", "Destination", None, jet_protocol::CredentialSource::HarnessNative).await.unwrap();
        let visa = owner.execute_command(Uuid::now_v7(), jet_protocol::CommandRequest::StartVisaRun(jet_protocol::VisaRunRequest {
            conversation_id:visa_conversation.conversation_id, destination_plane_id:owner.status().await.unwrap().plane_id,
            account_binding_id:binding.binding_id, craft:"fake".into(), prompt:"Make a change".into(),
        })).await.unwrap();
        let jet_protocol::CommandResponse::RunCreated(visa) = visa else { panic!("expected Visa Run"); };
        while !std::path::Path::new(&visa_workspace.root).join("result.txt").exists() { tokio::time::sleep(std::time::Duration::from_millis(20)).await; }
        let (_bridge, mut reader, mut writer, client_id) = paired_wire(&daemon).await;
        let operation_id = Uuid::now_v7();
        let request = json!({"kind":"remote_tool","id":1,"request":{
            "operation_id":operation_id, "origin":{"plane_id":Uuid::new_v4(),"conversation_id":Uuid::new_v4(),"run_id":Uuid::new_v4()},
            "destination_plane_id":owner.status().await.unwrap().plane_id,"workspace_id":workspace.workspace_id,
            "permissions":["remote_tools"],"action":{"type":"shell","directory":"","environment":[],
                "script":"printf '%s' \"$$\" > operation.pid; trap 'printf term > stopped' TERM; while :; do sleep 1; done"}
        }});
        writer.write(&Frame::stream_control(StreamId::new(1).unwrap(), encode_control(&request).unwrap())).await.unwrap();
        let Frame::Control { payload, .. } = reader.read().await.unwrap() else { panic!() };
        assert_eq!(decode_control::<Value>(&payload).unwrap(), json!({"kind":"remote_tool_result","id":1,"result":{"type":"approval_required","operation_id":operation_id}}));
        let mut review = support::connect_raw(&daemon, owner_id).await;
        review.send(&json!({"kind":"command","id":2,"command_id":Uuid::now_v7(),"command":{"type":"review_remote_tool","client_id":client_id,"operation_id":operation_id,"decision":"allow_once"}})).await;
        assert_eq!(review.receive::<Value>().await["result"]["type"], "remote_tool_reviewed");
        writer.write(&Frame::stream_control(StreamId::new(2).unwrap(), encode_control(&request).unwrap())).await.unwrap();
        let pid_path = std::path::Path::new(&workspace.root).join("operation.pid");
        while !pid_path.exists() { tokio::time::sleep(std::time::Duration::from_millis(20)).await; }
        let pid:i32 = std::fs::read_to_string(&pid_path).unwrap().parse().unwrap();
        let revoked = std::time::Instant::now();
        owner.revoke_paired_client(Uuid::now_v7(), client_id).await.unwrap();
        let pid = rustix::process::Pid::from_raw(pid).unwrap();
        while rustix::process::test_kill_process(pid).is_ok() {
            assert!(revoked.elapsed() < std::time::Duration::from_secs(4));
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(std::fs::read_to_string(std::path::Path::new(&workspace.root).join("stopped")).unwrap(), "term");
        assert_eq!(owner.conversation(conversation.conversation_id).await.unwrap().conversation, conversation);
        review.send(&json!({"kind":"query","id":4,"query":{"type":"run_execution","run_id":visa.run_id}})).await;
        let still_running:Value = review.receive().await;
        assert_eq!(still_running["result"]["run"]["lifecycle"], "active");
        for process in still_running["result"]["processes"].as_array().unwrap() {
            let pid = rustix::process::Pid::from_raw(i32::try_from(process["pid"].as_i64().unwrap()).unwrap()).unwrap();
            assert!(rustix::process::test_kill_process(pid).is_ok());
        }
        std::fs::write(std::path::Path::new(&visa_workspace.root).join("continue"), "go").unwrap();
    }).await.unwrap();
}

#[test]
#[ignore = "invoked as the platform credential storage fake"]
fn fake_platform_signer() {
	use std::io::Read;
	let mut transcript = Vec::new();
	std::io::stdin().read_to_end(&mut transcript).unwrap();
	let signature = SigningKey::from_bytes(&[38; 32])
		.sign(&transcript)
		.to_bytes();
	std::fs::write(std::env::var_os("JET_TEST_SIGNATURE").unwrap(), signature)
		.unwrap();
}
