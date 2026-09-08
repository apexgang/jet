//! PTY descendants are stopped through the authenticated destination API.
use super::{paired_wire, run_tests, support};
use jet_protocol::{Frame, StreamId, decode_control, encode_control};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::{io::AsyncReadExt, net::UnixListener, time::timeout};
use uuid::Uuid;

enum Completion {
	Normal,
	Revoked,
}

#[tokio::test]
async fn completed_terminal_stops_ordinary_background_children() {
	verify_cleanup(Completion::Normal).await;
}

#[tokio::test]
async fn revoked_terminal_gives_background_children_grace_then_stops_them() {
	verify_cleanup(Completion::Revoked).await;
}

async fn verify_cleanup(completion: Completion) {
	timeout(Duration::from_secs(15), async {
		let temporary = tempfile::tempdir_in("/tmp").unwrap();
		let daemon = support::start_jetd(&temporary.path().join("jet")).await;
		let owner_id = Uuid::new_v4();
		let owner = support::connect(&daemon, owner_id).await;
		let (_, workspace) = run_tests::workspace(
			&owner,
			&temporary.path().join("repo"),
		)
		.await;
		let root = Path::new(&workspace.root);
		let socket = temporary.path().join("child.sock");
		let listener = UnixListener::bind(&socket).unwrap();
		let executable = std::env::current_exe().unwrap();
		let leader = match completion {
			Completion::Normal => "",
			Completion::Revoked => "trap ':' TERM\n",
		};
		let mut input = format!(
			"{leader}JET_TEST_PTY_SOCKET={} {} --ignored --exact terminal_cleanup_tests::background_child >/dev/null 2>&1 &\nwhile [ ! -f child.ready ]; do sleep 0.01; done",
			quote(&socket),
			quote(&executable),
		);
		if matches!(completion, Completion::Revoked) {
			input.push_str("\nwhile :; do sleep 1; done");
		}
		let (mut bridge, mut reader, mut writer, client_id) = paired_wire(&daemon).await;
		let operation_id = Uuid::now_v7();
		let request = json!({
			"kind":"remote_tool", "id":1,
			"request":{
				"operation_id":operation_id,
				"origin":{"plane_id":Uuid::new_v4(), "conversation_id":Uuid::new_v4(), "run_id":Uuid::new_v4()},
				"destination_plane_id":owner.status().await.unwrap().plane_id,
				"workspace_id":workspace.workspace_id,
				"permissions":["remote_tools"],
				"action":{"type":"terminal", "directory":"", "input":input, "rows":24, "columns":80},
			},
		});
		writer.write(&Frame::stream_control(
			StreamId::new(1).unwrap(), encode_control(&request).unwrap(),
		)).await.unwrap();
		let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
			panic!("expected exact-action review");
		};
		assert_eq!(decode_control::<Value>(&payload).unwrap(), json!({
			"kind":"remote_tool_result", "id":1,
			"result":{"type":"approval_required", "operation_id":operation_id},
		}));
		let reviewed = owner.execute_command(
			Uuid::now_v7(),
			jet_protocol::CommandRequest::ReviewRemoteTool {
				client_id,
				operation_id,
				decision: jet_protocol::RemoteToolDecision::AllowOnce,
			},
		).await.unwrap();
		assert_eq!(serde_json::to_value(reviewed).unwrap(), json!({
			"type":"remote_tool_reviewed", "operation_id":operation_id,
		}));
		writer.write(&Frame::stream_control(
			StreamId::new(2).unwrap(), encode_control(&request).unwrap(),
		)).await.unwrap();
		let (mut child, _) = listener.accept().await.unwrap();
		while !root.join("child.ready").exists() {
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
		match completion {
			Completion::Normal => {
				let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
					panic!("expected completed terminal");
				};
				assert_eq!(decode_control::<Value>(&payload).unwrap(), json!({
					"kind":"remote_tool_result", "id":1,
					"result":{"type":"process", "exit_code":0, "stdout":"", "stderr":""},
				}));
			}
			Completion::Revoked => {
				owner.revoke_paired_client(Uuid::now_v7(), client_id).await.unwrap();
			}
		}
		// EOF proves the child stopped without relying on PID reuse or zombie reaping.
		assert_eq!(timeout(Duration::from_secs(3), child.read(&mut [0; 1])).await.unwrap().unwrap(), 0);
		if matches!(completion, Completion::Revoked) {
			assert_eq!(std::fs::read_to_string(root.join("child.terminated")).unwrap(), "graceful");
		}
		bridge.kill().await.unwrap();
	}).await.unwrap();
}

fn quote(path: &Path) -> String {
	format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

#[tokio::test]
#[ignore = "invoked as a real background PTY descendant"]
async fn background_child() {
	let mut terminated = tokio::signal::unix::signal(
		tokio::signal::unix::SignalKind::terminate(),
	)
	.unwrap();
	let socket = std::env::var_os("JET_TEST_PTY_SOCKET").unwrap();
	let mut connection = tokio::net::UnixStream::connect(socket).await.unwrap();
	std::fs::write("child.ready", "ready").unwrap();
	let mut byte = [0; 1];
	tokio::select! {
		_ = terminated.recv() => {
			std::fs::write("child.terminated", "graceful").unwrap();
			// Stay alive after TERM so this test also requires the forced stop.
			let _ = timeout(Duration::from_secs(6), connection.read(&mut byte)).await;
		}
		_ = connection.read(&mut byte) => {}
		_ = tokio::time::sleep(Duration::from_secs(6)) => {}
	}
}
