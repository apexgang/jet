//! The Craft against real processes: a host, this Craft, a real `jetfueld`,
//! and a Harness that speaks the Claude Code native protocol.
use jet_protocol::*;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{
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
async fn a_conversation_runs_turns_and_ends_through_the_native_protocol() {
	tokio::time::timeout(Duration::from_secs(180), async {
		let (root, run, harness) = workspace();
		let mut helper = start_helper(&root, run, &harness).await;
		let (craft_socket, mut craft) = start_craft(&root, &harness);
		let (mut reader, mut writer, ready) =
			accept_craft(&craft_socket, run).await;
		assert_eq!(
			ready.protocol.version,
			ProtocolVersion { major: 1, minor: 8 }
		);

		command(
			&mut writer,
			&json!({
				"kind": "start", "id": run.to_string(), "text": "first",
				"helper_socket": root.join("h.sock"),
			}),
		)
		.await;

		let mut events = vec![];
		let mut turns = 0;
		let mut answered = false;
		loop {
			events.extend(batch(&mut reader, &mut writer).await);
			let completed = events
				.iter()
				.filter(|event| matches!(event, CraftEvent::Completed { .. }))
				.count();
			if completed > turns {
				turns = completed;
				match turns {
					1 => {
						command(
							&mut writer,
							&json!({
								"kind": "turn", "id": "turn-2", "text": "second",
							}),
						)
						.await;
					}
					// A turn the Harness will not finish on its own, so the
					// only way it ends is native cancellation.
					2 => {
						command(
							&mut writer,
							&json!({
								"kind": "turn", "id": "turn-3", "text": "slow",
							}),
						)
						.await;
						command(
							&mut writer,
							&json!({
								"kind": "interrupt", "id": "turn-3",
							}),
						)
						.await;
					}
					// Using a tool needs an approval, so this turn cannot
					// finish until Jet answers the request behind it.
					3 => {
						command(
							&mut writer,
							&json!({
								"kind": "turn", "id": "turn-4", "text": "write",
							}),
						)
						.await;
					}
					4 => {
						command(
							&mut writer,
							&json!({
								"kind": "turn", "id": "turn-5", "text": "finish",
							}),
						)
						.await;
					}
					_ => {}
				}
			}
			if !answered
				&& events.iter().any(|event| {
					matches!(
						event,
						CraftEvent::Activity {
							activity: RunActivity::WaitingForApproval
						}
					)
				}) {
				answered = true;
				command(
					&mut writer,
					&json!({
						"kind": "action", "id": "action-1",
						"action": {
							"kind": "approval",
							"request_id": asked(&events),
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

		// The Harness was launched through its own structured protocol, and
		// its Conversation identity was pinned before any output existed.
		let arguments: Vec<String> = serde_json::from_slice(
			&std::fs::read(root.join("arguments.json")).unwrap(),
		)
		.unwrap();
		let arguments = arguments.join(" ");
		assert!(
			arguments.contains("--print")
				&& arguments.contains("--input-format stream-json")
				&& arguments.contains("--output-format stream-json")
				&& arguments.contains(&format!("--session-id {run}")),
			"unexpected native launch: {arguments}"
		);

		assert!(matches!(
			events.first(),
			Some(CraftEvent::RunStarted { helper_pid, harness_pid })
				if helper_pid != harness_pid
		));
		assert_eq!(
			completions(&events),
			vec![
				(run.to_string(), run.to_string()),
				("turn-2".into(), run.to_string()),
				("turn-3".into(), run.to_string()),
				("turn-4".into(), run.to_string()),
				("turn-5".into(), run.to_string()),
			],
			"every turn is answered under the pinned native Conversation"
		);
		assert_eq!(
			outcomes(&events),
			vec![
				TurnOutcome::Completed,
				TurnOutcome::Interrupted,
				TurnOutcome::Completed,
				TurnOutcome::Completed,
			]
		);
		assert_eq!(
			activities(&events),
			vec![
				RunActivity::Working,
				RunActivity::Working,
				RunActivity::WaitingForQuota,
				RunActivity::Working,
				RunActivity::Working,
				RunActivity::WaitingForApproval,
				RunActivity::Working,
				RunActivity::Working,
			]
		);
		// Consumption is reported per Model where the Harness breaks it
		// down, under an identity that makes a repeat replace it rather
		// than add to it (ADR-0023).
		assert!(reported(&events).contains(&CraftUsage::Observed {
			observed: CraftObservedUsage {
				measurement: CraftUsageMeasurement::Turn {
					turn: run.to_string(),
					native_usage_id: Some(format!("{run}:claude-opus-5")),
				},
				model: Some("claude-opus-5".into()),
				estimation: CraftUsageEstimation::Measured,
				finality: CraftUsageFinality::Final,
				tokens: CraftUsageTokens {
					input: 14,
					cached_input: 6,
					output: 90,
					reasoning: 0,
				},
			},
		}));
		// A rate-limit event states how full the unified window is, whether
		// or not the Harness could still proceed under it.
		assert!(reported(&events).contains(&CraftUsage::Quota {
			quota: CraftQuotaWindow {
				window: "unified".into(),
				scope: CraftQuotaScope::ProviderAccount,
				unit: CraftQuotaUnit::Share,
				used: 10_000,
				limit: Some(10_000),
				window_seconds: None,
				resets_in_seconds: None,
				estimation: CraftUsageEstimation::Measured,
				finality: CraftUsageFinality::Interim,
			},
		}));
		// The decision reached the Harness as its own answer, carrying the
		// input it was shown rather than one this Craft edited.
		assert_eq!(
			std::fs::read_to_string(root.join("decision.json")).unwrap(),
			json!({
				"behavior": "allow",
				"updatedInput": {"file_path": "note.txt"},
			})
			.to_string()
		);
		assert!(matches!(
			events.last(),
			Some(CraftEvent::RunEnded { exit_code: Some(0) })
		));

		// The native event survives whole, including an integer no JSON
		// number type in a GUI could hold, and its views accompany it.
		let assistant = events
			.iter()
			.find_map(|event| match event {
				CraftEvent::Output {
					native_event,
					presentation,
				} if native_event.get().contains("9007199254740993") => {
					Some((native_event.get().to_owned(), presentation.clone()))
				}
				_ => None,
			})
			.expect("the assistant event reached the host");
		assert!(assistant.0.contains("\"native_integer\":9007199254740993"));
		assert_eq!(
			assistant
				.1
				.iter()
				.map(|block| block.known().unwrap())
				.collect::<Vec<_>>(),
			vec![
				Some(Presentation::Text {
					text: "Considering the change".into()
				}),
				Some(Presentation::Markdown {
					text: "Done, **finally**.".into()
				}),
				Some(Presentation::Text {
					text: "Write".into()
				}),
			]
		);

		assert!(helper.wait().await.unwrap().success());
		craft.start_kill().unwrap();
	})
	.await
	.unwrap();
}

/// The identity of the permission request the Harness is waiting on, read
/// from the native event exactly as a client would.
fn asked(events: &[CraftEvent]) -> String {
	events
		.iter()
		.find_map(|event| {
			let CraftEvent::Output { native_event, .. } = event else {
				return None;
			};
			let native: Value =
				serde_json::from_str(native_event.get()).ok()?;
			(native.pointer("/request/message/method")?.as_str()?
				== "tools/call")
				.then(|| native["request_id"].as_str())?
				.map(str::to_owned)
		})
		.expect("the Harness asked through this Craft's own server")
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

fn outcomes(events: &[CraftEvent]) -> Vec<TurnOutcome> {
	events
		.iter()
		.filter_map(|event| match event {
			CraftEvent::TurnEnded { outcome } => Some(*outcome),
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

fn activities(events: &[CraftEvent]) -> Vec<RunActivity> {
	events
		.iter()
		.filter_map(|event| match event {
			CraftEvent::Activity { activity } => Some(*activity),
			_ => None,
		})
		.collect()
}

/// Read one source record's Events, acknowledge it, and return them.
async fn batch(reader: &mut Reader, writer: &mut Writer) -> Vec<CraftEvent> {
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
}

fn workspace() -> (PathBuf, Uuid, PathBuf) {
	let root = tempfile::tempdir_in("/tmp")
		.unwrap()
		.keep()
		.canonicalize()
		.unwrap();
	std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
		.unwrap();
	// The host launches this Craft with a private endpoint only, so the
	// Harness the Craft may launch comes from its accepted declaration.
	let harness = root.join("claude");
	let script = format!(
		"#!/bin/sh\nJET_CLAUDE_ARGUMENTS=\"$*\"\nexport JET_CLAUDE_ARGUMENTS\nexec {} --ignored --exact --nocapture claude_double\n",
		std::env::current_exe().unwrap().display()
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
	std::io::Write::write_all(&mut file, &encode_control(&config).unwrap())
		.unwrap();
	let mut child = tokio::process::Command::new(
		PathBuf::from(env!("CARGO_BIN_EXE_jet-craft-claude"))
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

/// Install the Craft the way a Plane owner would: the executable beside the
/// declaration that names the Harness it is allowed to launch.
fn start_craft(
	root: &Path,
	harness: &Path,
) -> (PathBuf, tokio::process::Child) {
	let installed = root.join("craft/jet-craft-claude");
	std::fs::create_dir_all(installed.with_file_name(".jet")).unwrap();
	std::fs::copy(env!("CARGO_BIN_EXE_jet-craft-claude"), &installed).unwrap();
	let declaration = format!(
		"id = \"claude-code\"\nharness = \"claude-code\"\nschema = {{ major = 1, minor = 0 }}\nbroker_permissions = []\nhost_access = [{{ kind = \"executable\", name = {:?} }}]\nfeatures = [{{ name = \"turns\", required = true }}, {{ name = \"actions\", required = true }}]\n\n[protocol]\nfamily = \"craft\"\nversions = [{{ major = 1, minor = 4 }}]\ncapabilities = [\"runs\", \"actions\"]\n",
		harness.to_str().unwrap()
	);
	let declaration = declaration
		.replace(
			"broker_permissions = []",
			"broker_permissions = [\"remote_tools\"]",
		)
		.replace("features = [", "features = [{ name = \"remote_tools\" }, ")
		.replace("minor = 4", "minor = 8");
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

async fn accept_craft(
	socket: &Path,
	run: Uuid,
) -> (Reader, Writer, CraftReady) {
	accept_craft_at_minor(socket, run, 8).await
}
async fn accept_craft_at_minor(
	socket: &Path,
	run: Uuid,
	minor: u32,
) -> (Reader, Writer, CraftReady) {
	let mut stream = loop {
		match UnixStream::connect(socket).await {
			Ok(stream) => break stream,
			Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
		}
	};
	stream.write_all(b"jet-craft\n").await.unwrap();
	let (read, write) = stream.into_split();
	let mut reader = FrameReader::new(read);
	let mut writer = FrameWriter::new(write);
	let hello = json!({
		"protocol": {"family": "craft", "versions": [{"major": 1, "minor": minor}], "capabilities": ["runs", "actions"]},
		"specification": {"family": "specification", "versions": [{"major": 1, "minor": 0}]},
		"execution_id": run,
		"resume": Value::Null,
	});
	writer
		.write(&Frame::control(encode_control(&hello).unwrap()))
		.await
		.unwrap();
	let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
		panic!("control")
	};
	let ready: CraftReady = decode_control(&payload).unwrap();
	reader.enable_multiplexing();
	writer.enable_multiplexing();
	(reader, writer, ready)
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

/// A Harness speaking the Claude Code native protocol: newline-delimited JSON
/// in both directions, for as long as its input stays open. It reaches its
/// SDK MCP server by asking the Craft, which is also how it asks permission.
#[test]
#[ignore = "invoked as a real Claude Code Harness owned by jetfueld"]
fn claude_double() {
	let arguments: Vec<String> = std::env::var("JET_CLAUDE_ARGUMENTS")
		.unwrap()
		.split_whitespace()
		.map(str::to_owned)
		.collect();
	std::fs::write(
		"arguments.json",
		serde_json::to_string(&arguments).unwrap(),
	)
	.unwrap();
	let session = arguments
		.windows(2)
		.find(|pair| pair[0] == "--session-id")
		.map(|pair| pair[1].clone())
		.expect("the Craft pins the native Conversation identity");
	let permission = arguments
		.windows(2)
		.find(|pair| pair[0] == "--permission-prompt-tool")
		.map(|pair| pair[1].clone())
		.expect("the Craft answers permission requests itself");

	let mut input = std::io::BufRead::lines(std::io::stdin().lock());
	let mut slow = false;
	let mut served = false;
	while let Some(line) = input.next() {
		let event: Value = serde_json::from_str(&line.unwrap()).unwrap();
		// Cancellation abandons the turn in flight and answers it, which is
		// the only way a turn the Harness will not finish ever ends.
		if event["type"] == "control_request" {
			if event["request"]["subtype"] == "interrupt" && slow {
				slow = false;
				emit(
					&serde_json::from_str::<Value>(&result(&session)).unwrap(),
				);
			}
			continue;
		}
		// The Craft registers its server before any turn runs.
		if event["type"] == "control_response" {
			continue;
		}
		let text = event["message"]["content"][0]["text"].as_str().unwrap();
		if !served {
			served = true;
			// The server behind the permission tool has to be usable before
			// it can be trusted to answer, so it is opened like any other.
			assert_eq!(
				ask(
					&mut input,
					"mcp-1",
					json!({
						"jsonrpc": "2.0", "id": 0, "method": "initialize",
					})
				)["serverInfo"]["name"],
				json!("jet")
			);
			assert_eq!(
				ask(
					&mut input,
					"mcp-2",
					json!({
						"jsonrpc": "2.0", "id": 1, "method": "tools/list",
					})
				)["tools"][0]["name"],
				json!(permission.rsplit("__").next().unwrap())
			);
		}
		emit(&json!({
			"type": "system", "subtype": "init",
			"claude_code_version": "2.1.263", "session_id": session,
		}));
		emit(&serde_json::from_str::<Value>(&assistant(&session)).unwrap());
		if text == "remote" {
			let tools = ask(
				&mut input,
				"remote-list",
				json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
			);
			assert!(
				tools["tools"]
					.as_array()
					.unwrap()
					.iter()
					.any(|t| t["name"] == "remote")
			);
			let call: Value = serde_json::from_slice(
				&std::fs::read("remote-call.json").unwrap(),
			)
			.unwrap();
			let result = ask(
				&mut input,
				"remote-call",
				json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"remote","arguments":call}}),
			);
			std::fs::write(
				"remote-result.json",
				serde_json::to_vec(&result).unwrap(),
			)
			.unwrap();
		}
		if text == "second" {
			emit(&json!({
				"type": "rate_limit_event", "session_id": session,
				"rate_limit_info": {"status": "rejected", "utilization": 1.0},
			}));
		}
		if text == "slow" {
			slow = true;
			continue;
		}
		// Using a tool needs permission, and nothing proceeds until the
		// answer to that exact request arrives.
		if text == "write" {
			let decision = ask(
				&mut input,
				"perm-1",
				json!({
					"jsonrpc": "2.0", "id": 2, "method": "tools/call",
					"params": {
						"name": permission.rsplit("__").next().unwrap(),
						"arguments": {
							"tool_name": "Write",
							"input": {"file_path": "note.txt"},
							"tool_use_id": "toolu_1",
						},
					},
				}),
			);
			std::fs::write(
				"decision.json",
				decision["content"][0]["text"].as_str().unwrap(),
			)
			.unwrap();
		}
		emit(&serde_json::from_str::<Value>(&result(&session)).unwrap());
		if text == "finish" {
			return;
		}
	}
}

/// Ask the Craft's own MCP server one message and wait for its answer, the
/// way the Harness reaches an SDK-hosted server.
fn ask(
	input: &mut impl Iterator<Item = std::io::Result<String>>,
	request_id: &str,
	message: Value,
) -> Value {
	emit(&json!({
		"type": "control_request", "request_id": request_id,
		"request": {
			"subtype": "mcp_message", "server_name": "jet",
			"message": message,
		},
	}));
	for line in input {
		let event: Value = serde_json::from_str(&line.unwrap()).unwrap();
		if event["type"] == "control_response"
			&& event["response"]["request_id"] == request_id
		{
			return event["response"]["response"]["mcp_response"]["result"]
				.clone();
		}
		// A turn cancelled while this Craft answers still ends the turn.
		if event["type"] == "control_request" {
			continue;
		}
	}
	panic!("the Craft left {request_id} unanswered")
}

fn emit(event: &Value) {
	use std::io::Write;
	let mut out = std::io::stdout().lock();
	writeln!(out, "{event}").unwrap();
	out.flush().unwrap();
}

fn assistant(session: &str) -> String {
	json!({
		"type": "assistant", "session_id": session,
		"native_integer": 9_007_199_254_740_993_i64,
		"message": {"role": "assistant", "content": [
			{"type": "thinking", "thinking": "Considering the change"},
			{"type": "text", "text": "Done, **finally**."},
			{"type": "tool_use", "id": "toolu_1", "name": "Write", "input": {"file_path": "note.txt"}},
		]},
	})
	.to_string()
}

fn result(session: &str) -> String {
	json!({
		"type": "result", "subtype": "success", "session_id": session,
		"total_cost_usd": 0.01, "num_turns": 1,
		"usage": {"input_tokens": 10, "output_tokens": 90,
			"cache_creation_input_tokens": 4, "cache_read_input_tokens": 6},
		"modelUsage": {"claude-opus-5": {"inputTokens": 10, "outputTokens": 90,
			"cacheCreationInputTokens": 4, "cacheReadInputTokens": 6}},
	})
	.to_string()
}

#[tokio::test]
async fn native_mcp_call_waits_for_the_jet_remote_result_before_acknowledging_source()
 {
	tokio::time::timeout(Duration::from_secs(20), async {
        let (root, run, harness) = workspace();
        let mut helper = start_helper(&root, run, &harness).await;
        let (socket, mut craft) = start_craft(&root, &harness);
        let (mut reader, mut writer, ready) = accept_craft_at_minor(&socket, run, 6).await;
        assert_eq!(ready.protocol.version, ProtocolVersion { major: 1, minor: 6 });
        let plane = Uuid::new_v4(); let workspace = Uuid::new_v4(); let operation = Uuid::new_v4();
        let call = json!({"operation_id":operation,"destination_plane_id":plane,"workspace_id":workspace,"action":{"type":"read_file","path":"README.md"}});
        std::fs::write(root.join("remote-call.json"), serde_json::to_vec(&call).unwrap()).unwrap();
        command(&mut writer, &json!({"kind":"configure_remote_tools","selection":{
            "origin_plane_id":Uuid::new_v4(),"conversation_id":Uuid::new_v4(),"account_binding_id":Uuid::new_v4(),
            "destinations":[{"plane_id":plane,"workspace_id":workspace,"ssh_endpoint":"paired"}],"jet_equivalent":["files"],"native_unavailable":["sandbox_internals"]
        }})).await;
        command(&mut writer, &json!({"kind":"start","id":run,"text":"remote","helper_socket":root.join("h.sock")})).await;
        let mut forwarded = false;
        loop {
            match event(&mut reader).await {
                CraftEvent::RemoteTool { call:actual } => {
                    assert_eq!(serde_json::to_value(actual).unwrap(), call);
                    forwarded = true;
                    command(&mut writer, &json!({"kind":"remote_tool_result","operation_id":operation,"outcome":{"type":"completed","result":{"type":"file","content":"destination evidence"}}})).await;
                },
                CraftEvent::Progress { source_offset, .. } => command(&mut writer, &json!({"kind":"acknowledge","source_offset":source_offset})).await,
                CraftEvent::Completed { .. } => break,
                _ => {},
            }
        }
        assert!(forwarded);
        let result:Value = serde_json::from_slice(&std::fs::read(root.join("remote-result.json")).unwrap()).unwrap();
        assert_eq!(serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(), json!({"type":"completed","result":{"type":"file","content":"destination evidence"}}));
        craft.kill().await.unwrap(); helper.kill().await.unwrap(); std::fs::remove_dir_all(root).unwrap();
    }).await.unwrap();
}
