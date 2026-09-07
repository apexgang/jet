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
		let mut helper = start_helper(&root, run, &harness);
		let (craft_socket, mut craft) = start_craft(&root, &harness);
		let (mut reader, mut writer, ready) =
			accept_craft(&craft_socket, run).await;
		assert_eq!(
			ready.protocol.version,
			ProtocolVersion { major: 1, minor: 4 }
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
					3 => {
						command(
							&mut writer,
							&json!({
								"kind": "turn", "id": "turn-4", "text": "finish",
							}),
						)
						.await;
					}
					_ => {}
				}
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
			],
			"every turn is answered under the pinned native Conversation"
		);
		assert_eq!(
			outcomes(&events),
			vec![
				TurnOutcome::Completed,
				TurnOutcome::Interrupted,
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
			]
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

fn start_helper(
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
	tokio::process::Command::new(
		PathBuf::from(env!("CARGO_BIN_EXE_jet-craft-claude"))
			.with_file_name("jetfueld"),
	)
	.args(["run", "--config"])
	.arg(path)
	.kill_on_drop(true)
	.spawn()
	.unwrap()
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
		"id = \"claude-code\"\nharness = \"claude-code\"\nschema = {{ major = 1, minor = 0 }}\nbroker_permissions = []\nhost_access = [{{ kind = \"executable\", name = {:?} }}]\nfeatures = [{{ name = \"turns\", required = true }}]\n\n[protocol]\nfamily = \"craft\"\nversions = [{{ major = 1, minor = 4 }}]\ncapabilities = [\"runs\"]\n",
		harness.to_str().unwrap()
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

async fn accept_craft(
	socket: &Path,
	run: Uuid,
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
		"protocol": {"family": "craft", "versions": [{"major": 1, "minor": 4}], "capabilities": ["runs"]},
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
/// in both directions, for as long as its input stays open.
#[test]
#[ignore = "invoked as a real Claude Code Harness owned by jetfueld"]
fn claude_double() {
	use std::io::{BufRead, Write};
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

	let mut slow = false;
	for line in std::io::stdin().lock().lines() {
		let event: Value = serde_json::from_str(&line.unwrap()).unwrap();
		let mut out = std::io::stdout().lock();
		// Cancellation abandons the turn in flight and answers it.
		if event["type"] == "control_request" {
			if slow {
				slow = false;
				writeln!(out, "{}", result(&session)).unwrap();
				out.flush().unwrap();
			}
			continue;
		}
		let text = event["message"]["content"][0]["text"].as_str().unwrap();
		writeln!(
			out,
			"{}",
			json!({
				"type": "system", "subtype": "init",
				"claude_code_version": "2.1.263", "session_id": session,
			})
		)
		.unwrap();
		writeln!(out, "{}", assistant(&session)).unwrap();
		if text == "second" {
			writeln!(
				out,
				"{}",
				json!({
					"type": "rate_limit_event", "session_id": session,
					"rate_limit_info": {"status": "rejected", "utilization": 1.0},
				})
			)
			.unwrap();
		}
		if text == "slow" {
			slow = true;
			out.flush().unwrap();
			continue;
		}
		writeln!(out, "{}", result(&session)).unwrap();
		out.flush().unwrap();
		if text == "finish" {
			return;
		}
	}
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
		"usage": {"input_tokens": 10, "output_tokens": 90},
	})
	.to_string()
}
