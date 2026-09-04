//! Black-box tests for Conversation retention at the public Jet protocol
//! boundary: a real `jetd`, a real temporary SQLite store, and only the
//! wire vocabulary.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use jet_client::ClientError;
use jet_protocol::{
	CommandResponse, Conversation, ConversationList, ConversationSnapshot,
	ErrorCategory, EventPage, RetentionPolicy, RunLifecycle, ServerMessage,
	WireError,
};
use pretty_assertions::assert_eq;
use support::{Daemon, connect, connect_raw, start_jetd};
use uuid::Uuid;

/// Creates a Conversation with a raw command frame that says nothing about
/// retention, so the daemon's default decides.
async fn create_conversation_without_a_retention_choice(
	daemon: &Daemon,
	client_id: Uuid,
) -> Conversation {
	let mut connection = connect_raw(daemon, client_id).await;
	connection
		.send_bytes(
			format!(
				"{{\"kind\":\"command\",\"id\":1,\"command_id\":\"{}\",\"command\":{{\"type\":\"create_conversation\"}}}}",
				Uuid::now_v7()
			)
			.into_bytes(),
		)
		.await;
	match connection.receive::<ServerMessage>().await {
		ServerMessage::CommandResult {
			id: 1,
			result: CommandResponse::ConversationCreated(conversation),
		} => conversation,
		other => panic!("unexpected reply {other:?}"),
	}
}

#[tokio::test]
async fn a_conversation_is_queryable_before_any_run_and_retained_by_default() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client_id = Uuid::new_v4();

	let conversation =
		create_conversation_without_a_retention_choice(&daemon, client_id)
			.await;
	let snapshot = connect(&daemon, client_id)
		.await
		.conversation(conversation.conversation_id)
		.await
		.unwrap();

	assert_eq!(
		snapshot,
		ConversationSnapshot {
			cursor: 1,
			conversation: Conversation {
				conversation_id: conversation.conversation_id,
				retention: RetentionPolicy::Retain,
				created_at_unix_ms: conversation.created_at_unix_ms,
			},
			runs: vec![],
		}
	);
}

#[tokio::test]
async fn a_conversation_is_retained_with_its_terminal_runs_across_a_jetd_restart()
 {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();

	let mut first = start_jetd(&home).await;
	let conversation =
		create_conversation_without_a_retention_choice(&first, client_id).await;
	let mut client = connect(&first, client_id).await;
	let mut run = client
		.create_run(Uuid::now_v7(), conversation.conversation_id)
		.await
		.unwrap();
	for lifecycle in [RunLifecycle::Starting, RunLifecycle::Active] {
		run = client
			.transition_run(Uuid::now_v7(), run.run_id, run.revision, lifecycle)
			.await
			.unwrap();
	}
	let completed = client
		.transition_run(
			Uuid::now_v7(),
			run.run_id,
			run.revision,
			RunLifecycle::Completed,
		)
		.await
		.unwrap();
	let second_run = client
		.create_run(Uuid::now_v7(), conversation.conversation_id)
		.await
		.unwrap();
	let canceled = client
		.transition_run(
			Uuid::now_v7(),
			second_run.run_id,
			second_run.revision,
			RunLifecycle::Canceled,
		)
		.await
		.unwrap();
	let before_restart = client
		.conversation(conversation.conversation_id)
		.await
		.unwrap();
	let journal_before_restart = client.events_after(0).await.unwrap();
	first.child.kill().await.unwrap();

	let second = start_jetd(&home).await;
	let mut client = connect(&second, client_id).await;
	let after_restart = client
		.conversation(conversation.conversation_id)
		.await
		.unwrap();
	let journal_after_restart = client.events_after(0).await.unwrap();
	let listed = client.conversations().await.unwrap();
	let third_run = client
		.create_run(Uuid::now_v7(), conversation.conversation_id)
		.await
		.unwrap();
	let newest = client.events_after(after_restart.cursor).await.unwrap();

	assert_eq!(after_restart, before_restart);
	assert_eq!(
		after_restart,
		ConversationSnapshot {
			cursor: 7,
			conversation: conversation.clone(),
			runs: vec![completed.clone(), canceled],
		}
	);
	assert!(completed.ended_at_unix_ms.is_some());
	assert_eq!(
		listed,
		ConversationList {
			cursor: 7,
			conversations: vec![conversation],
		}
	);
	assert_eq!(journal_after_restart, journal_before_restart);
	assert_eq!(
		(
			journal_after_restart.cursor,
			journal_after_restart
				.events
				.iter()
				.map(|event| (event.sequence, event.kind.as_str()))
				.collect::<Vec<_>>()
		),
		(
			7,
			vec![
				(1, "conversation.created"),
				(2, "run.created"),
				(3, "run.lifecycle_changed"),
				(4, "run.lifecycle_changed"),
				(5, "run.lifecycle_changed"),
				(6, "run.created"),
				(7, "run.lifecycle_changed"),
			]
		)
	);
	assert_eq!(
		(
			newest.cursor,
			newest
				.events
				.iter()
				.map(|event| (
					event.sequence,
					event.kind.as_str(),
					event.run_id
				))
				.collect::<Vec<_>>()
		),
		(8, vec![(8, "run.created", Some(third_run.run_id))])
	);
}

#[tokio::test]
async fn a_live_run_blocks_a_second_one_with_a_stable_conflict() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut client = connect(&daemon, Uuid::new_v4()).await;
	let conversation = client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();
	client
		.create_run(Uuid::now_v7(), conversation.conversation_id)
		.await
		.unwrap();

	let error = client
		.create_run(Uuid::now_v7(), conversation.conversation_id)
		.await
		.unwrap_err();

	let ClientError::Remote(error) = error else {
		panic!("expected a remote error, got {error:?}");
	};
	assert_eq!(
		error,
		WireError {
			category: ErrorCategory::Conflict,
			code: "run.conversation_busy".into(),
			retryable: false,
			message: "the Conversation already has a Run that has not ended"
				.into(),
			revision_conflict: None,
			recovery_actions: vec![],
		}
	);
}

#[tokio::test]
async fn an_unknown_conversation_is_reported_as_not_found() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut client = connect(&daemon, Uuid::new_v4()).await;

	let error = client.conversation(Uuid::now_v7()).await.unwrap_err();

	let ClientError::Remote(error) = error else {
		panic!("expected a remote error, got {error:?}");
	};
	assert_eq!(
		error,
		WireError {
			category: ErrorCategory::NotFound,
			code: "conversation.not_found".into(),
			retryable: false,
			message: "the Conversation does not exist".into(),
			revision_conflict: None,
			recovery_actions: vec![],
		}
	);
}

#[tokio::test]
async fn an_empty_journal_page_still_carries_the_cursor() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut client = connect(&daemon, Uuid::new_v4()).await;
	client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();

	let page = client.events_after(1).await.unwrap();

	assert_eq!(
		page,
		EventPage {
			cursor: 1,
			events: vec![],
		}
	);
}
