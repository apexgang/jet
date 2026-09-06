//! A real subprocess exercises the SDK over a private local byte stream.
use jet_craft_sdk::{CraftConnection, parse_specification};
use jet_protocol::{
	CraftCommand, CraftEvent, CraftReady, Frame, FrameReader, FrameWriter,
	Presentation, PresentationAction, PresentationBlock, decode_control,
	encode_control,
};
use pretty_assertions::assert_eq;
use std::{process::Stdio, time::Duration};
use tokio::{
	io::AsyncWriteExt,
	net::{UnixListener, UnixStream},
	process::Command,
	time::timeout,
};

const SPEC: &str = include_str!("fixtures/craft-spec.toml");

#[tokio::test]
async fn incompatible_restart_is_rejected_before_any_harness_output() {
	timeout(Duration::from_secs(20), async {
		let temp = tempfile::tempdir().unwrap();
		let socket = temp.path().join("craft.sock");
		let listener = UnixListener::bind(&socket).unwrap();
		let mut child = Command::new(std::env::current_exe().unwrap()).args(["--ignored", "--exact", "fake_craft_process"]).env("JET_TEST_CRAFT_SOCKET", &socket).env("JET_TEST_REJECTION", "1").stdout(Stdio::null()).stderr(Stdio::inherit()).kill_on_drop(true).spawn().unwrap();
		let (mut stream, _) = listener.accept().await.unwrap();
		stream.write_all(b"jet-craft\n").await.unwrap();
		let (read, write) = stream.into_split();
		let mut reader = FrameReader::new(read);
		let mut writer = FrameWriter::new(write);
		let hello = serde_json::json!({"protocol":{"family":"craft","versions":[{"major":1,"minor":0}],"capabilities":["resume"]},"specification":{"family":"specification","versions":[{"major":1,"minor":0}]},"execution_id":uuid::Uuid::new_v4(),"resume":{"version":{"major":2,"minor":0},"native_conversation":"native-42"}});
		writer.write(&Frame::control(encode_control(&hello).unwrap())).await.unwrap();
		let Frame::Control { payload, .. } = reader.read().await.unwrap() else { panic!("control") };
		assert_eq!(decode_control::<serde_json::Value>(&payload).unwrap(), serde_json::json!({"kind":"rejected","code":"incompatible"}));
		assert!(matches!(reader.read().await, Err(jet_protocol::FrameError::Closed)));
		assert!(child.wait().await.unwrap().success());
	}).await.unwrap();
}

#[tokio::test]
async fn subprocess_retains_native_output_handles_actions_and_resumes() {
	timeout(Duration::from_secs(20), async {
		let execution_id = uuid::Uuid::new_v4();
		let mut resume = serde_json::Value::Null;
		for attempt in 0..2 {
			let temp = tempfile::tempdir().unwrap();
			let socket = temp.path().join("craft.sock");
			let listener = UnixListener::bind(&socket).unwrap();
			let mut child = Command::new(std::env::current_exe().unwrap()).args(["--ignored", "--exact", "fake_craft_process"]).env("JET_TEST_CRAFT_SOCKET", &socket).stdout(Stdio::null()).stderr(Stdio::inherit()).kill_on_drop(true).spawn().unwrap();
			let (mut stream, _) = listener.accept().await.unwrap();
			stream.write_all(b"jet-craft\n").await.unwrap();
			let (read, write) = stream.into_split();
			let mut reader = FrameReader::new(read);
			let mut writer = FrameWriter::new(write);
			let hello = serde_json::json!({"protocol":{"family":"craft","versions":[{"major":2,"minor":0},{"major":1,"minor":0}],"capabilities":["actions","resume"]},"specification":{"family":"specification","versions":[{"major":1,"minor":0}]},"execution_id":execution_id,"resume":resume});
			writer.write(&Frame::control(encode_control(&hello).unwrap())).await.unwrap();
			let Frame::Control { payload, .. } = reader.read().await.unwrap() else { panic!("control") };
			let ready: CraftReady = decode_control(&payload).unwrap();
			let mut expected_spec = parse_specification(SPEC).unwrap();
			expected_spec.schema.minor = 1;
			assert_eq!(ready.specification, expected_spec);
			assert_eq!(ready.protocol.version, jet_protocol::ProtocolVersion { major: 1, minor: 0 });
			reader.enable_multiplexing();
			writer.enable_multiplexing();
			let Frame::Control { payload, .. } = reader.read().await.unwrap() else { panic!("control") };
			let CraftEvent::Output { presentation, .. } = decode_control(&payload).unwrap() else { panic!("output") };
			let Some(Presentation::Actions { actions }) = presentation[0].known().unwrap() else { panic!("actions") };
			let action = serde_json::json!({"kind":"action","id":"action-1","action":{"kind":"invoke","action_id":actions[0].id,"input":{"path":"src"}}});
			writer.write(&Frame::control(encode_control(&action).unwrap())).await.unwrap();
			let Frame::Control { payload, .. } = reader.read().await.unwrap() else { panic!("control") };
			let CraftEvent::Output { native_event, presentation } = decode_control(&payload).unwrap() else { panic!("output") };
			assert_eq!(native_event.get(), r#"{ "native": "action", "large": 9007199254740993 }"#);
			assert_eq!(presentation[0].known().unwrap(), Some(Presentation::Text { text: if resume.is_null() { "fresh" } else { "native-42" }.into() }));
			let Frame::Control { payload, .. } = reader.read().await.unwrap() else { panic!("control") };
			let CraftEvent::Completed { id, native_conversation } = decode_control(&payload).unwrap() else { panic!("completion") };
			assert_eq!(id, "action-1");
			resume = serde_json::json!({"version":ready.protocol.version,"native_conversation":native_conversation});
			if attempt == 0 {
				child.kill().await.unwrap();
			} else {
				writer.write(&Frame::control(encode_control(&CraftCommand::Shutdown).unwrap())).await.unwrap();
				assert!(child.wait().await.unwrap().success());
			}
		}
	}).await.unwrap();
}

#[tokio::test]
#[ignore = "invoked as a separate Craft process by the conformance test"]
async fn fake_craft_process() {
	let socket = std::env::var_os("JET_TEST_CRAFT_SOCKET").unwrap();
	let count = std::env::var("JET_TEST_CONNECTIONS")
		.unwrap_or_else(|_| "1".into())
		.parse::<usize>()
		.unwrap();
	let mut executions = tokio::task::JoinSet::new();
	for _ in 0..count {
		let stream = UnixStream::connect(&socket).await.unwrap();
		executions.spawn(fake_execution(stream));
	}
	while let Some(result) = executions.join_next().await {
		result.unwrap();
	}
}

async fn fake_execution(stream: UnixStream) {
	let (read, write) = stream.into_split();
	let mut specification = parse_specification(SPEC).unwrap();
	specification.schema.minor = 1;
	let accepted = CraftConnection::accept(read, write, specification).await;
	if std::env::var_os("JET_TEST_REJECTION").is_some() {
		assert!(accepted.is_err());
		return;
	}
	let connection = accepted.unwrap();
	let text = connection
		.hello()
		.resume
		.as_ref()
		.map_or("fresh", |resume| resume.native_conversation.as_str())
		.to_owned();
	let (mut receiver, mut sender) = connection.split();
	let (commands, mut pending) = tokio::sync::mpsc::channel(1);
	let receiving = tokio::spawn(async move {
		while let Ok(command) = receiver.receive().await {
			if commands.send(command).await.is_err() {
				break;
			}
		}
	});
	sender
		.send(&CraftEvent::Output {
			native_event: serde_json::value::RawValue::from_string(
				r#"{"native":"actions"}"#.into(),
			)
			.unwrap(),
			presentation: vec![
				PresentationBlock::new(&Presentation::Actions {
					actions: vec![PresentationAction {
						id: "inspect".into(),
						label: "Inspect file".into(),
					}],
				})
				.unwrap(),
			],
		})
		.await
		.unwrap();
	while let Some(command) = pending.recv().await {
		match command {
			CraftCommand::Action {
				id,
				action: jet_protocol::CraftAction::Invoke { action_id, input },
			} => {
				assert_eq!(
					(action_id.as_str(), input),
					("inspect", serde_json::json!({"path":"src"}))
				);
				sender.send(&CraftEvent::Output { native_event: serde_json::value::RawValue::from_string(r#"{ "native": "action", "large": 9007199254740993 }"#.into()).unwrap(), presentation: vec![PresentationBlock::new(&Presentation::Text { text: text.clone() }).unwrap()] }).await.unwrap();
				sender
					.send(&CraftEvent::Completed {
						id,
						native_conversation: "native-42".into(),
					})
					.await
					.unwrap();
			}
			CraftCommand::Shutdown => break,
			CraftCommand::Turn { .. } | CraftCommand::Action { .. } => {
				panic!("unexpected command")
			}
		}
	}
	receiving.abort();
}

#[tokio::test]
async fn one_craft_process_serves_two_active_executions() {
	timeout(Duration::from_secs(20), async {
		let temp = tempfile::tempdir().unwrap();
		let socket = temp.path().join("craft.sock");
		let listener = UnixListener::bind(&socket).unwrap();
		let mut child = Command::new(std::env::current_exe().unwrap()).args(["--ignored", "--exact", "fake_craft_process"]).env("JET_TEST_CRAFT_SOCKET", &socket).env("JET_TEST_CONNECTIONS", "2").stdout(Stdio::null()).stderr(Stdio::inherit()).kill_on_drop(true).spawn().unwrap();
		let mut peers = Vec::new();
		for identity in ["native-first", "native-second"] {
			let (mut stream, _) = listener.accept().await.unwrap();
			stream.write_all(b"jet-craft\n").await.unwrap();
			let (read, write) = stream.into_split();
			let mut reader = FrameReader::new(read);
			let mut writer = FrameWriter::new(write);
			let hello = serde_json::json!({"protocol":{"family":"craft","versions":[{"major":1,"minor":0}],"capabilities":["actions","resume"]},"specification":{"family":"specification","versions":[{"major":1,"minor":0}]},"execution_id":uuid::Uuid::new_v4(),"resume":{"version":{"major":1,"minor":0},"native_conversation":identity}});
			writer.write(&Frame::control(encode_control(&hello).unwrap())).await.unwrap();
			let Frame::Control { payload, .. } = reader.read().await.unwrap() else { panic!("control") };
			let ready: CraftReady = decode_control(&payload).unwrap();
			assert_eq!(ready.specification_protocol.version, jet_protocol::ProtocolVersion { major: 1, minor: 0 });
			reader.enable_multiplexing(); writer.enable_multiplexing();
			reader.read().await.unwrap(); // Initial native action menu.
			peers.push((reader, writer, identity));
		}
		for (mut reader, mut writer, identity) in peers {
			let action = serde_json::json!({"kind":"action","id":identity,"action":{"kind":"invoke","action_id":"inspect","input":{"path":"src"}}});
			writer.write(&Frame::control(encode_control(&action).unwrap())).await.unwrap();
			let Frame::Control { payload, .. } = reader.read().await.unwrap() else { panic!("control") };
			let CraftEvent::Output { presentation, .. } = decode_control(&payload).unwrap() else { panic!("output") };
			assert_eq!(presentation[0].known().unwrap(), Some(Presentation::Text { text: identity.into() }));
			reader.read().await.unwrap(); // Completion before shutting down this execution.
			writer.write(&Frame::control(encode_control(&CraftCommand::Shutdown).unwrap())).await.unwrap();
		}
		assert!(child.wait().await.unwrap().success());
	}).await.unwrap();
}
