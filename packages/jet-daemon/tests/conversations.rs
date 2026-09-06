//! Black-box tests for Conversation retention at the public Jet protocol
//! boundary: a real `jetd`, a real temporary SQLite store, and only the
//! wire vocabulary.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use jet_client::ClientError;
use jet_protocol::{
	ClientMessage, CommandResponse, Conversation, ConversationList,
	ConversationOrigin, ConversationSnapshot, ErrorCategory, EventPage,
	QueryRequest, QueryResponse, RestartMetadata, RetentionPolicy,
	RunLifecycle, ServerHello, ServerMessage, WireError, WorkingTree,
};
use jet_store::{
	ActorRecord, CONVERSATION_PAGE_LIMIT, EventClass, NewEvent, Store,
};
use pretty_assertions::assert_eq;
use support::{Daemon, connect, connect_raw, handshake_raw, hello, start_jetd};
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
				working_tree: Some(WorkingTree::NoProject),
				origin: Some(ConversationOrigin::New),
				created_at_unix_ms: conversation.created_at_unix_ms,
			},
			workspace: None,
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
	let client = connect(&first, client_id).await;
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
	let client = connect(&second, client_id).await;
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
			workspace: None,
			runs: vec![completed.clone(), canceled],
		}
	);
	assert!(completed.ended_at_unix_ms.is_some());
	assert_eq!(
		listed,
		ConversationList {
			cursor: 7,
			conversations: vec![conversation],
			next_page: None,
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
	let client = connect(&daemon, Uuid::new_v4()).await;
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
			restart: None,
			recovery_actions: vec![],
		}
	);
}

#[tokio::test]
async fn an_unknown_conversation_is_reported_as_not_found() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;

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
			restart: None,
			recovery_actions: vec![],
		}
	);
}

#[tokio::test]
async fn an_empty_journal_page_still_carries_the_cursor() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
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

#[tokio::test]
async fn an_event_cursor_ahead_of_the_plane_requires_a_fresh_snapshot() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;

	let error = client.events_after(1).await.unwrap_err();
	let ClientError::Remote(error) = error else {
		panic!("expected a remote error, got {error:?}");
	};

	assert_eq!(
		(error.code.as_str(), error.restart),
		(
			"event.cursor_ahead",
			Some(RestartMetadata::CursorAhead {
				current_snapshot_revision: 0,
			}),
		)
	);
}

#[tokio::test]
async fn an_expired_event_cursor_requires_a_fresh_fenced_snapshot() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();
	let mut first = start_jetd(&home).await;
	let client = connect(&first, client_id).await;
	let conversation = client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();
	drop(client);
	first.child.kill().await.unwrap();

	let store = Store::open(&home.join("plane.sqlite3")).await.unwrap();
	store
		.write(async |tx| {
			tx.append_event(NewEvent {
				event_id: Uuid::now_v7(),
				actor: ActorRecord::InteractiveClient { client_id },
				recorded_at_unix_ms: 0,
				conversation_id: Some(conversation.conversation_id),
				run_id: None,
				kind: "run.output_progressed".into(),
				payload_version: 1,
				payload: "{}".into(),
				class: EventClass::Operational,
			})
			.await?;
			let coverage = tx.verified_projection_coverage().await?;
			tx.compact_operational_events(coverage, 1).await
		})
		.await
		.unwrap();
	store.close().await;
	drop(store);

	let second = start_jetd(&home).await;
	let client = connect(&second, client_id).await;
	let expired = client.events_after(0).await.unwrap_err();
	let snapshot = client.conversations().await.unwrap();
	let resumed = client.events_after(snapshot.cursor).await.unwrap();
	let ClientError::Remote(expired) = expired else {
		panic!("expected a remote error, got {expired:?}");
	};

	assert_eq!(
		(expired.code.as_str(), expired.restart),
		(
			"event.cursor_expired",
			Some(RestartMetadata::CursorExpired {
				minimum_available_cursor: 2,
				current_snapshot_revision: 2,
			}),
		)
	);
	assert_eq!(
		(snapshot.cursor, resumed),
		(
			2,
			EventPage {
				cursor: 2,
				events: vec![]
			}
		)
	);
}

#[tokio::test]
async fn a_concurrent_write_stales_pagination_and_replays_after_the_fence() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();
	let daemon = start_jetd(&home).await;
	let client = connect(&daemon, client_id).await;
	for _ in 0..=CONVERSATION_PAGE_LIMIT {
		client
			.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
			.await
			.unwrap();
	}
	let mut legacy_hello = hello(client_id);
	legacy_hello.minor = 0;
	let (mut legacy, welcome) = handshake_raw(&daemon, &legacy_hello).await;
	assert!(matches!(welcome, ServerHello::Welcome { minor: 0, .. }));
	legacy
		.send(&ClientMessage::Query {
			id: 1,
			query: QueryRequest::Conversations,
		})
		.await;
	let ServerMessage::QueryResult {
		result: QueryResponse::Conversations(legacy_list),
		..
	} = legacy.receive().await
	else {
		panic!("expected the legacy Conversation list");
	};
	legacy
		.send(&ClientMessage::Query {
			id: 2,
			query: QueryRequest::Status,
		})
		.await;
	let ServerMessage::QueryResult {
		result: QueryResponse::Status(legacy_status),
		..
	} = legacy.receive().await
	else {
		panic!("expected the legacy status");
	};
	assert_eq!(
		(
			legacy_list.conversations.len(),
			legacy_list.next_page,
			legacy_status.cursor,
		),
		(CONVERSATION_PAGE_LIMIT + 1, None, None)
	);
	let first = client.conversations().await.unwrap();
	let page_cursor = first.next_page.expect("a second page");
	let second = client.next_conversations(page_cursor).await.unwrap();
	let snapshot_revision = u64::try_from(CONVERSATION_PAGE_LIMIT + 1).unwrap();
	assert_eq!(
		(
			first.cursor,
			first.conversations.len(),
			second.cursor,
			second.conversations.len(),
			second.next_page,
		),
		(
			snapshot_revision,
			CONVERSATION_PAGE_LIMIT,
			snapshot_revision,
			1,
			None,
		)
	);

	client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();
	let stale = client.next_conversations(page_cursor).await.unwrap_err();
	let ClientError::Remote(stale) = stale else {
		panic!("expected a remote error, got {stale:?}");
	};
	assert_eq!(
		(stale.code.as_str(), stale.restart),
		(
			"pagination.stale",
			Some(RestartMetadata::PaginationStale {
				current_snapshot_revision: snapshot_revision + 1,
			}),
		)
	);

	let fresh = client.conversations().await.unwrap();
	client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();
	let replay = client.events_after(fresh.cursor).await.unwrap();
	assert_eq!(
		(
			fresh.cursor,
			replay.cursor,
			replay.events.len(),
			replay.events[0].sequence,
			replay.events[0].kind.as_str(),
		),
		(
			snapshot_revision + 1,
			snapshot_revision + 2,
			1,
			snapshot_revision + 2,
			"conversation.created",
		)
	);
}
