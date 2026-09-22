//! The Craft against a real helper and a Codex app-server protocol peer.
use jet_protocol::*;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{
	io::{BufRead, Write},
	os::unix::fs::{OpenOptionsExt, PermissionsExt},
	path::{Path, PathBuf},
	sync::{Mutex, MutexGuard, PoisonError},
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
		// The host was told exactly what the Harness asked to do before it
		// was told the Harness was waiting, and the decision it sent back
		// answers that one request (ADR-0012).
		assert_eq!(
			events
				.iter()
				.filter_map(|event| match event {
					CraftEvent::ApprovalRequested { request } =>
						Some(request.clone()),
					_ => None,
				})
				.map(|request| (
					request.request_id,
					request.tool,
					request.action
				))
				.collect::<Vec<_>>(),
			vec![(
				"approval-1".into(),
				"item/commandExecution/requestApproval".into(),
				json!({
					"command": "touch note.txt", "itemId": "command-1",
					"startedAtMs": 1, "threadId": "thread-native-1",
					"turnId": "native-write",
				})
				.to_string()
			)]
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

#[tokio::test]
async fn an_imported_thread_resumes_with_its_pinned_model() {
	tokio::time::timeout(Duration::from_secs(20), async {
        let (root, run, harness) = workspace();
        let mut helper = start_helper(&root, run, &harness).await;
        let (socket, mut craft) = start_craft(&root, &harness);
        let (mut reader, mut writer) = connect_craft(&socket, run, json!({
            "native_conversation": "imported-thread", "model": "pinned-model",
            "version": {"major": 1, "minor": 10},
        })).await;
        command(&mut writer, &json!({"kind":"start", "id":run, "text":"resume",
            "helper_socket":root.join("h.sock")})).await;
        let mut events = vec![];
        loop {
            events.extend(batch(&mut reader, &mut writer).await);
            if events.iter().any(|event| matches!(event, CraftEvent::RunEnded { .. })) { break; }
        }
        assert_eq!(completions(&events), vec![(run.to_string(), "imported-thread".into())]);
        assert!(events.iter().any(|event| matches!(event, CraftEvent::Model { model } if model == "pinned-model")));
        assert!(helper.wait().await.unwrap().success());
        craft.start_kill().unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn a_resumed_thread_cannot_silently_change_the_model() {
	tokio::time::timeout(Duration::from_secs(20), async {
        let (root, run, harness) = workspace();
        std::fs::write(root.join("wrong-model"), "").unwrap();
        let _helper = start_helper(&root, run, &harness).await;
        let (socket, _craft) = start_craft(&root, &harness);
        let (mut reader, mut writer) = connect_craft(&socket, run, json!({
            "native_conversation":"imported-thread","model":"pinned-model","version":{"major":1,"minor":10}
        })).await;
        command(&mut writer, &json!({"kind":"start","id":run,"text":"resume","helper_socket":root.join("h.sock")})).await;
        while let Ok(Frame::Control { payload, .. }) = reader.read().await {
            match decode_control::<CraftEvent>(&payload).unwrap() {
                CraftEvent::Progress { source_offset, .. } => command(&mut writer, &json!({"kind":"acknowledge","source_offset":source_offset})).await,
                CraftEvent::Completed { .. } | CraftEvent::Model { .. } => panic!("a different Model must not be admitted"),
                _ => {}
            }
        }
        assert!(!root.join("resumed-turn").exists());
    }).await.unwrap();
}

#[tokio::test]
async fn a_disconnected_remote_call_is_not_replayed_and_later_calls_can_reconnect()
 {
	use tokio::io::{AsyncBufReadExt, BufReader};
	tokio::time::timeout(Duration::from_secs(10), async {
		let root = tempfile::tempdir_in("/tmp").unwrap();
		let socket = root.path().join("mcp.sock");
		let listener = tokio::net::UnixListener::bind(&socket).unwrap();
		let mut proxy = spawn_serialized(
			tokio::process::Command::new(env!("CARGO_BIN_EXE_jet-craft-codex"))
				.arg("--remote-tools-socket")
				.arg(&socket)
				.stdin(std::process::Stdio::piped())
				.stdout(std::process::Stdio::piped())
				.kill_on_drop(true),
		)
		.unwrap();
		let mut input = proxy.stdin.take().unwrap();
		let mut output = BufReader::new(proxy.stdout.take().unwrap()).lines();
		input
			.write_all(
				b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\"}\n",
			)
			.await
			.unwrap();
		let (stream, _) = listener.accept().await.unwrap();
		let mut request = BufReader::new(stream);
		let mut line = String::new();
		request.read_line(&mut line).await.unwrap();
		drop(request);
		let failed: Value =
			serde_json::from_str(&output.next_line().await.unwrap().unwrap())
				.unwrap();
		assert_eq!(failed["id"], 1);
		assert!(
			failed["error"]["message"]
				.as_str()
				.unwrap()
				.contains("outcome unknown")
		);
		input
			.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n")
			.await
			.unwrap();
		let (stream, _) = listener.accept().await.unwrap();
		let mut request = BufReader::new(stream);
		line.clear();
		request.read_line(&mut line).await.unwrap();
		assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["id"], 2);
		request
			.get_mut()
			.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n")
			.await
			.unwrap();
		assert_eq!(
			serde_json::from_str::<Value>(
				&output.next_line().await.unwrap().unwrap()
			)
			.unwrap(),
			json!({"jsonrpc":"2.0","id":2,"result":{}})
		);
		drop(input);
		assert!(proxy.wait().await.unwrap().success());
	})
	.await
	.unwrap();
}

#[tokio::test]
async fn craft_and_host_restarts_keep_the_live_thread_and_turn_sequence() {
	tokio::time::timeout(Duration::from_secs(30), async {
        let (root, run, harness) = workspace();
        std::fs::write(root.join("recovery"), "").unwrap();
        let mut helper = start_helper(&root, run, &harness).await;
        let (socket, mut craft) = start_craft(&root, &harness);
        let (mut reader, mut writer) = accept_craft(&socket, run).await;
        command(&mut writer, &json!({"kind":"start","id":run,"text":"first",
            "helper_socket":root.join("h.sock")})).await;
        let mut expected = run.to_string();
        for restart in 0..2 {
            let (offset, checkpoint) = loop {
                let (events, offset, checkpoint) = recorded_batch(&mut reader, &mut writer).await;
                if !completions(&events).is_empty() {
                    assert_eq!(completions(&events), vec![(expected.clone(), "thread-native-1".into())]);
                    break (offset, checkpoint);
                }
            };
            drop(reader);
            drop(writer);
            if restart == 0 {
                craft.kill().await.unwrap();
                std::fs::remove_file(&socket).unwrap();
                craft = spawn_serialized(tokio::process::Command::new(root.join("craft/jet-craft-codex"))
                    .arg("--socket").arg(&socket).kill_on_drop(true)).unwrap();
            }
            (reader, writer) = accept_craft(&socket, run).await;
            command(&mut writer, &json!({"kind":"recover","id":run,
                "helper_socket":root.join("h.sock"),"source_offset":offset,"checkpoint":checkpoint})).await;
            assert!(matches!(event(&mut reader).await, CraftEvent::RunRecovered { source_offset, .. } if source_offset == offset));
            expected = format!("after-restart-{restart}");
            command(&mut writer, &json!({"kind":"turn","id":expected,"text":"continue"})).await;
        }
        let mut events = vec![];
        loop {
            events.extend(batch(&mut reader, &mut writer).await);
            if events.iter().any(|e| matches!(e, CraftEvent::RunEnded { .. })) { break; }
        }
        assert_eq!(completions(&events), vec![(expected, "thread-native-1".into())]);
        assert!(helper.wait().await.unwrap().success());
        craft.start_kill().unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn recovery_refuses_an_uncheckpointed_native_turn_instead_of_replaying_it()
 {
	tokio::time::timeout(Duration::from_secs(20), async {
        let (root, run, harness) = workspace();
        std::fs::write(root.join("recovery"), "").unwrap();
        let _helper = start_helper(&root, run, &harness).await;
        let (socket, mut craft) = start_craft(&root, &harness);
        let (mut reader, mut writer) = accept_craft(&socket, run).await;
        command(&mut writer, &json!({"kind":"start","id":run,"text":"first","helper_socket":root.join("h.sock")})).await;
        let (offset, checkpoint) = loop {
            let (events, offset, checkpoint) = recorded_batch(&mut reader, &mut writer).await;
            if !completions(&events).is_empty() { break (offset, checkpoint); }
        };
        command(&mut writer, &json!({"kind":"turn","id":"uncheckpointed-turn","text":"continue"})).await;
        loop {
            if let CraftEvent::Output { native_event, .. } = event(&mut reader).await {
                let native: Value = serde_json::from_str(native_event.get()).unwrap();
                if native.pointer("/result/turn/id").is_some() { break; }
            }
        }
        craft.kill().await.unwrap();
        drop(reader); drop(writer);
        std::fs::remove_file(&socket).unwrap();
        let _craft = spawn_serialized(tokio::process::Command::new(root.join("craft/jet-craft-codex"))
            .arg("--socket").arg(&socket).kill_on_drop(true)).unwrap();
        let (mut reader, mut writer) = accept_craft(&socket, run).await;
        command(&mut writer, &json!({"kind":"recover","id":run,"helper_socket":root.join("h.sock"),"source_offset":offset,"checkpoint":checkpoint})).await;
        assert!(reader.read().await.is_err(), "uncertain native input must require reconciliation");
    }).await.unwrap();
}

#[tokio::test]
async fn recovery_refuses_ambiguous_startup_input() {
	recover_startup(StartupCheckpoint::Previous).await;
}

#[tokio::test]
async fn a_committed_startup_checkpoint_recovers_when_its_acknowledgement_was_lost()
 {
	recover_startup(StartupCheckpoint::Committed).await;
}

enum StartupCheckpoint {
	Previous,
	Committed,
}

async fn recover_startup(boundary: StartupCheckpoint) {
	tokio::time::timeout(Duration::from_secs(20), async {
        let (root, run, harness) = workspace();
        let _helper = start_helper(&root, run, &harness).await;
        let (socket, mut craft) = start_craft(&root, &harness);
        let (mut reader, mut writer) = connect_craft(&socket, run, json!({
            "native_conversation":"imported-thread","model":"pinned-model","version":{"major":1,"minor":10}
        })).await;
        command(&mut writer, &json!({"kind":"start","id":run,"text":"resume","helper_socket":root.join("h.sock")})).await;
        let mut previous = (0, String::new());
        let mut thread_seen = false;
        let committed = loop {
            match event(&mut reader).await {
                CraftEvent::Output { native_event, .. } => {
                    let native: Value = serde_json::from_str(native_event.get()).unwrap();
                    thread_seen |= native.pointer("/result/thread/id").is_some();
                }
                CraftEvent::Progress { source_offset, checkpoint } => {
                    if thread_seen { break (source_offset, checkpoint); }
                    previous = (source_offset, checkpoint);
                    command(&mut writer, &json!({"kind":"acknowledge","source_offset":source_offset})).await;
                }
                _ => {}
            }
        };
        while !root.join("resumed-turn").exists() { tokio::time::sleep(Duration::from_millis(5)).await; }
        craft.kill().await.unwrap();
        drop(reader); drop(writer);
        std::fs::remove_file(&socket).unwrap();
        let _craft = spawn_serialized(tokio::process::Command::new(root.join("craft/jet-craft-codex"))
            .arg("--socket").arg(&socket).kill_on_drop(true)).unwrap();
        let (mut reader, mut writer) = accept_craft(&socket, run).await;
        let (offset, checkpoint) = match boundary { StartupCheckpoint::Previous => previous, StartupCheckpoint::Committed => committed };
        command(&mut writer, &json!({"kind":"recover","id":run,"helper_socket":root.join("h.sock"),"source_offset":offset,"checkpoint":checkpoint})).await;
        match boundary {
            StartupCheckpoint::Previous => assert!(reader.read().await.is_err()),
            StartupCheckpoint::Committed => {
                assert!(matches!(event(&mut reader).await, CraftEvent::RunRecovered { source_offset, .. } if source_offset == offset));
                let mut events = vec![];
                loop {
                    events.extend(batch(&mut reader, &mut writer).await);
                    if events.iter().any(|event| matches!(event, CraftEvent::RunEnded { .. })) { break; }
                }
                assert_eq!(completions(&events), vec![(run.to_string(), "imported-thread".into())]);
            }
        }
    }).await.unwrap();
}

#[tokio::test]
async fn no_visa_mcp_calls_reach_the_host_and_return_its_outcome() {
	tokio::time::timeout(Duration::from_secs(30), async {
        let (root, run, harness) = workspace();
        std::fs::write(root.join("remote"), "").unwrap();
        let mut helper = start_helper(&root, run, &harness).await;
        let (socket, mut craft) = start_craft(&root, &harness);
        let (mut reader, mut writer) = accept_craft(&socket, run).await;
        command(&mut writer, &json!({"kind":"configure_remote_tools", "selection": {
            "origin_plane_id":Uuid::nil(),"account_binding_id":Uuid::nil(),"conversation_id":run,
            "jet_equivalent":["files"],"native_unavailable":["native_checkpoints"],
            "destinations":[{"plane_id":Uuid::nil(),"workspace_id":Uuid::nil(),"ssh_endpoint":"destination"}]
        }})).await;
        command(&mut writer, &json!({"kind":"start","id":run,"text":"remote",
            "helper_socket":root.join("h.sock")})).await;
        let mut calls = vec![];
        let mut ended = false;
        loop {
            match event(&mut reader).await {
                CraftEvent::Progress { source_offset, .. } => { command(&mut writer, &json!({"kind":"acknowledge","source_offset":source_offset})).await; if ended { break; } },
                CraftEvent::RemoteTool { call } => {
                    calls.push(serde_json::to_value(&call).unwrap());
                    command(&mut writer, &json!({"kind":"remote_tool_result","operation_id":call.operation_id,
                        "outcome":{"type":"completed","result":{"type":"file","content":"destination content"}}})).await;
                }
                CraftEvent::RunEnded { exit_code } => { assert_eq!(exit_code, Some(0)); ended = true; }
                _ => {}
            }
        }
        assert_eq!(calls, vec![json!({"operation_id":Uuid::nil(),"destination_plane_id":Uuid::nil(),
            "workspace_id":Uuid::nil(),"action":{"type":"read_file","path":"note.txt"}})]);
        assert!(helper.wait().await.unwrap().success());
        craft.start_kill().unwrap();
    }).await.unwrap();
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
	recorded_batch(reader, writer).await.0
}

async fn recorded_batch(
	reader: &mut Reader,
	writer: &mut Writer,
) -> (Vec<CraftEvent>, u64, String) {
	tokio::time::timeout(Duration::from_secs(3), async {
		let mut events = vec![];
		loop {
			match event(reader).await {
				CraftEvent::Progress {
					source_offset,
					checkpoint,
				} => {
					command(
						writer,
						&json!({
							"kind": "acknowledge", "source_offset": source_offset,
						}),
					)
					.await;
					return (events, source_offset, checkpoint);
				}
				other => events.push(other),
			}
		}
	})
	.await
	.expect("the native source makes bounded progress")
}

// Serializing executable writes with forks avoids Linux ETXTBSY when a
// sibling child briefly inherits a descriptor for a Craft being installed.
static PROCESS_IMAGES: Mutex<()> = Mutex::new(());
fn process_images() -> MutexGuard<'static, ()> {
	PROCESS_IMAGES
		.lock()
		.unwrap_or_else(PoisonError::into_inner)
}
fn spawn_serialized(
	command: &mut tokio::process::Command,
) -> std::io::Result<tokio::process::Child> {
	let _guard = process_images();
	command.spawn()
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
	{
		let _guard = process_images();
		std::fs::write(&harness, script).unwrap();
	}
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
	let mut child = spawn_serialized(
		tokio::process::Command::new(
			PathBuf::from(env!("CARGO_BIN_EXE_jet-craft-codex"))
				.with_file_name("jetfueld"),
		)
		.args(["run", "--config"])
		.arg(path)
		.kill_on_drop(true),
	)
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
	{
		let _guard = process_images();
		std::fs::copy(env!("CARGO_BIN_EXE_jet-craft-codex"), &installed)
			.unwrap();
	}
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
	let child = spawn_serialized(
		tokio::process::Command::new(&installed)
			.arg("--socket")
			.arg(&socket)
			.kill_on_drop(true),
	)
	.unwrap();
	(socket, child)
}

async fn accept_craft(socket: &Path, run: Uuid) -> (Reader, Writer) {
	connect_craft(socket, run, Value::Null).await
}

async fn connect_craft(
	socket: &Path,
	run: Uuid,
	resume: Value,
) -> (Reader, Writer) {
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
		"protocol": {"family": "craft", "versions": [{"major": 1, "minor": 10}], "capabilities": ["runs", "actions", "resume"]},
		"specification": {"family": "specification", "versions": [{"major": 1, "minor": 0}]},
		"execution_id": run, "resume": resume,
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
		ProtocolVersion {
			major: 1,
			minor: 10
		}
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
	if Path::new("remote").exists() {
		let server = &thread["params"]["config"]["mcp_servers.jet"];
		let mut mcp =
			std::process::Command::new(server["command"].as_str().unwrap())
				.args(
					server["args"]
						.as_array()
						.unwrap()
						.iter()
						.map(|arg| arg.as_str().unwrap()),
				)
				.stdin(std::process::Stdio::piped())
				.stdout(std::process::Stdio::piped())
				.spawn()
				.unwrap();
		let mut input = mcp.stdin.take().unwrap();
		let mut output =
			std::io::BufReader::new(mcp.stdout.take().unwrap()).lines();
		writeln!(input, "{}", json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"conformance","version":"1"}}})).unwrap();
		assert_eq!(
			read(&mut output)["result"]["capabilities"],
			json!({"tools":{}})
		);
		writeln!(
			input,
			"{}",
			json!({"jsonrpc":"2.0","method":"notifications/initialized"})
		)
		.unwrap();
		writeln!(
			input,
			"{}",
			json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})
		)
		.unwrap();
		let list = read(&mut output);
		assert_eq!(list["result"]["tools"][0]["name"], "remote");
		assert!(
			list["result"]["tools"][0]["description"]
				.as_str()
				.unwrap()
				.contains("destination")
		);
		for (id, arguments) in [
			(2, json!({})),
			(
				3,
				json!({"operation_id":Uuid::nil(),"destination_plane_id":Uuid::nil(),"workspace_id":Uuid::nil(),"action":{"type":"read_file","path":"note.txt"}}),
			),
		] {
			writeln!(input, "{}", json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"remote","arguments":arguments}})).unwrap();
			let response = read(&mut output);
			assert_eq!(response["id"], id);
			assert_eq!(response["result"]["isError"], id == 2);
			if id == 3 {
				assert_eq!(
					serde_json::from_str::<Value>(
						response["result"]["content"][0]["text"]
							.as_str()
							.unwrap()
					)
					.unwrap(),
					json!({"type":"completed","result":{"type":"file","content":"destination content"}})
				);
			}
		}
		drop(input);
		assert!(mcp.wait().unwrap().success());
		reply(
			&thread,
			json!({"thread":{"id":"remote-thread"},"model":"configured-model"}),
		);
		let turn = read(&mut lines);
		assert_eq!(turn["params"]["model"], "configured-model");
		notify(
			"turn/completed",
			json!({"threadId":"remote-thread","turn":{"id":"remote-turn","status":"completed"}}),
		);
		return;
	}
	if thread["method"] == "thread/resume" {
		assert_eq!(
			thread,
			json!({"id":1,"method":"thread/resume","params":{
            "threadId":"imported-thread", "model":"pinned-model"}})
		);
		if Path::new("wrong-model").exists() {
			reply(
				&thread,
				json!({"thread":{"id":"imported-thread"},"model":"changed-model"}),
			);
		} else {
			reply(
				&thread,
				json!({"thread":{"id":"imported-thread"},"model":"pinned-model"}),
			);
		}
		let turn = read(&mut lines);
		std::fs::write("resumed-turn", "received").unwrap();
		assert_eq!(
			turn,
			json!({"id":2,"method":"turn/start","params":{
            "threadId":"imported-thread","model":"pinned-model","input":[{"type":"text","text":"resume"}]}})
		);
		notify(
			"turn/completed",
			json!({"threadId":"imported-thread", "turn":{"id":"resumed","status":"completed"}}),
		);
		return;
	}
	assert_eq!(
		thread,
		json!({"id": 1, "method": "thread/start", "params": {}}),
	);
	reply(&thread, json!({"thread": {"id": "thread-native-1"}}));
	if Path::new("recovery").exists() {
		for (id, text) in [(2, "first"), (3, "continue"), (4, "continue")] {
			let turn = read(&mut lines);
			assert_eq!(
				turn,
				json!({"id":id,"method":"turn/start","params":{
                "threadId":"thread-native-1","input":[{"type":"text","text":text}]}})
			);
			reply(&turn, json!({"turn":{"id":format!("native-{id}")}}));
			notify(
				"turn/completed",
				json!({"threadId":"thread-native-1", "turn":{"id":format!("native-{id}"),"status":"completed"}}),
			);
		}
		return;
	}

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
