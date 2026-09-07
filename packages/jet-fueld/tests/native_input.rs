//! Native input reaching a launched Harness through the real helper protocol.
use jet_protocol::*;
use pretty_assertions::assert_eq;
use std::{
	os::unix::fs::{OpenOptionsExt, PermissionsExt},
	path::Path,
	time::Duration,
};
use tokio::net::{
	UnixStream,
	unix::{OwnedReadHalf, OwnedWriteHalf},
};
use uuid::Uuid;

type Reader = FrameReader<OwnedReadHalf>;
type Writer = FrameWriter<OwnedWriteHalf>;

/// Reads its input line by line for as long as that input stays open, and
/// records that it saw the end of it. A sealed launch ends it immediately.
const ECHO_HARNESS: &str = "while IFS= read -r line; do printf 'echo:%s\\n' \"$line\"; done; printf 'ended' > input-ended";

/// Reads one line and then outlives its own input, so nothing else is
/// pending when a refusal is what is being observed.
const LINGERING_HARNESS: &str = "IFS= read -r line; printf 'echo:%s\\n' \"$line\"; while true; do sleep 1; done";

#[tokio::test]
async fn a_streaming_harness_takes_input_after_launch_and_ends_when_it_closes()
{
	tokio::time::timeout(Duration::from_secs(120), async {
		let (root, id, socket, mut helper) = start_helper().await;
		let (mut reader, mut writer, ready) = connect(&socket, id, 3).await;
		assert_eq!(ready.version, ProtocolVersion { major: 1, minor: 3 });
		launch(&mut writer, ECHO_HARNESS, NativeInputMode::Streaming).await;
		assert!(matches!(
			settle(&mut reader, &mut writer).await,
			HelperEvent::Started { .. }
		));
		assert_eq!(line(&mut reader, &mut writer).await, "echo:first");

		// Input written while no source record is pending reaches the
		// Harness, which only an open standard input makes possible.
		send(
			&mut writer,
			&HelperCommand::Input {
				text: "second\n".into(),
			},
		)
		.await;
		assert_eq!(line(&mut reader, &mut writer).await, "echo:second");

		// The same Harness keeps taking input from a replacement connection,
		// because the open pipe belongs to the execution, not the connection.
		drop((reader, writer));
		let (mut reader, mut writer, ready) = connect(&socket, id, 3).await;
		send(
			&mut writer,
			&HelperCommand::Recover {
				source_offset: ready.descriptor.replay.acknowledged,
			},
		)
		.await;
		send(
			&mut writer,
			&HelperCommand::Input {
				text: "third\n".into(),
			},
		)
		.await;
		assert_eq!(line(&mut reader, &mut writer).await, "echo:third");

		// Closing input is the whole stop: no signal, and the Harness exits
		// on end of input having seen every byte that was written to it.
		send(&mut writer, &HelperCommand::CloseInput).await;
		let HelperEvent::Exited { exit_code } =
			settle(&mut reader, &mut writer).await
		else {
			panic!("expected the Harness to end on end of input")
		};
		assert_eq!(exit_code, Some(0));
		assert_eq!(
			std::fs::read_to_string(root.join("input-ended")).unwrap(),
			"ended"
		);
		// A Craft that has nothing left to do closes as soon as it
		// acknowledges the terminal record. The helper still finishes: its
		// own ended source, not the peer leaving, decides that.
		drop((reader, writer));
		assert!(helper.wait().await.unwrap().success());
	})
	.await
	.unwrap();
}

#[tokio::test]
async fn input_is_refused_below_helper_1_3_and_by_a_sealed_launch() {
	tokio::time::timeout(Duration::from_secs(120), async {
		// Each refusal is asked for only once the Harness has gone quiet, so
		// what is tested is the refusal and never a race with a record the
		// helper is right to deliver first.

		// A Helper 1.2 peer cannot write input, whatever the launch declared.
		let (_, id, socket, mut helper) = start_helper().await;
		let (mut reader, mut writer, ready) = connect(&socket, id, 2).await;
		assert_eq!(ready.version, ProtocolVersion { major: 1, minor: 2 });
		launch(&mut writer, ECHO_HARNESS, NativeInputMode::Streaming).await;
		assert!(matches!(
			settle(&mut reader, &mut writer).await,
			HelperEvent::Started { .. }
		));
		assert_eq!(line(&mut reader, &mut writer).await, "echo:first");
		send(
			&mut writer,
			&HelperCommand::Input {
				text: "second\n".into(),
			},
		)
		.await;
		assert!(
			reader.read().await.is_err(),
			"native input needs Helper 1.3"
		);
		helper.start_kill().unwrap();

		// A sealed launch opened no input to write to. This Harness outlives
		// its own input, so the refusal is the only thing left to happen.
		let (_, id, socket, mut helper) = start_helper().await;
		let (mut reader, mut writer, _) = connect(&socket, id, 3).await;
		launch(&mut writer, LINGERING_HARNESS, NativeInputMode::Sealed).await;
		assert!(matches!(
			settle(&mut reader, &mut writer).await,
			HelperEvent::Started { .. }
		));
		assert_eq!(line(&mut reader, &mut writer).await, "echo:first");
		send(
			&mut writer,
			&HelperCommand::Input {
				text: "second\n".into(),
			},
		)
		.await;
		assert!(
			reader.read().await.is_err(),
			"a sealed launch has no open input"
		);
		helper.start_kill().unwrap();

		// The seal itself is what a Harness that ends on end of input sees.
		let (root, id, socket, mut helper) = start_helper().await;
		let (mut reader, mut writer, _) = connect(&socket, id, 3).await;
		launch(&mut writer, ECHO_HARNESS, NativeInputMode::Sealed).await;
		let exit_code = loop {
			match settle(&mut reader, &mut writer).await {
				HelperEvent::Exited { exit_code } => break exit_code,
				HelperEvent::Started { .. } | HelperEvent::Output { .. } => {
					continue;
				}
				event => panic!("unexpected native observation: {event:?}"),
			}
		};
		assert_eq!(exit_code, Some(0));
		drop((reader, writer));
		assert!(helper.wait().await.unwrap().success());
		assert_eq!(
			std::fs::read_to_string(root.join("input-ended")).unwrap(),
			"ended"
		);
	})
	.await
	.unwrap();
}

/// Start a Harness with the declared input mode.
async fn launch(
	writer: &mut Writer,
	script: &str,
	input_mode: NativeInputMode,
) {
	send(
		writer,
		&HelperCommand::Launch {
			program: "/bin/sh".into(),
			arguments: vec!["-c".into(), script.into()],
			input: "first\n".into(),
			input_mode,
		},
	)
	.await;
}

/// Take one record and acknowledge it, keeping the stream at its boundary.
async fn settle(reader: &mut Reader, writer: &mut Writer) -> HelperEvent {
	let record: HelperRecord = receive(reader).await;
	send(
		writer,
		&HelperCommand::Acknowledge {
			source_offset: record.source_offset,
		},
	)
	.await;
	record.event
}

/// Accumulate acknowledged standard output until one complete line exists.
async fn line(reader: &mut Reader, writer: &mut Writer) -> String {
	let mut text = String::new();
	loop {
		match settle(reader, writer).await {
			HelperEvent::Output {
				stream: NativeStream::Stdout,
				bytes,
			} => text.push_str(&String::from_utf8(bytes).unwrap()),
			event => panic!("unexpected native observation: {event:?}"),
		}
		if let Some(end) = text.find('\n') {
			text.truncate(end);
			return text;
		}
	}
}

async fn start_helper() -> (
	std::path::PathBuf,
	Uuid,
	std::path::PathBuf,
	tokio::process::Child,
) {
	let directory = tempfile::tempdir_in("/tmp").unwrap();
	let root = directory.keep().canonicalize().unwrap();
	std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
		.unwrap();
	let id = Uuid::new_v4();
	let config = HelperConfig {
		execution_id: id,
		working_directory: root.to_str().unwrap().into(),
		project_directory: root.to_str().unwrap().into(),
		craft_digest: "test-craft".into(),
		executables: vec!["/bin/sh".into()],
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
	let helper = tokio::process::Command::new(env!("CARGO_BIN_EXE_jetfueld"))
		.args(["run", "--config"])
		.arg(path)
		.kill_on_drop(true)
		.spawn()
		.unwrap();
	let socket = root.join("h.sock");
	(root, id, socket, helper)
}

async fn connect(
	socket: &Path,
	id: Uuid,
	minor: u32,
) -> (Reader, Writer, HelperReady) {
	let stream = loop {
		match UnixStream::connect(socket).await {
			Ok(stream) => break stream,
			Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
		}
	};
	let (read, write) = stream.into_split();
	let mut reader = FrameReader::new(read);
	let mut writer = FrameWriter::new(write);
	send(
		&mut writer,
		&HelperHello {
			execution_id: id,
			protocol: ProtocolOffer {
				family: ProtocolFamily::Helper,
				versions: vec![ProtocolVersion { major: 1, minor }],
				capabilities: vec![],
			},
		},
	)
	.await;
	let ready = receive(&mut reader).await;
	(reader, writer, ready)
}

async fn send(writer: &mut Writer, value: &impl serde::Serialize) {
	writer
		.write(&Frame::control(encode_control(value).unwrap()))
		.await
		.unwrap();
}

async fn receive<T: serde::de::DeserializeOwned>(reader: &mut Reader) -> T {
	let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
		panic!("control")
	};
	decode_control(&payload).unwrap()
}
