//! Workspace terminals through the public protocol and real PTYs.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod support;
use jet_protocol::*;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::*;
use uuid::Uuid;

#[tokio::test]
async fn a_workspace_terminal_accepts_input_and_reports_real_pty_size() {
	let fixture = Fixture::new().await;
	let mut wire = fixture.attach(0, 65536).await;
	input(&mut wire, "stty size; test -t 0 && printf 'PTY_OK\\n'\n").await;
	let output = until(&mut wire, b"PTY_OK\r\n").await;
	assert!(String::from_utf8_lossy(&output).contains("24 80\r\n"));
	fixture.close().await;
}

struct Fixture {
	dir: tempfile::TempDir,
	daemon: Daemon,
	conversation: Conversation,
	workspace: Workspace,
	terminal: Value,
}
impl Fixture {
	async fn new() -> Self {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let daemon = start_jetd(&dir.path().join("jet")).await;
		let client = connect(&daemon, Uuid::new_v4()).await;
		let root = init_repository(&dir.path().join("repo"));
		let project = client
			.register_project(Uuid::now_v7(), root.to_str().unwrap())
			.await
			.unwrap();
		let conversation = client
			.create_conversation_in(
				Uuid::now_v7(),
				RetentionPolicy::Retain,
				WorkingTreeRequest::Workspace {
					project_id: project.project_id,
					base: BaseSelection::Head,
					seed: SeedSelection::None,
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
		let mut wire = connect_raw(&daemon, Uuid::new_v4()).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"open_terminal","workspace_id":workspace.workspace_id,"rows":24,"columns":80}})).await;
		let reply: Value = wire.receive().await;
		assert_eq!(reply["kind"], "command_result", "{reply}");
		let terminal = reply["result"]["terminal"]["terminal_id"].clone();
		let fixture = Self {
			dir,
			daemon,
			conversation,
			workspace,
			terminal,
		};
		fixture.wait_state("open").await;
		fixture
	}
	async fn wait_state(&self, expected: &str) {
		let mut wire = connect_raw(&self.daemon, Uuid::new_v4()).await;
		tokio::time::timeout(std::time::Duration::from_secs(12), async {
			loop {
				wire.send(&json!({"kind":"query","id":1,"query":{"type":"workspace_terminals","workspace_id":self.workspace.workspace_id}})).await;
				let reply: Value = wire.receive().await;
				if reply["result"]["terminals"][0]["state"] == expected { return; }
				tokio::time::sleep(std::time::Duration::from_millis(20)).await;
			}
		}).await.unwrap();
	}
	async fn attach(&self, after: u64, credit: u64) -> RawConnection {
		let mut wire = connect_raw(&self.daemon, Uuid::new_v4()).await;
		wire.send(&json!({"kind":"attach_terminal","id":1,"terminal_id":self.terminal,"after":after.to_string(),"credit":credit.to_string()})).await;
		let reply: Value = wire.receive().await;
		assert_eq!(reply["kind"], "terminal_attached", "{reply}");
		wire
	}
	async fn close(&self) {
		let mut wire = connect_raw(&self.daemon, Uuid::new_v4()).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"close_terminal","terminal_id":self.terminal}})).await;
		assert_eq!(wire.receive::<Value>().await["kind"], "command_result");
		self.wait_state("closed").await;
	}
}
async fn input(wire: &mut RawConnection, input: &str) {
	wire.send_frame(Frame::data(
		StreamId::new(1).unwrap(),
		input.as_bytes().to_vec(),
	))
	.await;
}
async fn until(wire: &mut RawConnection, marker: &[u8]) -> Vec<u8> {
	tokio::time::timeout(std::time::Duration::from_secs(12), async {
		let mut bytes = Vec::new();
		loop {
			match wire.receive_frame().await {
				Frame::Data { payload, .. } => bytes.extend(payload),
				Frame::Control { payload, .. } => panic!(
					"unexpected control: {}",
					String::from_utf8_lossy(&payload)
				),
			}
			if bytes.windows(marker.len()).any(|part| part == marker) {
				return bytes;
			}
		}
	})
	.await
	.unwrap()
}

#[tokio::test]
async fn shell_state_survives_restart_and_run_completion_then_closes_explicitly()
 {
	let mut fixture = Fixture::new().await;
	let mut wire = fixture.attach(0, 65536).await;
	input(&mut wire, "stty -echo; saved=survived; printf 'READY\\n'\n").await;
	let before = until(&mut wire, b"READY\r\n").await;
	fixture.daemon.child.kill().await.unwrap();
	drop(wire);
	fixture.daemon = start_jetd(&fixture.dir.path().join("jet")).await;
	fixture.wait_state("open").await;
	let client = connect(&fixture.daemon, Uuid::new_v4()).await;
	let mut run = client
		.create_run(Uuid::now_v7(), fixture.conversation.conversation_id)
		.await
		.unwrap();
	for lifecycle in [
		RunLifecycle::Starting,
		RunLifecycle::Active,
		RunLifecycle::Completed,
	] {
		run = client
			.transition_run(Uuid::now_v7(), run.run_id, run.revision, lifecycle)
			.await
			.unwrap();
	}
	let mut wire = fixture.attach(before.len() as u64, 65536).await;
	input(&mut wire, "printf '%s\\n' \"$saved\"\n").await;
	until(&mut wire, b"survived\r\n").await;
	wire.send(
		&json!({"kind":"resize_terminal","id":7,"rows":37,"columns":109}),
	)
	.await;
	loop {
		if let Frame::Control { payload, .. } = wire.receive_frame().await {
			assert_eq!(
				decode_control::<Value>(&payload).unwrap()["kind"],
				"terminal_resized"
			);
			break;
		}
	}
	input(&mut wire, "stty size; printf 'RESIZED\\n'\n").await;
	let output = until(&mut wire, b"RESIZED\r\n").await;
	assert!(String::from_utf8_lossy(&output).contains("37 109\r\n"));
	fixture.close().await;
	let mut finished = false;
	while !finished {
		if let Frame::Control { payload, .. } = wire.receive_frame().await {
			let control: StreamControl = decode_control(&payload).unwrap();
			finished =
				matches!(control, StreamControl::TerminalFinished { .. });
		}
	}
}

#[tokio::test]
async fn rolling_output_reports_an_exact_gap_and_obeys_receiver_credit() {
	let fixture = Fixture::new().await;
	let mut wire = fixture.attach(0, 65536).await;
	input(&mut wire, "stty -echo; printf 'READY\\n'\n").await;
	until(&mut wire, b"READY\r\n").await;
	drop(wire);
	let mut wire = fixture.attach(0, 0).await;
	input(
		&mut wire,
		"dd if=/dev/zero bs=1048576 count=9 2>/dev/null; printf 'ROLLED\\n'\n",
	)
	.await;
	let displaced =
		tokio::time::timeout(std::time::Duration::from_secs(15), async {
			let mut cursor = 0;
			loop {
				let Frame::Control { payload, .. } = wire.receive_frame().await
				else {
					panic!("bytes arrived without credit");
				};
				let StreamControl::TerminalGap {
					first_missing_offset,
					missing_bytes,
				} = decode_control(&payload).unwrap()
				else {
					panic!("expected explicit gap");
				};
				assert_eq!(first_missing_offset, cursor);
				cursor += missing_bytes;
				if cursor >= 1024 * 1024 {
					return cursor;
				}
			}
		})
		.await
		.unwrap();
	drop(wire);
	let mut wire = fixture.attach(0, 65536).await;
	let Frame::Control { payload, .. } = wire.receive_frame().await else {
		panic!("gap must precede retained bytes");
	};
	let StreamControl::TerminalGap {
		first_missing_offset,
		missing_bytes,
	} = decode_control(&payload).unwrap()
	else {
		panic!("missing gap");
	};
	assert_eq!(first_missing_offset, 0);
	assert!(missing_bytes >= displaced);
	let mut delivered = 0;
	while delivered < 65536 {
		let Frame::Data { payload, .. } = wire.receive_frame().await else {
			panic!("expected retained bytes");
		};
		delivered += payload.len();
	}
	assert_eq!(delivered, 65536);
	assert!(
		tokio::time::timeout(
			std::time::Duration::from_millis(100),
			wire.receive_frame()
		)
		.await
		.is_err(),
		"credit must stop output"
	);
	wire.send(&StreamControl::Credit {
		bytes: 8 * 1024 * 1024,
	})
	.await;
	let output = until(&mut wire, b"ROLLED\r\n").await;
	assert!(output.iter().filter(|byte| **byte == 0).count() > 7 * 1024 * 1024);
	// Resource conformance: the rolling raw spool cannot grow past eight MiB.
	let id = Uuid::parse_str(fixture.terminal.as_str().unwrap()).unwrap();
	let spool = fixture
		.dir
		.path()
		.join("jet/runtime")
		.join(format!("t-{}", id.simple()))
		.join("output.bin");
	assert_eq!(std::fs::metadata(spool).unwrap().len(), 8 * 1024 * 1024);
	fixture.close().await;
}

#[tokio::test]
async fn a_shell_exit_releases_its_helper_and_replays_final_output_before_finished()
 {
	let fixture = Fixture::new().await;
	let mut wire = fixture.attach(0, 65536).await;
	input(&mut wire, "stty -echo; printf 'READY\\n'\n").await;
	let prefix = until(&mut wire, b"READY\r\n").await;
	drop(wire);
	let mut wire = fixture.attach(prefix.len() as u64, 0).await;
	input(&mut wire, "printf 'FINAL_OUTPUT\\n'; exit\n").await;
	drop(wire);
	fixture.wait_state("closed").await;
	let mut wire = fixture.attach(prefix.len() as u64, 65536).await;
	let output = until(&mut wire, b"FINAL_OUTPUT\r\n").await;
	let Frame::Control { payload, .. } = wire.receive_frame().await else {
		panic!("expected terminal_finished");
	};
	assert_eq!(
		decode_control::<StreamControl>(&payload).unwrap(),
		StreamControl::TerminalFinished {
			total_bytes: (prefix.len() + output.len()) as u64
		}
	);
	let id = Uuid::parse_str(fixture.terminal.as_str().unwrap()).unwrap();
	let descriptor = fixture
		.dir
		.path()
		.join("jet/runtime")
		.join(format!("t-{}", id.simple()))
		.join("descriptor.json");
	let descriptor: TerminalDescriptor =
		serde_json::from_slice(&std::fs::read(descriptor).unwrap()).unwrap();
	// Resource conformance: no helper remains after the terminal ends.
	assert_eq!(
		jet_runtime::execution_process_identity(descriptor.pid).unwrap(),
		None
	);
}

#[tokio::test]
async fn terminal_commands_are_deduplicated_and_snapshots_follow_lifecycle_events()
 {
	let fixture = Fixture::new().await;
	let mut wire = connect_raw(&fixture.daemon, Uuid::new_v4()).await;
	let command = json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"close_terminal","terminal_id":fixture.terminal}});
	wire.send(&command).await;
	let first: Value = wire.receive().await;
	wire.send(&command).await;
	assert_eq!(wire.receive::<Value>().await, first);
	fixture.wait_state("closed").await;
	wire.send(&json!({"kind":"query","id":2,"query":{"type":"workspace_terminals","workspace_id":fixture.workspace.workspace_id}})).await;
	let snapshot: Value = wire.receive().await;
	let cursor: u64 = snapshot["result"]["cursor"]
		.as_str()
		.unwrap()
		.parse()
		.unwrap();
	wire.send(
		&json!({"kind":"query","id":3,"query":{"type":"events","after":"0"}}),
	)
	.await;
	let events: Value = wire.receive().await;
	let closed = events["result"]["events"]
		.as_array()
		.unwrap()
		.iter()
		.find(|event| {
			event["kind"] == "terminal.state_changed"
				&& event["payload"]["state"] == "closed"
		})
		.unwrap();
	assert!(
		closed["sequence"].as_str().unwrap().parse::<u64>().unwrap() <= cursor
	);
}

#[tokio::test]
async fn execution_loss_closes_a_terminal_without_launching_a_replacement() {
	let mut fixture = Fixture::new().await;
	let id = Uuid::parse_str(fixture.terminal.as_str().unwrap()).unwrap();
	let descriptor_path = fixture
		.dir
		.path()
		.join("jet/runtime")
		.join(format!("t-{}", id.simple()))
		.join("descriptor.json");
	let descriptor: TerminalDescriptor =
		serde_json::from_slice(&std::fs::read(&descriptor_path).unwrap())
			.unwrap();
	fixture.daemon.child.kill().await.unwrap();
	// Fault injection models loss of the live execution at an OS reboot.
	let pid = rustix::process::Pid::from_raw(descriptor.pid as i32).unwrap();
	rustix::process::kill_process(pid, rustix::process::Signal::KILL).unwrap();
	fixture.daemon = start_jetd(&fixture.dir.path().join("jet")).await;
	fixture.wait_state("closed").await;
	let mut wire = fixture.attach(0, 65536).await;
	loop {
		let Frame::Control { payload, .. } = wire.receive_frame().await else {
			panic!("lost execution cannot generate new output");
		};
		match decode_control::<StreamControl>(&payload).unwrap() {
			StreamControl::TerminalGap { .. } => {}
			StreamControl::TerminalFinished { .. } => break,
			other => panic!("unexpected control {other:?}"),
		}
	}
	assert_eq!(
		jet_runtime::execution_process_identity(descriptor.pid).unwrap(),
		None
	);
	assert_eq!(
		std::fs::read(descriptor_path).unwrap(),
		encode_control(&descriptor).unwrap()
	);
	fixture.daemon.child.kill().await.unwrap();
}

#[path = "terminals/recovery_tests.rs"]
mod recovery;
