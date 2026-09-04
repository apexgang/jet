//! Black-box conformance tests at the public Jet protocol boundary: a real
//! `jetd` subprocess, a real temporary SQLite store, and the Rust client.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_protocol::{
	CODEC_JSON_V1, ClientHello, ErrorCategory, MAX_DATA_FRAME, PROTOCOL_MINOR,
	PROTOCOL_VERSION, PlaneStatus, ServerHello, ServerMessage, WireError,
};
use pretty_assertions::assert_eq;
use support::{Daemon, connect_raw, handshake_raw, hello, jetd, start_jetd};
use uuid::Uuid;

async fn status(daemon: &Daemon, client_id: Uuid) -> PlaneStatus {
	support::connect(daemon, client_id)
		.await
		.status()
		.await
		.unwrap()
}

fn send_sigterm(daemon: &Daemon) {
	let pid = rustix::process::Pid::from_raw(
		i32::try_from(daemon.child.id().unwrap()).unwrap(),
	)
	.unwrap();
	rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
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

	assert_eq!(
		(&before, &after),
		(
			&PlaneStatus {
				plane_id: after.plane_id,
				daemon_starts: 1,
				started_at_unix_ms: before.started_at_unix_ms,
				core_version: env!("CARGO_PKG_VERSION").into(),
			},
			&PlaneStatus {
				plane_id: after.plane_id,
				daemon_starts: 2,
				started_at_unix_ms: after.started_at_unix_ms,
				core_version: env!("CARGO_PKG_VERSION").into(),
			}
		)
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
async fn sigterm_drains_connected_clients_then_exits_and_removes_the_socket() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let mut daemon = start_jetd(&home).await;
	let mut connected = connect_raw(&daemon, Uuid::new_v4()).await;

	send_sigterm(&daemon);
	let farewell: ServerMessage = connected.receive().await;
	let exit = daemon.child.wait().await.unwrap();

	assert_eq!(
		(farewell, exit.success(), daemon.socket.exists()),
		(
			ServerMessage::Error {
				id: None,
				error: WireError {
					category: ErrorCategory::Unavailable,
					code: "daemon.draining".into(),
					retryable: true,
					message: "jetd is shutting down; reconnect later and retry with the same Command identity".into(),
					revision_conflict: None,
					recovery_actions: vec![],
				},
			},
			true,
			false
		),
		"jetd should tell clients it is draining, exit cleanly, and remove its socket"
	);
}

#[tokio::test]
async fn the_handshake_negotiates_the_smaller_frame_limits_and_minor() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let daemon = start_jetd(&home).await;
	let newer_client = ClientHello {
		minor: PROTOCOL_MINOR + 3,
		max_control_frame: 4096,
		..hello(Uuid::new_v4())
	};

	let (_, reply) = handshake_raw(&daemon, &newer_client).await;

	assert_eq!(
		reply,
		ServerHello::Welcome {
			protocol: PROTOCOL_VERSION,
			minor: PROTOCOL_MINOR,
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
	let cbor_client = ClientHello {
		codec: "cbor-v1".into(),
		..hello(Uuid::new_v4())
	};

	let (_, reply) = handshake_raw(&daemon, &cbor_client).await;

	assert_eq!(
		reply,
		ServerHello::Rejected {
			error: WireError {
				category: ErrorCategory::Incompatible,
				code: "protocol.unsupported_codec".into(),
				retryable: false,
				message: format!("only the {CODEC_JSON_V1} codec is supported"),
				revision_conflict: None,
				recovery_actions: vec![],
			}
		}
	);
}
