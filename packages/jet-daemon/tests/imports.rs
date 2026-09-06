//! Black-box tests for external Conversations and imports at the public
//! Jet protocol boundary (ADR-0010).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_client::ClientError;
use jet_protocol::{
	ClientMessage, ConversationOrigin, ErrorCategory, ExternalConversationList,
	MANAGED_RUNS_MINOR, QueryRequest, RetentionPolicy, ServerHello,
	ServerMessage, WorkingTreeRequest,
};
use pretty_assertions::assert_eq;
use support::{connect, handshake_raw, hello, init_repository, start_jetd};
use uuid::Uuid;

fn refusal(error: ClientError) -> (ErrorCategory, String) {
	let ClientError::Remote(error) = error else {
		panic!("expected a stable remote error, got {error:?}");
	};
	(error.category, error.code)
}

/// A Plane whose Crafts report no native identities yet sees nothing,
/// registers nothing on a client's say-so, and continues nothing that was
/// never imported; every refusal is a stable code. A Conversation the
/// Plane creates itself says so (ADR-0010, ADR-0068).
#[tokio::test]
async fn a_plane_registers_only_identities_it_can_see() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let repository = init_repository(&dir.path().join("repo"));
	let project = client
		.register_project(Uuid::now_v7(), repository.to_str().unwrap())
		.await
		.unwrap();

	let listed = client.external_conversations().await.unwrap();
	let unseen = client
		.import_conversation(Uuid::now_v7(), "codex", "thread-1")
		.await
		.unwrap_err();
	let unplaced = client
		.resume_imported_conversation(
			Uuid::now_v7(),
			Uuid::nil(),
			RetentionPolicy::Retain,
			WorkingTreeRequest::NoProject,
		)
		.await
		.unwrap_err();
	let unknown = client
		.resume_imported_conversation(
			Uuid::now_v7(),
			Uuid::nil(),
			RetentionPolicy::Retain,
			WorkingTreeRequest::LocalCheckout {
				project_id: project.project_id,
			},
		)
		.await
		.unwrap_err();
	let created = client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();

	assert_eq!(
		(
			listed,
			refusal(unseen),
			refusal(unplaced),
			refusal(unknown),
			created.origin,
		),
		(
			ExternalConversationList {
				cursor: 1,
				discovered: vec![],
				imported: vec![],
			},
			(ErrorCategory::NotFound, "import.not_discovered".into()),
			(
				ErrorCategory::InvalidInput,
				"import.working_tree_required".into()
			),
			(ErrorCategory::NotFound, "import.not_found".into()),
			Some(ConversationOrigin::New),
		)
	);
}

/// A client that negotiated a minor without imports is answered with a
/// stable refusal rather than a guess (ADR-0019, ADR-0094).
#[tokio::test]
async fn a_client_below_the_import_minor_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut older = hello(Uuid::new_v4());
	older.minor = MANAGED_RUNS_MINOR;

	let (mut connection, welcome) = handshake_raw(&daemon, &older).await;
	assert!(
		matches!(welcome, ServerHello::Welcome { minor, .. }
			if minor == MANAGED_RUNS_MINOR),
		"expected a welcome at the older minor, got {welcome:?}"
	);
	connection
		.send(&ClientMessage::Query {
			id: 1,
			query: QueryRequest::ExternalConversations,
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
			"the external Conversation Query needs protocol minor 14"
		)
	);
}
