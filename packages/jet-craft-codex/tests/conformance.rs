//! The Craft against a real helper and a Codex app-server protocol peer.
use jet_protocol::*;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{
	io::{BufRead, Write},
	os::unix::fs::{OpenOptionsExt, PermissionsExt},
	path::{Path, PathBuf},
	time::Duration,
};
use tokio::{
	io::AsyncWriteExt,
	net::{
		UnixStream,
		unix::{OwnedReadHalf, OwnedWriteHalf},
	},
};
use uuid::Uuid;

type Reader = FrameReader<OwnedReadHalf>;
type Writer = FrameWriter<OwnedWriteHalf>;

#[tokio::test]
async fn a_conversation_uses_codex_native_events_through_the_craft_contract() {
	tokio::time::timeout(Duration::from_secs(180), async {
		let (root, run, harness) = workspace();
		let mut helper = start_helper(&root, run, &harness).await;
		let (socket, mut craft) = start_craft(&root, &harness);
		let (mut reader, mut writer) = accept_craft(&socket, run).await;
		command(
			&mut writer,
			&json!({
				"kind": "start", "id": run, "text": "first",
				"helper_socket": root.join("h.sock"),
			}),
		)
		.await;

		let mut events = vec![];
		let mut sent_turns = 0;
		let mut approved = false;
		let mut interrupted = false;
		loop {
			events.extend(batch(&mut reader, &mut writer).await);
			let completed = completions(&events).len();
			if completed > sent_turns {
				sent_turns = completed;
				let next = match completed {
					1 => Some(("turn-2", "second")),
					2 => Some(("turn-3", "write")),
					3 => Some(("turn-4", "slow")),
					4 => Some(("turn-5", "finish")),
					_ => None,
				};
				if let Some((id, text)) = next {
					command(
						&mut writer,
						&json!({
							"kind": "turn", "id": id, "text": text,
						}),
					)
					.await;
				}
			}
			if !interrupted
				&& events
					.iter()
					.filter(|event| matches!(event, CraftEvent::TurnStarted))
					.count() == 3
			{
				interrupted = true;
				command(
					&mut writer,
					&json!({
						"kind": "interrupt", "id": "turn-4",
					}),
				)
				.await;
			}
			if !approved
				&& events.iter().any(|event| {
					matches!(
						event,
						CraftEvent::Activity {
							activity: RunActivity::WaitingForApproval
						}
					)
				}) {
				approved = true;
				command(
					&mut writer,
					&json!({
						"kind": "action", "id": "approval-action",
						"action": {
							"kind": "approval", "request_id": "approval-1",
							"decision": "allow_once",
						},
					}),
				)
				.await;
			}
			if events
				.iter()
				.any(|event| matches!(event, CraftEvent::RunEnded { .. }))
			{
				break;
			}
		}

		assert!(matches!(events.first(), Some(CraftEvent::RunStarted {
			helper_pid, harness_pid,
		}) if helper_pid != harness_pid));
		assert_eq!(
			completions(&events),
			vec![
				(run.to_string(), "thread-native-1".into()),
				("turn-2".into(), "thread-native-1".into()),
				("turn-3".into(), "thread-native-1".into()),
				("turn-4".into(), "thread-native-1".into()),
				("turn-5".into(), "thread-native-1".into()),
			],
		);
		assert!(
			events
				.iter()
				.any(|event| matches!(event, CraftEvent::TurnStarted))
		);
		assert!(events.iter().any(|event| matches!(
			event,
			CraftEvent::TurnEnded {
				outcome: TurnOutcome::Completed
			}
		)));
		assert!(events.iter().any(|event| matches!(
			event,
			CraftEvent::TurnEnded {
				outcome: TurnOutcome::Interrupted
			}
		)));
		assert!(events.iter().any(|event| matches!(
			event,
			CraftEvent::Activity {
				activity: RunActivity::WaitingForQuota
			}
		)));
		assert!(native_events(&events).iter().any(|event| {
			event.contains("\"nativeInteger\":9007199254740993")
		}));
		assert!(native_events(&events).iter().any(|event| {
			event.contains("thread/tokenUsage/updated")
				&& event.contains("\"inputTokens\":21")
		}));
		// The turn's own counts are reported; the thread's cumulative total
		// covers earlier Runs too and is deliberately left out (ADR-0023).
		assert_eq!(
			reported(&events),
			vec![
				CraftUsage::Observed {
					observed: CraftObservedUsage {
						measurement: CraftUsageMeasurement::Turn {
							turn: run.to_string(),
							native_usage_id: Some("native-first".into()),
						},
						model: Some("gpt-5.4-codex".into()),
						estimation: CraftUsageEstimation::Measured,
						finality: CraftUsageFinality::Interim,
						tokens: CraftUsageTokens {
							input: 8,
							cached_input: 2,
							output: 5,
							reasoning: 1,
						},
					},
				},
				CraftUsage::Quota {
					quota: CraftQuotaWindow {
						window: "primary".into(),
						scope: CraftQuotaScope::ProviderAccount,
						unit: CraftQuotaUnit::Share,
						used: 4_250,
						limit: Some(10_000),
						window_seconds: Some(18_000),
						resets_in_seconds: Some(3_600),
						estimation: CraftUsageEstimation::Measured,
						finality: CraftUsageFinality::Interim,
					},
				}
			]
		);
		assert_eq!(
			presentations(&events),
			vec![
				Presentation::Markdown {
					text: "- [ ] Inspect\n- [x] Implement".into(),
				},
				Presentation::Text {
					text: "Considering the native protocol".into(),
				},
				Presentation::Markdown {
					text: "Done, **natively**.".into(),
				},
			],
		);
		assert_eq!(
			std::fs::read_to_string(root.join("decision.json")).unwrap(),
			json!({"decision": "accept"}).to_string(),
		);
		assert!(matches!(
			events.last(),
			Some(CraftEvent::RunEnded { exit_code: Some(0) })
		));
		assert!(helper.wait().await.unwrap().success());
		craft.start_kill().unwrap();
	})
	.await
	.unwrap();
}

fn completions(events: &[CraftEvent]) -> Vec<(String, String)> {
	events
		.iter()
		.filter_map(|event| match event {
			CraftEvent::Completed {
				id,
				native_conversation,
			} => Some((id.clone(), native_conversation.clone())),
			_ => None,
		})
		.collect()
}

fn reported(events: &[CraftEvent]) -> Vec<CraftUsage> {
	events
		.iter()
		.filter_map(|event| match event {
			CraftEvent::Usage { usage } => Some(usage.clone()),
			_ => None,
		})
		.collect()
}

fn native_events(events: &[CraftEvent]) -> Vec<&str> {
	events
		.iter()
		.filter_map(|event| match event {
			CraftEvent::Output { native_event, .. } => Some(native_event.get()),
			_ => None,
		})
		.collect()
}

fn presentations(events: &[CraftEvent]) -> Vec<Presentation> {
	events
		.iter()
		.filter_map(|event| match event {
			CraftEvent::Output { presentation, .. } => Some(presentation),
			_ => None,
		})
		.flatten()
		.filter_map(|block| block.known().unwrap())
		.collect()
}

async fn batch(reader: &mut Reader, writer: &mut Writer) -> Vec<CraftEvent> {
	tokio::time::timeout(Duration::from_secs(3), async {
		let mut events = vec![];
		loop {
			match event(reader).await {
				CraftEvent::Progress { source_offset, .. } => {
					command(
						writer,
						&json!({
							"kind": "acknowledge", "source_offset": source_offset,
						}),
					)
					.await;
					return events;
				}
				other => events.push(other),
			}
		}
	})
	.await
	.expect("the native source makes bounded progress")
}

fn workspace() -> (PathBuf, Uuid, PathBuf) {
	let root = tempfile::tempdir_in("/tmp")
		.unwrap()
		.keep()
		.canonicalize()
		.unwrap();
	std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
		.unwrap();
	let harness = root.join("codex");
	let script = format!(
		"#!/bin/sh\nexec {} --ignored --exact --nocapture codex_double\n",
		std::env::current_exe().unwrap().display(),
	);
	std::fs::write(&harness, script).unwrap();
	std::fs::set_permissions(&harness, std::fs::Permissions::from_mode(0o700))
		.unwrap();
	(root, Uuid::new_v4(), harness)
}

async fn start_helper(
	root: &Path,
	run: Uuid,
	harness: &Path,
) -> tokio::process::Child {
	let config = HelperConfig {
		execution_id: run,
		working_directory: root.to_str().unwrap().into(),
		project_directory: root.to_str().unwrap().into(),
		craft_digest: "test-craft".into(),
		executables: vec![harness.to_str().unwrap().into()],
	};
	let path = root.join("config.json");
	let mut file = std::fs::OpenOptions::new()
		.write(true)
		.create_new(true)
		.mode(0o600)
		.open(&path)
		.unwrap();
	file.write_all(&encode_control(&config).unwrap()).unwrap();
	let mut child = tokio::process::Command::new(
		PathBuf::from(env!("CARGO_BIN_EXE_jet-craft-codex"))
			.with_file_name("jetfueld"),
	)
	.args(["run", "--config"])
	.arg(path)
	.kill_on_drop(true)
	.spawn()
	.unwrap();
	tokio::time::timeout(Duration::from_secs(10), async {
		while !root.join("h.sock").exists() {
			assert!(
				child.try_wait().unwrap().is_none(),
				"helper exited before binding its socket"
			);
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("the helper serves its private socket");
	child
}

fn start_craft(
	root: &Path,
	harness: &Path,
) -> (PathBuf, tokio::process::Child) {
	let installed = root.join("craft/jet-craft-codex");
	std::fs::create_dir_all(installed.with_file_name(".jet")).unwrap();
	std::fs::copy(env!("CARGO_BIN_EXE_jet-craft-codex"), &installed).unwrap();
	let declaration = include_str!("../.jet/craft-spec.toml").replace(
		"name = \"codex\"",
		&format!("name = {:?}", harness.to_str().unwrap()),
	);
	std::fs::write(
		installed.with_file_name(".jet").join("craft-spec.toml"),
		declaration,
	)
	.unwrap();
	let socket = root.join("craft.sock");
	let child = tokio::process::Command::new(&installed)
		.arg("--socket")
		.arg(&socket)
		.kill_on_drop(true)
		.spawn()
		.unwrap();
	(socket, child)
}

async fn accept_craft(socket: &Path, run: Uuid) -> (Reader, Writer) {
	let mut stream = tokio::time::timeout(Duration::from_secs(10), async {
		loop {
			match UnixStream::connect(socket).await {
				Ok(stream) => break stream,
				Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
			}
		}
	})
	.await
	.expect("the Craft serves its private socket");
	stream.write_all(b"jet-craft\n").await.unwrap();
	let (read, write) = stream.into_split();
	let mut reader = FrameReader::new(read);
	let mut writer = FrameWriter::new(write);
	let hello = json!({
		"protocol": {"family": "craft", "versions": [{"major": 1, "minor": 8}], "capabilities": ["runs", "actions"]},
		"specification": {"family": "specification", "versions": [{"major": 1, "minor": 0}]},
		"execution_id": run, "resume": Value::Null,
	});
	writer
		.write(&Frame::control(encode_control(&hello).unwrap()))
		.await
		.unwrap();
	let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
		panic!("control")
	};
	let ready: CraftReady = decode_control(&payload).unwrap();
	assert_eq!(
		ready.protocol.version,
		ProtocolVersion { major: 1, minor: 8 }
	);
	reader.enable_multiplexing();
	writer.enable_multiplexing();
	(reader, writer)
}

async fn command(writer: &mut Writer, value: &Value) {
	writer
		.write(&Frame::control(encode_control(value).unwrap()))
		.await
		.unwrap();
}

async fn event(reader: &mut Reader) -> CraftEvent {
	let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
		panic!("control")
	};
	decode_control(&payload).unwrap()
}

#[test]
#[ignore = "invoked as a real Codex app-server owned by jetfueld"]
fn codex_double() {
	let input = std::io::stdin();
	let mut lines = input.lock().lines();
	let initialize = read(&mut lines);
	assert_eq!(
		initialize,
		json!({
			"id": 0, "method": "initialize",
			"params": {"clientInfo": {
				"name": "jet", "title": "Jet", "version": "0.2.0",
			}},
		}),
	);
	reply(
		&initialize,
		json!({
			"userAgent": "codex-cli/0.153.4", "platformFamily": "unix",
			"platformOs": "macos", "codexHome": "/tmp/codex",
		}),
	);
	assert_eq!(read(&mut lines), json!({"method": "initialized"}));
	let thread = read(&mut lines);
	assert_eq!(
		thread,
		json!({"id": 1, "method": "thread/start", "params": {}}),
	);
	reply(&thread, json!({"thread": {"id": "thread-native-1"}}));

	let mut request_id = 2;
	for text in ["first", "second", "write", "slow", "finish"] {
		let turn = read(&mut lines);
		assert_eq!(
			turn,
			json!({
				"id": request_id, "method": "turn/start",
				"params": {
					"threadId": "thread-native-1",
					"input": [{"type": "text", "text": text}],
				},
			}),
		);
		request_id += 1;
		let turn_id = format!("native-{text}");
		reply(
			&turn,
			json!({"turn": {
				"id": turn_id, "status": "inProgress", "items": [],
			}}),
		);
		notify(
			"turn/started",
			json!({
				"threadId": "thread-native-1",
				"turn": {"id": turn_id, "status": "inProgress", "items": []},
			}),
		);
		if text == "first" {
			notify(
				"turn/plan/updated",
				json!({
					"threadId": "thread-native-1", "turnId": turn_id,
					"plan": [
						{"step": "Inspect", "status": "pending"},
						{"step": "Implement", "status": "completed"},
					],
				}),
			);
			notify(
				"item/completed",
				json!({
					"threadId": "thread-native-1", "turnId": turn_id,
					"completedAtMs": 1,
					"item": {"id": "reasoning-1", "type": "reasoning",
						"summary": ["Considering the native protocol"]},
				}),
			);
			notify(
				"item/completed",
				json!({
					"threadId": "thread-native-1", "turnId": turn_id,
					"completedAtMs": 2,
					"nativeInteger": 9007199254740993_u64,
					"item": {"id": "message-1", "type": "agentMessage",
						"text": "Done, **natively**."},
				}),
			);
			notify(
				"thread/tokenUsage/updated",
				json!({
					"threadId": "thread-native-1", "turnId": turn_id,
					"model": "gpt-5.4-codex",
					"tokenUsage": {
						"total": {"inputTokens": 21},
						"last": {"inputTokens": 8, "cachedInputTokens": 2,
							"outputTokens": 5, "reasoningOutputTokens": 1},
					},
					"rateLimits": {
						"primary": {"usedPercent": 42.5, "windowMinutes": 300,
							"resetsInSeconds": 3600},
					},
				}),
			);
		}
		if text == "write" {
			emit(&json!({
				"id": "approval-1",
				"method": "item/commandExecution/requestApproval",
				"params": {
					"threadId": "thread-native-1", "turnId": turn_id,
					"itemId": "command-1", "startedAtMs": 1,
					"command": "touch note.txt",
				},
			}));
			let decision = read(&mut lines);
			assert_eq!(
				decision,
				json!({
					"id": "approval-1", "result": {"decision": "accept"},
				}),
			);
			std::fs::write("decision.json", decision["result"].to_string())
				.unwrap();
		}
		if text == "second" {
			notify(
				"error",
				json!({
					"threadId": "thread-native-1", "turnId": turn_id,
					"willRetry": true,
					"error": {
						"message": "quota exhausted",
						"codexErrorInfo": "rateLimitExceeded",
					},
				}),
			);
		}
		let status = if text == "slow" {
			let interrupt = read(&mut lines);
			assert_eq!(
				interrupt,
				json!({
					"id": request_id, "method": "turn/interrupt",
					"params": {
						"threadId": "thread-native-1", "turnId": turn_id,
					},
				})
			);
			request_id += 1;
			reply(&interrupt, json!({}));
			"interrupted"
		} else {
			"completed"
		};
		notify(
			"turn/completed",
			json!({
				"threadId": "thread-native-1",
				"turn": {"id": turn_id, "status": status, "items": []},
			}),
		);
	}
}

fn read(lines: &mut impl Iterator<Item = std::io::Result<String>>) -> Value {
	serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap()
}

fn reply(request: &Value, result: Value) {
	emit(&json!({"id": request["id"], "result": result}));
}

fn notify(method: &str, params: Value) {
	emit(&json!({"method": method, "params": params}));
}

fn emit(value: &Value) {
	let mut stdout = std::io::stdout().lock();
	writeln!(stdout, "{value}").unwrap();
	stdout.flush().unwrap();
}
