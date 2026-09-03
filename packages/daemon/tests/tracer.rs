//! Black-box conformance tests at the public Jet protocol boundary: a real
//! `jetd` subprocess, a real temporary SQLite store, and the Rust client.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_protocol::{
	CODEC_JSON_V1, ClientHello, ErrorCategory, Frame, FrameReader, FrameWriter,
	MAX_CONTROL_FRAME, MAX_DATA_FRAME, PREFACE, PROTOCOL_VERSION, PlaneStatus,
	ServerHello, VersionRange, WireError, decode_control, encode_control,
};
use pretty_assertions::assert_eq;
use support::{Daemon, jetd, start_jetd};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use uuid::Uuid;

async fn status(daemon: &Daemon, client_id: Uuid) -> PlaneStatus {
	support::connect(daemon, client_id)
		.await
		.status()
		.await
		.unwrap()
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

fn hello(codec: &str, max_control_frame: u32) -> ClientHello {
	ClientHello {
		protocol: VersionRange { min: 1, max: 1 },
		codec: codec.into(),
		client_id: Uuid::new_v4(),
		max_control_frame,
		max_data_frame: u32::try_from(MAX_DATA_FRAME).unwrap(),
		capabilities: vec![],
	}
}

async fn raw_handshake(daemon: &Daemon, hello: &ClientHello) -> ServerHello {
	let mut stream = UnixStream::connect(&daemon.socket).await.unwrap();
	stream.write_all(PREFACE).await.unwrap();
	let (read, write) = stream.into_split();
	let mut writer = FrameWriter::new(write);
	let mut reader = FrameReader::new(read);
	writer
		.write(&Frame::Control(encode_control(hello).unwrap()))
		.await
		.unwrap();
	let Frame::Control(reply) = reader.read().await.unwrap() else {
		panic!("expected a control frame");
	};
	decode_control(&reply).unwrap()
}

#[tokio::test]
async fn the_handshake_negotiates_the_smaller_frame_limits() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let daemon = start_jetd(&home).await;

	let reply = raw_handshake(&daemon, &hello(CODEC_JSON_V1, 4096)).await;

	assert_eq!(
		reply,
		ServerHello::Welcome {
			protocol: PROTOCOL_VERSION,
			codec: CODEC_JSON_V1.into(),
			max_control_frame: 4096,
			max_data_frame: u32::try_from(MAX_DATA_FRAME).unwrap(),
			capabilities: vec![],
		}
	);
}

#[tokio::test]
async fn an_unsupported_codec_is_rejected_during_the_handshake() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let daemon = start_jetd(&home).await;

	let reply = raw_handshake(
		&daemon,
		&hello("cbor-v1", u32::try_from(MAX_CONTROL_FRAME).unwrap()),
	)
	.await;
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
