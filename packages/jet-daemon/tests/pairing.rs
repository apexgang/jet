//! Black-box Pairing conformance tests at the public Jet protocol boundary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_protocol::{
	ClientMessage, ErrorCategory, PairingGate, PairingSnapshot, QueryRequest,
	SECURITY_AUDIT_MINOR, ServerHello, ServerMessage,
};
use pretty_assertions::assert_eq;
use support::{connect, handshake_raw, hello, start_jetd};
use uuid::Uuid;

/// The gate is the Plane's own answer to whether it accepts new clients, so
/// it starts closed and survives the daemon that was told to open it
/// (ADR-0017).
#[tokio::test]
async fn the_gate_starts_closed_and_outlives_the_daemon_that_opened_it() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();

	let mut first = start_jetd(&home).await;
	let client = connect(&first, client_id).await;
	let untouched = client.pairing().await.unwrap();
	let opened = client
		.set_pairing_gate(Uuid::now_v7(), PairingGate::Open)
		.await
		.unwrap();
	first.child.kill().await.unwrap();

	let second = start_jetd(&home).await;
	let client = connect(&second, client_id).await;
	let after_restart = client.pairing().await.unwrap();

	assert_eq!(
		(untouched, opened, after_restart),
		(
			PairingSnapshot {
				cursor: 0,
				gate: PairingGate::Closed,
			},
			PairingGate::Open,
			PairingSnapshot {
				cursor: 1,
				gate: PairingGate::Open,
			}
		)
	);
}

/// A client that negotiated a minor without Pairing is answered with a
/// stable refusal rather than a guess (ADR-0019, ADR-0094).
#[tokio::test]
async fn a_client_below_the_pairing_minor_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut older = hello(Uuid::new_v4());
	older.minor = SECURITY_AUDIT_MINOR;

	let (mut connection, welcome) = handshake_raw(&daemon, &older).await;
	assert!(
		matches!(welcome, ServerHello::Welcome { minor, .. }
			if minor == SECURITY_AUDIT_MINOR),
		"expected a welcome at the older minor, got {welcome:?}"
	);
	connection
		.send(&ClientMessage::Query {
			id: 1,
			query: QueryRequest::Pairing,
		})
		.await;

	let ServerMessage::Error { id, error } = connection.receive().await else {
		panic!("expected a refusal");
	};
	assert_eq!(
		(
			id,
			error.category,
			error.code.as_str(),
			error.message.as_str()
		),
		(
			Some(1),
			ErrorCategory::Incompatible,
			"protocol.unsupported_minor",
			"the Pairing Query needs protocol minor 6"
		)
	);
}
