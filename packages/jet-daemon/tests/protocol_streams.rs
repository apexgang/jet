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
