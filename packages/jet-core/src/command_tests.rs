use std::time::{Duration, SystemTime};

use jet_store::{EventClass, NewEvent};
use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::test_support::{
	FixedProbe, ManualClock, actor, command_id, equipped, request,
	request_with_id, start_core, start_core_with,
};
use crate::{
	Command, CommandEnvelope, CommandOutcome, Conversation, ConversationId,
	ConversationOrigin, ConversationSnapshot, Core, CoreError, ErrorCategory,
	EventKind, EventPage, EventPayload, EventSequence, Query, QueryResult,
	RetentionPolicy, Revision, Run, RunId, RunLifecycle, WorkingTree,
	WorkingTreeRequest,
};

async fn create_conversation(
	core: &Core,
	retention: RetentionPolicy,
) -> Conversation {
	let outcome = core
		.execute(
			&actor(),
			request(Command::CreateConversation {
				retention,
				working_tree: WorkingTreeRequest::NoProject,
			}),
		)
		.await
		.unwrap();
	let CommandOutcome::ConversationCreated(conversation) = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	conversation
}

async fn create_run(core: &Core, conversation_id: ConversationId) -> Run {
	let outcome = core
		.execute(&actor(), request(Command::CreateRun { conversation_id }))
		.await
		.unwrap();
	let CommandOutcome::RunCreated(run) = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	run
}

async fn transition(core: &Core, run: Run, lifecycle: RunLifecycle) -> Run {
	let outcome = core
		.execute(
			&actor(),
			request(Command::TransitionRun {
				run_id: run.run_id,
				expected_revision: run.revision,
				lifecycle,
			}),
		)
		.await
		.unwrap();
	let CommandOutcome::RunTransitioned(run) = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	run
}

async fn snapshot(
	core: &Core,
	conversation_id: ConversationId,
) -> ConversationSnapshot {
	let result = core
		.query(&actor(), Query::Conversation { conversation_id })
		.await
		.unwrap();
	let QueryResult::Conversation(snapshot) = result else {
		panic!("unexpected result {result:?}");
	};
	*snapshot
}

async fn events_after(core: &Core, after: EventSequence) -> EventPage {
	let result = core.query(&actor(), Query::Events { after }).await.unwrap();
	let QueryResult::Events(page) = result else {
		panic!("unexpected result {result:?}");
	};
	page
}

async fn event_kinds(
	core: &Core,
	after: EventSequence,
) -> Vec<(u64, EventKind)> {
	events_after(core, after)
		.await
		.events
		.into_iter()
		.map(|event| (event.sequence.0, event.kind))
		.collect()
}

fn not_found(code: &str, message: &str) -> CoreError {
	CoreError {
		category: ErrorCategory::NotFound,
		code: code.into(),
		retryable: false,
		message: message.into(),
		detail: None,
		revision_conflict: None,
		recovery_actions: vec![],
	}
}

#[tokio::test]
async fn a_conversation_exists_and_is_queryable_before_any_run() {
	let dir = tempfile::tempdir().unwrap();
	let core = start_core(&dir.path().join("p.sqlite3")).await;

	let conversation =
		create_conversation(&core, RetentionPolicy::Retain).await;

	assert_eq!(
		snapshot(&core, conversation.conversation_id).await,
		ConversationSnapshot {
			cursor: EventSequence(1),
			conversation,
			workspace: None,
			runs: vec![],
		}
	);
	assert_eq!(
		event_kinds(&core, EventSequence(0)).await,
		vec![(
			1,
			EventKind::ConversationCreated {
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTree::NoProject,
				origin: ConversationOrigin::New,
			}
		)]
	);
}

#[tokio::test]
async fn a_conversation_retains_its_terminal_runs_across_core_restarts() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("p.sqlite3");
	let first = start_core(&path).await;
	let conversation =
		create_conversation(&first, RetentionPolicy::Retain).await;
	let conversation_id = conversation.conversation_id;

	let run = create_run(&first, conversation_id).await;
	let run = transition(&first, run, RunLifecycle::Starting).await;
	let run = transition(&first, run, RunLifecycle::Active).await;
	let completed = transition(&first, run, RunLifecycle::Completed).await;
	let second_run = create_run(&first, conversation_id).await;
	let canceled = transition(&first, second_run, RunLifecycle::Canceled).await;
	drop(first);

	let second = start_core(&path).await;
	let restored = snapshot(&second, conversation_id).await;
	let third_run = create_run(&second, conversation_id).await;
	let kinds = event_kinds(&second, EventSequence(6)).await;

	assert_eq!(
		restored,
		ConversationSnapshot {
			cursor: EventSequence(7),
			conversation,
			workspace: None,
			runs: vec![completed, canceled],
		}
	);
	assert!(completed.ended_at.is_some() && canceled.ended_at.is_some());
	assert_eq!(
		kinds,
		vec![
			(
				7,
				EventKind::RunLifecycleChanged {
					from: RunLifecycle::Created,
					to: RunLifecycle::Canceled,
				}
			),
			(8, EventKind::RunCreated {}),
		]
	);
	assert_eq!(
		third_run,
		Run {
			run_id: third_run.run_id,
			conversation_id,
			revision: Revision(1),
			lifecycle: RunLifecycle::Created,
			created_at: third_run.created_at,
			ended_at: None,
		}
	);
}

#[tokio::test]
async fn a_second_run_is_refused_while_one_has_not_ended() {
	let dir = tempfile::tempdir().unwrap();
	let core = start_core(&dir.path().join("p.sqlite3")).await;
	let conversation =
		create_conversation(&core, RetentionPolicy::Retain).await;
	let run = create_run(&core, conversation.conversation_id).await;
	let starting = transition(&core, run, RunLifecycle::Starting).await;

	let error = core
		.execute(
			&actor(),
			request(Command::CreateRun {
				conversation_id: conversation.conversation_id,
			}),
		)
		.await
		.unwrap_err();

	assert_eq!(
		error,
		CoreError {
			category: ErrorCategory::Conflict,
			code: "run.conversation_busy".into(),
			retryable: false,
			message: "the Conversation already has a Run that has not ended"
				.into(),
			detail: None,
			revision_conflict: None,
			recovery_actions: vec![],
		}
	);
	assert_eq!(
		snapshot(&core, conversation.conversation_id).await.runs,
		vec![starting]
	);
}

#[tokio::test]
async fn a_run_lifecycle_only_moves_forward_and_never_leaves_a_terminal_state()
{
	let dir = tempfile::tempdir().unwrap();
	let core = start_core(&dir.path().join("p.sqlite3")).await;
	let conversation =
		create_conversation(&core, RetentionPolicy::Retain).await;
	let run = create_run(&core, conversation.conversation_id).await;
	let refused = async |run: Run, lifecycle: RunLifecycle| {
		core.execute(
			&actor(),
			request(Command::TransitionRun {
				run_id: run.run_id,
				expected_revision: run.revision,
				lifecycle,
			}),
		)
		.await
		.unwrap_err()
	};

	let skipped = refused(run, RunLifecycle::Active).await;
	let never_active = refused(run, RunLifecycle::Completed).await;
	let failed = transition(&core, run, RunLifecycle::Failed).await;
	let revived = refused(failed, RunLifecycle::Active).await;

	let invalid = |message: &str| CoreError {
		category: ErrorCategory::Conflict,
		code: "run.invalid_transition".into(),
		retryable: false,
		message: message.into(),
		detail: None,
		revision_conflict: None,
		recovery_actions: vec![],
	};
	assert_eq!(
		(skipped, never_active, revived),
		(
			invalid("a created Run cannot move to active"),
			invalid("a created Run cannot move to completed"),
			invalid("a failed Run cannot move to active"),
		)
	);
}

#[tokio::test]
async fn an_unknown_conversation_or_run_is_not_found() {
	let dir = tempfile::tempdir().unwrap();
	let core = start_core(&dir.path().join("p.sqlite3")).await;
	let conversation_id = ConversationId(Uuid::now_v7());
	let run_id = RunId(Uuid::now_v7());

	let queried = core
		.query(&actor(), Query::Conversation { conversation_id })
		.await
		.unwrap_err();
	let run_created = core
		.execute(&actor(), request(Command::CreateRun { conversation_id }))
		.await
		.unwrap_err();
	let transitioned = core
		.execute(
			&actor(),
			request(Command::TransitionRun {
				run_id,
				expected_revision: Revision(1),
				lifecycle: RunLifecycle::Starting,
			}),
		)
		.await
		.unwrap_err();

	assert_eq!(
		(queried, run_created, transitioned),
		(
			not_found(
				"conversation.not_found",
				"the Conversation does not exist"
			),
			not_found(
				"conversation.not_found",
				"the Conversation does not exist"
			),
			not_found("run.not_found", "the Run does not exist"),
		)
	);
}

#[tokio::test]
async fn a_command_identity_older_than_thirty_days_cannot_execute_again() {
	let dir = tempfile::tempdir().unwrap();
	let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
	let clock = ManualClock::at(start);
	let core = start_core_with(
		&dir.path().join("p.sqlite3"),
		clock.clone(),
		FixedProbe::new(equipped()),
	)
	.await;
	let command_id = command_id();
	let command = Command::CreateConversation {
		retention: RetentionPolicy::Retain,
		working_tree: WorkingTreeRequest::NoProject,
	};
	let original = core
		.execute(&actor(), request_with_id(command_id, command.clone()))
		.await
		.unwrap();
	clock.advance(Duration::from_hours(30 * 24));
	let within_window = core
		.execute(&actor(), request_with_id(command_id, command.clone()))
		.await
		.unwrap();
	clock.advance(Duration::from_millis(1));

	let error = core
		.execute(&actor(), request_with_id(command_id, command))
		.await
		.unwrap_err();

	assert_eq!(
		error,
		CoreError {
			category: ErrorCategory::InvalidInput,
			code: "command.identity_expired".into(),
			retryable: false,
			message: "the Command identity is older than thirty days".into(),
			detail: None,
			revision_conflict: None,
			recovery_actions: vec![],
		}
	);
	let QueryResult::Conversations(conversations) =
		core.query(&actor(), Query::Conversations).await.unwrap()
	else {
		panic!("expected the Conversation list");
	};
	let CommandOutcome::ConversationCreated(original) = original else {
		panic!("expected the original Conversation");
	};
	assert_eq!(
		(within_window, conversations.conversations),
		(
			CommandOutcome::ConversationCreated(original),
			vec![Conversation {
				conversation_id: original.conversation_id,
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTree::NoProject,
				origin: ConversationOrigin::New,
				created_at: start,
			}]
		)
	);
}

#[tokio::test]
async fn typed_command_content_is_bound_to_the_request_digest() {
	let dir = tempfile::tempdir().unwrap();
	let core = start_core(&dir.path().join("p.sqlite3")).await;
	let command_id = command_id();
	core.execute(
		&actor(),
		CommandEnvelope::new(
			command_id,
			Command::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRequest::NoProject,
			},
			b"same adapter bytes",
		)
		.unwrap(),
	)
	.await
	.unwrap();

	let error = core
		.execute(
			&actor(),
			CommandEnvelope::new(
				command_id,
				Command::CreateConversation {
					retention: RetentionPolicy::ForgetAfterFinalRun,
					working_tree: WorkingTreeRequest::NoProject,
				},
				b"same adapter bytes",
			)
			.unwrap(),
		)
		.await
		.unwrap_err();

	assert_eq!(
		error,
		CoreError {
			category: ErrorCategory::Conflict,
			code: "command.identity_reused".into(),
			retryable: false,
			message:
				"the Command identity was already used for different content"
					.into(),
			detail: None,
			revision_conflict: None,
			recovery_actions: vec![],
		}
	);
}

#[tokio::test]
async fn events_written_by_a_newer_core_are_served_without_interpretation() {
	let dir = tempfile::tempdir().unwrap();
	let core = start_core(&dir.path().join("p.sqlite3")).await;
	let conversation =
		create_conversation(&core, RetentionPolicy::Retain).await;
	let future = EventPayload {
		kind: "run.teleported".into(),
		payload_version: 7,
		payload: serde_json::json!({"to": "another Plane"}),
	};
	core.store
		.write(async |tx| {
			tx.append_event(NewEvent {
				event_id: Uuid::now_v7(),
				actor: actor().record(),
				recorded_at_unix_ms: 0,
				conversation_id: Some(conversation.conversation_id.0),
				run_id: None,
				kind: future.kind.clone(),
				payload_version: future.payload_version,
				payload: future.payload.to_string(),
				class: EventClass::Semantic,
			})
			.await
		})
		.await
		.unwrap();

	let page = events_after(&core, EventSequence(1)).await;

	let kinds: Vec<_> = page.events.iter().map(|event| &event.kind).collect();
	assert_eq!(
		(page.cursor, kinds),
		(
			EventSequence(2),
			vec![&EventKind::Unrecognized(future.clone())]
		)
	);
	assert_eq!(
		EventKind::Unrecognized(future.clone()).encode().unwrap(),
		future
	);
}
