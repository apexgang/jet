use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::{
	Actor, ClientId, Clock, Command, CommandEnvelope, CommandId,
	CommandOutcome, Conversation, ConversationId, ConversationSnapshot, Core,
	CoreError, ErrorCategory, EventKind, EventSequence, Query, QueryResult,
	Retention, Revision, Run, RunId, RunLifecycle,
};
use jet_store::Store;

fn actor() -> Actor {
	Actor::InteractiveClient {
		client_id: ClientId(Uuid::nil()),
	}
}

fn command_id() -> CommandId {
	CommandId(Uuid::now_v7())
}

fn request(command: Command) -> CommandEnvelope {
	request_with_id(command_id(), command)
}

fn request_with_id(command_id: CommandId, command: Command) -> CommandEnvelope {
	let bytes = serde_json::to_vec(&command).unwrap();
	CommandEnvelope::new(command_id, command, &bytes)
}

#[derive(Debug)]
struct ManualClock(Mutex<SystemTime>);

impl ManualClock {
	fn advance(&self, duration: Duration) {
		let mut now = self.0.lock().unwrap();
		*now += duration;
	}
}

impl Clock for ManualClock {
	fn now(&self) -> SystemTime {
		*self.0.lock().unwrap()
	}
}

fn create_conversation(core: &Core, retention: Retention) -> Conversation {
	match core
		.execute(&actor(), request(Command::CreateConversation { retention }))
		.unwrap()
	{
		CommandOutcome::ConversationCreated(conversation) => conversation,
		other => panic!("unexpected outcome {other:?}"),
	}
}

fn create_run(core: &Core, conversation_id: ConversationId) -> Run {
	match core
		.execute(&actor(), request(Command::CreateRun { conversation_id }))
		.unwrap()
	{
		CommandOutcome::RunCreated(run) => run,
		other => panic!("unexpected outcome {other:?}"),
	}
}

fn transition(core: &Core, run: Run, lifecycle: RunLifecycle) -> Run {
	match core
		.execute(
			&actor(),
			request(Command::TransitionRun {
				run_id: run.run_id,
				expected_revision: run.revision,
				lifecycle,
			}),
		)
		.unwrap()
	{
		CommandOutcome::RunTransitioned(run) => run,
		other => panic!("unexpected outcome {other:?}"),
	}
}

fn snapshot(
	core: &Core,
	conversation_id: ConversationId,
) -> ConversationSnapshot {
	match core
		.query(&actor(), Query::Conversation { conversation_id })
		.unwrap()
	{
		QueryResult::Conversation(snapshot) => snapshot,
		other => panic!("unexpected result {other:?}"),
	}
}

fn event_kinds(core: &Core, after: EventSequence) -> Vec<(u64, EventKind)> {
	match core.query(&actor(), Query::Events { after }).unwrap() {
		QueryResult::Events(events) => events
			.into_iter()
			.map(|event| (event.sequence.0, event.kind))
			.collect(),
		other => panic!("unexpected result {other:?}"),
	}
}

#[test]
fn a_conversation_exists_and_is_queryable_before_any_run() {
	let dir = tempfile::tempdir().unwrap();
	let core = Core::start(Store::open(&dir.path().join("p.sqlite3")).unwrap())
		.unwrap();

	let conversation = create_conversation(&core, Retention::Retain);

	assert_eq!(
		snapshot(&core, conversation.conversation_id),
		ConversationSnapshot {
			cursor: EventSequence(1),
			conversation,
			runs: vec![],
		}
	);
	assert_eq!(
		event_kinds(&core, EventSequence(0)),
		vec![(
			1,
			EventKind::ConversationCreated {
				retention: Retention::Retain
			}
		)]
	);
}

#[test]
fn a_conversation_retains_its_terminal_runs_across_core_restarts() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("p.sqlite3");
	let first = Core::start(Store::open(&path).unwrap()).unwrap();
	let conversation = create_conversation(&first, Retention::Retain);
	let conversation_id = conversation.conversation_id;

	let run = create_run(&first, conversation_id);
	let run = transition(&first, run, RunLifecycle::Starting);
	let run = transition(&first, run, RunLifecycle::Active);
	let completed = transition(&first, run, RunLifecycle::Completed);
	let second_run = create_run(&first, conversation_id);
	let canceled = transition(&first, second_run, RunLifecycle::Canceled);
	drop(first);

	let second = Core::start(Store::open(&path).unwrap()).unwrap();
	let restored = snapshot(&second, conversation_id);
	let third_run = create_run(&second, conversation_id);
	let kinds = event_kinds(&second, EventSequence(6));

	assert_eq!(
		restored,
		ConversationSnapshot {
			cursor: EventSequence(7),
			conversation,
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

#[test]
fn a_second_run_is_refused_while_one_has_not_ended() {
	let dir = tempfile::tempdir().unwrap();
	let core = Core::start(Store::open(&dir.path().join("p.sqlite3")).unwrap())
		.unwrap();
	let conversation = create_conversation(&core, Retention::Retain);
	let run = create_run(&core, conversation.conversation_id);
	transition(&core, run, RunLifecycle::Starting);

	let error = core
		.execute(
			&actor(),
			request(Command::CreateRun {
				conversation_id: conversation.conversation_id,
			}),
		)
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
		}
	);
	assert_eq!(snapshot(&core, conversation.conversation_id).runs.len(), 1);
}

#[test]
fn a_run_lifecycle_only_moves_forward_and_never_leaves_a_terminal_state() {
	let dir = tempfile::tempdir().unwrap();
	let core = Core::start(Store::open(&dir.path().join("p.sqlite3")).unwrap())
		.unwrap();
	let conversation = create_conversation(&core, Retention::Retain);
	let run = create_run(&core, conversation.conversation_id);
	let refused = |run: Run, lifecycle: RunLifecycle| {
		core.execute(
			&actor(),
			request(Command::TransitionRun {
				run_id: run.run_id,
				expected_revision: run.revision,
				lifecycle,
			}),
		)
		.unwrap_err()
	};

	let skipped = refused(run, RunLifecycle::Active);
	let never_active = refused(run, RunLifecycle::Completed);
	let failed = transition(&core, run, RunLifecycle::Failed);
	let revived = refused(failed, RunLifecycle::Active);

	let invalid = |message: &str| CoreError {
		category: ErrorCategory::Conflict,
		code: "run.invalid_transition".into(),
		retryable: false,
		message: message.into(),
		detail: None,
		revision_conflict: None,
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

#[test]
fn an_unknown_conversation_or_run_is_not_found() {
	let dir = tempfile::tempdir().unwrap();
	let core = Core::start(Store::open(&dir.path().join("p.sqlite3")).unwrap())
		.unwrap();
	let conversation_id = ConversationId(Uuid::now_v7());
	let run_id = RunId(Uuid::now_v7());

	let queried = core
		.query(&actor(), Query::Conversation { conversation_id })
		.unwrap_err();
	let run_created = core
		.execute(&actor(), request(Command::CreateRun { conversation_id }))
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
		.unwrap_err();

	assert_eq!(
		(queried.code, run_created.code, transitioned.code),
		(
			"conversation.not_found".to_string(),
			"conversation.not_found".to_string(),
			"run.not_found".to_string()
		)
	);
	assert_eq!(
		(
			queried.category,
			run_created.category,
			transitioned.category
		),
		(
			ErrorCategory::NotFound,
			ErrorCategory::NotFound,
			ErrorCategory::NotFound
		)
	);
}

#[test]
fn a_command_identity_older_than_thirty_days_cannot_execute_again() {
	let dir = tempfile::tempdir().unwrap();
	let clock = Arc::new(ManualClock(Mutex::new(
		SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
	)));
	let core = Core::start_with_clock(
		Store::open(&dir.path().join("p.sqlite3")).unwrap(),
		clock.clone(),
	)
	.unwrap();
	let command_id = command_id();
	let command = Command::CreateConversation {
		retention: Retention::Retain,
	};
	let original = core
		.execute(&actor(), request_with_id(command_id, command))
		.unwrap();
	clock.advance(Duration::from_hours(30 * 24));
	let within_window = core
		.execute(&actor(), request_with_id(command_id, command))
		.unwrap();
	clock.advance(Duration::from_millis(1));

	let error = core
		.execute(&actor(), request_with_id(command_id, command))
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
		}
	);
	let QueryResult::Conversations(conversations) =
		core.query(&actor(), Query::Conversations).unwrap()
	else {
		panic!("expected the Conversation list");
	};
	let CommandOutcome::ConversationCreated(original) = original else {
		panic!("expected the original Conversation");
	};
	assert_eq!(within_window, CommandOutcome::ConversationCreated(original));
	assert_eq!(conversations.conversations, vec![original]);
}
