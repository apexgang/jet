//! Black-box conformance tests at the public Jet protocol boundary: a real
//! `jetd` subprocess, a real temporary SQLite store, and the Rust client.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Stdio;

use jet_client::Client;
use jet_protocol::{
	CODEC_JSON_V1, ClientHello, ErrorCategory, Frame, FrameReader, FrameWriter,
	PREFACE, PlaneStatus, ServerHello, VersionRange, WireError, decode_control,
	encode_control,
};
use pretty_assertions::assert_eq;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use uuid::Uuid;

struct Daemon {
	child: Child,
	socket: PathBuf,
}

fn jetd(home: &Path) -> Command {
	let mut command = Command::new(env!("CARGO_BIN_EXE_jetd"));
	command
		.arg("run")
		.arg("--home")
		.arg(home)
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.kill_on_drop(true);
	command
}

async fn start_jetd(home: &Path) -> Daemon {
	let mut child = jetd(home).spawn().unwrap();
	let stdout = child.stdout.take().unwrap();
	let mut lines = BufReader::new(stdout).lines();
	let ready = lines.next_line().await.unwrap().expect("jetd exited early");
	let ready: serde_json::Value = serde_json::from_str(&ready).unwrap();
	assert_eq!(ready["event"], "ready");
	Daemon {
		child,
		socket: PathBuf::from(ready["socket"].as_str().unwrap()),
	}
}

async fn status(daemon: &Daemon, client_id: Uuid) -> PlaneStatus {
	let mut client = Client::connect_local(&daemon.socket, client_id)
		.await
		.unwrap();
	client.status().await.unwrap()
}

#[tokio::test]
async fn status_is_answered_before_and_after_a_daemon_crash_and_restart() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();

	let mut first = start_jetd(&home).await;
	let before = status(&first, client_id).await;
	first.child.kill().await.unwrap();

	let second = start_jetd(&home).await;
	let after = status(&second, client_id).await;

	assert_eq!(before.daemon_starts, 1);
	assert_eq!(
		after,
		PlaneStatus {
			plane_id: before.plane_id,
			daemon_starts: 2,
			started_at_unix_ms: after.started_at_unix_ms,
			core_version: env!("CARGO_PKG_VERSION").into(),
		}
	);
	assert!(after.started_at_unix_ms >= before.started_at_unix_ms);
}

#[tokio::test]
async fn a_second_jetd_is_refused_by_the_live_lock_not_by_stale_metadata() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");

	let mut first = start_jetd(&home).await;
	let refused = jetd(&home).output().await.unwrap();
	let stderr = String::from_utf8(refused.stderr).unwrap();
	let first_pid = first.child.id().unwrap();
	assert_eq!(refused.status.code(), Some(2), "{stderr}");
	assert!(stderr.contains(&format!("pid {first_pid}")), "{stderr}");

	// Killing the owner leaves its metadata behind; only the released OS
	// lock decides that a new daemon may start.
	first.child.kill().await.unwrap();
	let third = start_jetd(&home).await;
	assert!(third.socket.exists());
}

#[tokio::test]
async fn sigterm_shuts_jetd_down_and_removes_its_socket() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");

	let mut daemon = start_jetd(&home).await;
	let pid = rustix::process::Pid::from_raw(
		i32::try_from(daemon.child.id().unwrap()).unwrap(),
	)
	.unwrap();
	rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
	let exit = daemon.child.wait().await.unwrap();

	assert_eq!(
		(exit.success(), daemon.socket.exists()),
		(true, false),
		"jetd should exit cleanly and remove its socket"
	);
}

#[tokio::test]
async fn an_unsupported_codec_is_rejected_during_the_handshake() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let daemon = start_jetd(&home).await;

	let mut stream = UnixStream::connect(&daemon.socket).await.unwrap();
	stream.write_all(PREFACE).await.unwrap();
	let (read, write) = stream.into_split();
	let mut writer = FrameWriter::new(write);
	let mut reader = FrameReader::new(read);
	let hello = ClientHello {
		protocol: VersionRange { min: 1, max: 1 },
		codec: "cbor-v1".into(),
		client_id: Uuid::new_v4(),
	};
	writer
		.write(&Frame::Control(encode_control(&hello).unwrap()))
		.await
		.unwrap();

	let Frame::Control(reply) = reader.read().await.unwrap() else {
		panic!("expected a control frame");
	};
	let reply: ServerHello = decode_control(&reply).unwrap();
	assert_eq!(
		reply,
		ServerHello::Rejected {
			error: WireError {
				category: ErrorCategory::Incompatible,
				code: "protocol.unsupported_codec".into(),
				retryable: false,
				message: format!("only the {CODEC_JSON_V1} codec is supported"),
			}
		}
	);
}
