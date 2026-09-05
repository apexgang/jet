//! A real `jetd` subprocess over a temporary Jet home, plus raw Jet
//! protocol access beside the Rust client.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Stdio;

use jet_client::Client;
use jet_protocol::{
	CODEC_JSON_V1, ClientHello, Frame, FrameReader, FrameWriter,
	MAX_CONTROL_FRAME, MAX_DATA_FRAME, PREFACE, PROTOCOL_MINOR, ServerHello,
	StreamId, VersionRange, decode_control, encode_control,
};
use pretty_assertions::assert_eq;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::process::{Child, Command};
use uuid::Uuid;

pub struct Daemon {
	pub child: Child,
	pub socket: PathBuf,
}

pub fn jetd(home: &Path) -> Command {
	let mut command = Command::new(env!("CARGO_BIN_EXE_jetd"));
	command
		.arg("serve")
		.arg("--home")
		.arg(home)
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.kill_on_drop(true);
	command
}

pub async fn start_jetd(home: &Path) -> Daemon {
	start(&mut jetd(home)).await
}

/// Starts `jetd` where none of the external tools it invokes can be found,
/// so its Capability snapshot reports them missing (ADR-0056). The search
/// path is given to the child alone; this process keeps its own.
pub async fn start_jetd_without_external_tools(home: &Path) -> Daemon {
	start(jetd(home).env("PATH", "/jet-has-no-tools-here")).await
}

pub async fn start(command: &mut Command) -> Daemon {
	let mut child = command.spawn().unwrap();
	let stdout = child.stdout.take().unwrap();
	let mut lines = BufReader::new(stdout).lines();
	let ready = lines.next_line().await.unwrap().expect("jetd exited early");
	let ready: serde_json::Value = serde_json::from_str(&ready).unwrap();
	assert_eq!(ready["status"], "ready");
	Daemon {
		child,
		socket: PathBuf::from(ready["socket"].as_str().unwrap()),
	}
}

pub async fn connect(daemon: &Daemon, client_id: Uuid) -> Client {
	Client::connect_local(&daemon.socket, client_id)
		.await
		.unwrap()
}

/// A hello that speaks exactly what this build of `jetd` speaks.
pub fn hello(client_id: Uuid) -> ClientHello {
	ClientHello {
		protocol: VersionRange { min: 1, max: 1 },
		minor: PROTOCOL_MINOR,
		codec: CODEC_JSON_V1.into(),
		client_id,
		max_control_frame: u32::try_from(MAX_CONTROL_FRAME).unwrap(),
		max_data_frame: u32::try_from(MAX_DATA_FRAME).unwrap(),
		capabilities: vec![],
	}
}

/// One framed connection driven by hand, below the Rust client.
pub struct RawConnection {
	reader: FrameReader<OwnedReadHalf>,
	writer: FrameWriter<OwnedWriteHalf>,
	multiplexed: bool,
}

impl RawConnection {
	pub async fn send<T: serde::Serialize>(&mut self, message: &T) {
		self.send_bytes(encode_control(message).unwrap()).await;
	}

	pub async fn send_bytes(&mut self, payload: Vec<u8>) {
		let frame = if self.multiplexed {
			Frame::stream_control(StreamId::new(1).unwrap(), payload)
		} else {
			Frame::control(payload)
		};
		self.writer.write(&frame).await.unwrap();
	}

	pub async fn receive<T: serde::de::DeserializeOwned>(&mut self) -> T {
		let Frame::Control { payload: reply, .. } =
			self.reader.read().await.unwrap()
		else {
			panic!("expected a control frame");
		};
		decode_control(&reply).unwrap()
	}

	fn enable_multiplexing(&mut self) {
		self.reader.enable_multiplexing();
		self.writer.enable_multiplexing();
		self.multiplexed = true;
	}
}

/// Connects and sends the preface, leaving the handshake to the caller.
pub async fn open_raw(daemon: &Daemon) -> RawConnection {
	let mut stream = UnixStream::connect(&daemon.socket).await.unwrap();
	stream.write_all(PREFACE).await.unwrap();
	let (read, write) = stream.into_split();
	RawConnection {
		reader: FrameReader::new(read),
		writer: FrameWriter::new(write),
		multiplexed: false,
	}
}

/// Connects, sends `hello`, and returns the daemon's answer to it.
pub async fn handshake_raw(
	daemon: &Daemon,
	hello: &ClientHello,
) -> (RawConnection, ServerHello) {
	let mut connection = open_raw(daemon).await;
	connection.send(hello).await;
	let reply = connection.receive().await;
	if matches!(
		&reply,
		ServerHello::Welcome {
			minor,
			..
		} if *minor >= jet_protocol::MULTIPLEXED_STREAMS_MINOR
	) {
		connection.enable_multiplexing();
	}
	(connection, reply)
}

/// Connects as `client_id` and asserts the daemon welcomed it.
pub async fn connect_raw(daemon: &Daemon, client_id: Uuid) -> RawConnection {
	let (connection, reply) = handshake_raw(daemon, &hello(client_id)).await;
	assert!(
		matches!(reply, ServerHello::Welcome { .. }),
		"expected a welcome, got {reply:?}"
	);
	connection
}
