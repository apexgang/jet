use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn terminal_output_respects_the_clients_negotiated_frame_size() {
	let fixture = Fixture::new().await;
	let mut hello = hello(Uuid::new_v4());
	hello.max_data_frame = 1024;
	let (mut wire, _) = handshake_raw(&fixture.daemon, &hello).await;
	wire.send(&json!({"kind":"attach_terminal","id":1,"terminal_id":fixture.terminal,"after":"0","credit":"65536"})).await;
	assert_eq!(wire.receive::<Value>().await["kind"], "terminal_attached");
	input(&mut wire, "stty -echo; dd if=/dev/zero bs=16384 count=1 2>/dev/null; printf 'SMALL\\n'\n").await;
	let mut total = 0;
	tokio::time::timeout(std::time::Duration::from_secs(12), async {
		loop {
			match wire.receive_frame().await {
				Frame::Data { payload, .. } => {
					assert!(payload.len() <= 1024);
					total += payload.len();
					if payload.windows(7).any(|p| p == b"SMALL\r\n") {
						break;
					}
				}
				frame => panic!("unexpected frame {frame:?}"),
			}
		}
	})
	.await
	.unwrap();
	assert!(total >= 16384);
	fixture.close().await;
}

#[tokio::test]
async fn changed_workspace_roots_block_attachment_after_restart() {
	let mut fixture = Fixture::new().await;
	fixture.daemon.child.kill().await.unwrap();
	let root = std::path::Path::new(&fixture.workspace.root);
	let moved = root.with_extension("moved");
	std::fs::rename(root, &moved).unwrap();
	std::os::unix::fs::symlink(&moved, root).unwrap();
	fixture.daemon = start_jetd(&fixture.dir.path().join("jet")).await;
	fixture.wait_state("unavailable").await;
	let mut wire = connect_raw(&fixture.daemon, Uuid::new_v4()).await;
	wire.send(&json!({"kind":"attach_terminal","id":1,"terminal_id":fixture.terminal,"after":"0","credit":"65536"})).await;
	assert_eq!(wire.receive::<Value>().await["kind"], "error");
	// Restoring roots alone must not silently adopt an Orphaned execution.
	std::fs::remove_file(root).unwrap();
	std::fs::rename(&moved, root).unwrap();
	let instance = descriptor(&fixture).instance;
	wire.send(&json!({"kind":"command","id":2,"command_id":Uuid::now_v7(),"command":{"type":"resolve_execution","execution_id":fixture.terminal,"instance":instance,"action":"adopt"}})).await;
	assert_eq!(wire.receive::<Value>().await["kind"], "command_result");
	fixture.wait_state("open").await;
	let mut attached = fixture.attach(0, 65536).await;
	input(&mut attached, "printf 'ADOPTED\\n'\n").await;
	until(&mut attached, b"ADOPTED\r\n").await;
	fixture.close().await;
}

#[tokio::test]
async fn explicit_close_finishes_with_a_background_slave_holder() {
	let fixture = Fixture::new().await;
	let mut wire = fixture.attach(0, 65536).await;
	input(&mut wire, "stty -echo; sleep 30 & printf 'BACKGROUND\\n'\n").await;
	until(&mut wire, b"BACKGROUND\r\n").await;
	let descriptor = descriptor(&fixture);
	fixture.close().await;
	tokio::time::timeout(std::time::Duration::from_secs(3), async {
		loop {
			if let Frame::Control { payload, .. } = wire.receive_frame().await
				&& matches!(
					decode_control::<StreamControl>(&payload).unwrap(),
					StreamControl::TerminalFinished { .. }
				) {
				break;
			}
		}
	})
	.await
	.unwrap();
	wait_gone(descriptor.pid).await;
}

fn descriptor(fixture: &Fixture) -> TerminalDescriptor {
	let id = Uuid::parse_str(fixture.terminal.as_str().unwrap()).unwrap();
	serde_json::from_slice(
		&std::fs::read(
			fixture
				.dir
				.path()
				.join("jet/runtime")
				.join(format!("t-{}", id.simple()))
				.join("descriptor.json"),
		)
		.unwrap(),
	)
	.unwrap()
}
async fn wait_gone(pid: u32) {
	tokio::time::timeout(std::time::Duration::from_secs(3), async {
		while jet_runtime::execution_process_identity(pid)
			.unwrap()
			.is_some()
		{
			tokio::time::sleep(std::time::Duration::from_millis(20)).await;
		}
	})
	.await
	.unwrap();
}

#[tokio::test]
async fn a_terminal_missing_from_sqlite_is_an_inspectable_orphan() {
	let mut fixture = Fixture::new().await;
	let descriptor = descriptor(&fixture);
	fixture.daemon.child.kill().await.unwrap();
	// Fault injection: restore authority without executions, preserving helpers.
	for name in [
		"plane.sqlite3",
		"plane.sqlite3-wal",
		"plane.sqlite3-shm",
		"plane.sqlite3.audit-head",
	] {
		let path = fixture.dir.path().join("jet").join(name);
		if path.exists() {
			std::fs::remove_file(path).unwrap();
		}
	}
	fixture.daemon = start_jetd(&fixture.dir.path().join("jet")).await;
	let mut wire = connect_raw(&fixture.daemon, Uuid::new_v4()).await;
	wire.send(&json!({"kind":"query","id":1,"query":{"type":"orphaned_executions","after":null}})).await;
	let reply: Value = wire.receive().await;
	let orphan = &reply["result"]["executions"][0];
	assert_eq!(orphan["role"], "terminal", "{reply}");
	assert_eq!(orphan["execution_id"], fixture.terminal);
	assert_eq!(
		orphan["metadata"]["instance"],
		descriptor.instance.to_string()
	);
	for (action, expected) in [
		("adopt", "error"),
		("leave", "command_result"),
		("terminate", "command_result"),
	] {
		wire.send(&json!({"kind":"command","id":2,"command_id":Uuid::now_v7(),"command":{"type":"resolve_execution","execution_id":fixture.terminal,"instance":descriptor.instance,"action":action}})).await;
		let reply: Value = wire.receive().await;
		assert_eq!(reply["kind"], expected, "{reply}");
	}
	wait_gone(descriptor.pid).await;
}

#[tokio::test]
async fn closed_history_does_not_exhaust_terminal_admission() {
	let mut fixture = Fixture::new().await;
	fixture.close().await;
	for _ in 0..64 {
		let mut wire = connect_raw(&fixture.daemon, Uuid::new_v4()).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"open_terminal","workspace_id":fixture.workspace.workspace_id,"rows":24,"columns":80}})).await;
		let reply: Value = wire.receive().await;
		assert_eq!(reply["kind"], "command_result", "{reply}");
		fixture.terminal = reply["result"]["terminal"]["terminal_id"].clone();
		fixture.wait_state("open").await;
		fixture.close().await;
	}
}

#[tokio::test]
async fn failed_output_retention_closes_with_a_gap_instead_of_clean_replay() {
	let fixture = Fixture::new().await;
	let mut wire = fixture.attach(0, 65536).await;
	input(&mut wire, "stty -echo; printf 'RETAINED\\n'\n").await;
	until(&mut wire, b"RETAINED\r\n").await;
	let descriptor = descriptor(&fixture);
	let path = fixture
		.dir
		.path()
		.join("jet/runtime")
		.join(format!("t-{}", descriptor.config.terminal_id.simple()));
	// Fault injection prevents the atomic replay publication after the next write.
	std::fs::create_dir(path.join("replay.pending")).unwrap();
	input(&mut wire, "printf 'RETENTION_FAILURE\\n'\n").await;
	drop(wire);
	fixture.wait_state("closed").await;
	wait_gone(descriptor.pid).await;
	let state: Value = serde_json::from_slice(
		&std::fs::read(path.join("replay.json")).unwrap(),
	)
	.unwrap();
	assert_eq!(state["closed"], false);
	let mut wire = fixture.attach(0, 65536).await;
	let Frame::Control { payload, .. } = wire.receive_frame().await else {
		panic!("uncertain bytes must be omitted");
	};
	assert!(
		matches!(decode_control::<StreamControl>(&payload).unwrap(), StreamControl::TerminalGap { missing_bytes, .. } if missing_bytes > 0)
	);
	let Frame::Control { payload, .. } = wire.receive_frame().await else {
		panic!("uncertain bytes must not be replayed");
	};
	assert!(matches!(
		decode_control::<StreamControl>(&payload).unwrap(),
		StreamControl::TerminalFinished { .. }
	));
}
