//! Black-box multiplexing contracts against a real `jetd` and Plane store.

mod support;

use jet_protocol::{ErrorCategory, ServerMessage, StreamControl, WireError};
use pretty_assertions::assert_eq;
use uuid::Uuid;

use support::{connect, connect_raw, start_jetd};

#[tokio::test]
async fn one_connection_serves_concurrent_numbered_queries() {
	let home = tempfile::tempdir().unwrap();
	let daemon = start_jetd(home.path()).await;
	let client = connect(&daemon, Uuid::new_v4()).await;

	let (first, second) = tokio::join!(client.status(), client.status());

	assert_eq!(first.unwrap(), second.unwrap());
}

#[tokio::test]
async fn credit_for_an_unopened_binary_stream_is_rejected_explicitly() {
	let home = tempfile::tempdir().unwrap();
	let daemon = start_jetd(home.path()).await;
	let mut connection = connect_raw(&daemon, Uuid::new_v4()).await;

	connection
		.send(&StreamControl::Credit { bytes: 1024 })
		.await;
	let reply: ServerMessage = connection.receive().await;

	assert_eq!(
		reply,
		ServerMessage::Error {
			id: None,
			error: WireError {
				category: ErrorCategory::InvalidInput,
				code: "protocol.unknown_stream".into(),
				retryable: false,
				message: "credit addressed a binary stream that is not open"
					.into(),
				revision_conflict: None,
				restart: None,
				recovery_actions: vec![],
			},
		}
	);
}

#[tokio::test]
async fn merged_features_keep_their_negotiated_protocol_boundaries() {
	use serde_json::{Value, json};
	let home = tempfile::tempdir().unwrap();
	let daemon = start_jetd(home.path()).await;
	let missing = Uuid::new_v4();
	for minor in [16, 17, 18] {
		let mut hello = support::hello(Uuid::new_v4());
		hello.minor = minor;
		let (mut connection, _) = support::handshake_raw(&daemon, &hello).await;
		let mut errors = Vec::new();
		for query in [
			json!({"type":"turn_queue", "conversation_id":missing}),
			json!({"type":"change_artifact", "sha256":"0".repeat(64), "offset":"0"}),
			json!({"type":"workspace_terminals", "workspace_id":missing}),
		] {
			connection
				.send(&json!({"kind":"query", "id":1, "query":query}))
				.await;
			let reply: Value = connection.receive().await;
			errors.push(reply["error"]["code"].as_str().unwrap().to_owned());
		}
		// A supported request reaches domain validation; newer features are
		// refused before execution rather than borrowing another feature's minor.
		assert_eq!(
			errors,
			vec![
				"conversation.not_found",
				if minor < 17 {
					"protocol.unsupported_minor"
				} else {
					"checkpoint.capture_failed"
				},
				if minor < 18 {
					"protocol.unsupported_minor"
				} else {
					"terminal.not_found"
				},
			],
			"negotiated minor {minor}"
		);
	}
}
