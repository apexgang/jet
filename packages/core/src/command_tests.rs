use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::{
	Actor, ClientId, Command, CommandOutcome, Conversation, ConversationId,
	ConversationSnapshot, Core, CoreError, ErrorCategory, EventKind,
	EventSequence, Query, QueryResult, Retention, Run, RunId, RunLifecycle,
};
use jet_store::Store;

fn actor() -> Actor {
	Actor::InteractiveClient {
		client_id: ClientId(Uuid::nil()),
	}
}

fn create_conversation(core: &Core, retention: Retention) -> Conversation {
	match core
		.execute(&actor(), Command::CreateConversation { retention })
		.unwrap()
	{
		CommandOutcome::ConversationCreated(conversation) => conversation,
		other => panic!("unexpected outcome {other:?}"),
	}
}

fn create_run(core: &Core, conversation_id: ConversationId) -> Run {
	match core
		.execute(&actor(), Command::CreateRun { conversation_id })
		.unwrap()
	{
		CommandOutcome::RunCreated(run) => run,
		other => panic!("unexpected outcome {other:?}"),
	}
}

fn transition(core: &Core, run_id: RunId, lifecycle: RunLifecycle) -> Run {
	match core
		.execute(&actor(), Command::TransitionRun { run_id, lifecycle })
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
	transition(&first, run.run_id, RunLifecycle::Starting);
	transition(&first, run.run_id, RunLifecycle::Active);
	let completed = transition(&first, run.run_id, RunLifecycle::Completed);
	let second_run = create_run(&first, conversation_id);
	let canceled =
		transition(&first, second_run.run_id, RunLifecycle::Canceled);
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
	transition(&core, run.run_id, RunLifecycle::Starting);

	let error = core
		.execute(
			&actor(),
			Command::CreateRun {
				conversation_id: conversation.conversation_id,
			},
		)
		.unwrap_err();

	assert_eq!(
		error,
		CoreError {
			category: ErrorCategory::Conflict,
			code: "run.conversation_busy",
			retryable: false,
			message: "the Conversation already has a Run that has not ended"
				.into(),
			detail: None,
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
	let refused = |lifecycle: RunLifecycle| {
		core.execute(
			&actor(),
			Command::TransitionRun {
				run_id: run.run_id,
				lifecycle,
			},
		)
		.unwrap_err()
	};

	let skipped = refused(RunLifecycle::Active);
	let never_active = refused(RunLifecycle::Completed);
	transition(&core, run.run_id, RunLifecycle::Failed);
	let revived = refused(RunLifecycle::Active);

	let invalid = |message: &str| CoreError {
		category: ErrorCategory::Conflict,
		code: "run.invalid_transition",
		retryable: false,
		message: message.into(),
		detail: None,
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
		.execute(&actor(), Command::CreateRun { conversation_id })
		.unwrap_err();
	let transitioned = core
		.execute(
			&actor(),
			Command::TransitionRun {
				run_id,
				lifecycle: RunLifecycle::Starting,
			},
		)
		.unwrap_err();

	assert_eq!(
		(queried.code, run_created.code, transitioned.code),
		(
			"conversation.not_found",
			"conversation.not_found",
			"run.not_found"
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
