use pretty_assertions::assert_eq;
use uuid::Uuid;

use jet_store::{EffectKindRecord, EffectSafetyRecord, NewEffect};

use crate::effect::{
	Effect, EffectAdapter, EffectKind, EffectResult, EffectSafety, EffectState,
};
use crate::test_support::{actor, start_core};
use crate::{
	Command, CommandEnvelope, CommandId, CommandOutcome, ConversationId, Core,
	Query, QueryResult, RetentionPolicy, Run, RunLifecycle,
};

fn execute(
	core: &Core,
	command_id: CommandId,
	command: Command,
) -> CommandOutcome {
	let bytes = serde_json::to_vec(&command).unwrap();
	core.execute(
		&actor(),
		CommandEnvelope::new(command_id, command, &bytes).unwrap(),
	)
	.unwrap()
}

fn create_run(core: &Core) -> (ConversationId, Run) {
	let CommandOutcome::ConversationCreated(conversation) = execute(
		core,
		CommandId(Uuid::now_v7()),
		Command::CreateConversation {
			retention: RetentionPolicy::Retain,
		},
	) else {
		panic!("expected a Conversation");
	};
	let CommandOutcome::RunCreated(run) = execute(
		core,
		CommandId(Uuid::now_v7()),
		Command::CreateRun {
			conversation_id: conversation.conversation_id,
		},
	) else {
		panic!("expected a Run");
	};
	(conversation.conversation_id, run)
}

fn queue_start(core: &Core, run: Run) -> CommandId {
	let command_id = CommandId(Uuid::now_v7());
	execute(
		core,
		command_id,
		Command::TransitionRun {
			run_id: run.run_id,
			expected_revision: run.revision,
			lifecycle: RunLifecycle::Starting,
		},
	);
	command_id
}

fn lifecycle(core: &Core, conversation_id: ConversationId) -> RunLifecycle {
	let QueryResult::Conversation(snapshot) = core
		.query(&actor(), Query::Conversation { conversation_id })
		.unwrap()
	else {
		panic!("expected a Conversation snapshot");
	};
	snapshot.runs[0].lifecycle
}

struct RecordingAdapter {
	execution: EffectResult,
	executed: Vec<Effect>,
	reconciled: Vec<Effect>,
}

impl RecordingAdapter {
	fn new(execution: EffectResult) -> Self {
		Self {
			execution,
			executed: vec![],
			reconciled: vec![],
		}
	}
}

impl EffectAdapter for RecordingAdapter {
	fn execute(&mut self, effect: &Effect) -> EffectResult {
		self.executed.push(effect.clone());
		self.execution
	}

	fn reconcile(&mut self, effect: &Effect) -> EffectResult {
		self.reconciled.push(effect.clone());
		EffectResult::Unknown
	}
}

fn assert_no_work_remains(path: &std::path::Path) {
	let core = start_core(path);
	let mut adapter = RecordingAdapter::new(EffectResult::Unknown);
	assert_eq!(core.reconcile_effects(&mut adapter).unwrap(), vec![]);
	assert_eq!((adapter.executed, adapter.reconciled), (vec![], vec![]));
}

#[test]
fn a_starting_run_and_its_effect_commit_before_external_work_begins() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let first = start_core(&path);
	let (conversation_id, run) = create_run(&first);
	let command_id = queue_start(&first, run);
	assert_eq!(lifecycle(&first, conversation_id), RunLifecycle::Starting);
	drop(first);

	let restarted = start_core(&path);
	assert_eq!(
		lifecycle(&restarted, conversation_id),
		RunLifecycle::Starting
	);
	let mut adapter = RecordingAdapter::new(EffectResult::Completed);
	let reconciled = restarted.reconcile_effects(&mut adapter).unwrap();
	let attempted = adapter.executed[0].clone();
	let expected_attempt = Effect {
		effect_id: attempted.effect_id,
		command_id,
		kind: EffectKind::StartRun { run_id: run.run_id },
		safety: EffectSafety::Idempotent {
			external_key: attempted.effect_id,
			max_attempts: 3,
		},
		state: EffectState::InFlight,
		attempt_count: 1,
	};

	assert_eq!(attempted, expected_attempt);
	assert_eq!(
		reconciled,
		vec![Effect {
			state: EffectState::Completed,
			..expected_attempt
		}]
	);
	drop(restarted);
	assert_no_work_remains(&path);
}

#[test]
fn an_idempotent_effect_resumes_under_the_same_identity_after_interruption() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let first = start_core(&path);
	let (_, run) = create_run(&first);
	let command_id = queue_start(&first, run);
	let mut interrupted_adapter = RecordingAdapter::new(EffectResult::Unknown);
	let first_pass = first.reconcile_effects(&mut interrupted_adapter).unwrap();
	let interrupted = interrupted_adapter.executed[0].clone();
	let expected_interrupted = Effect {
		effect_id: interrupted.effect_id,
		command_id,
		kind: EffectKind::StartRun { run_id: run.run_id },
		safety: EffectSafety::Idempotent {
			external_key: interrupted.effect_id,
			max_attempts: 3,
		},
		state: EffectState::InFlight,
		attempt_count: 1,
	};

	assert_eq!(interrupted, expected_interrupted);
	assert_eq!(first_pass, vec![expected_interrupted.clone()]);
	drop(first);

	let second = start_core(&path);
	let mut retry_adapter = RecordingAdapter::new(EffectResult::Failed);
	let second_pass = second.reconcile_effects(&mut retry_adapter).unwrap();
	let retried = retry_adapter.executed[0].clone();
	let expected_retried = Effect {
		attempt_count: 2,
		..expected_interrupted.clone()
	};

	assert_eq!(retry_adapter.reconciled, vec![expected_interrupted]);
	assert_eq!(retried, expected_retried);
	assert_eq!(
		second_pass,
		vec![Effect {
			state: EffectState::Failed,
			..expected_retried
		}]
	);
	drop(second);
	assert_no_work_remains(&path);
}

#[test]
fn an_ambiguous_interrupted_effect_becomes_outcome_unknown_without_retry() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let first = start_core(&path);
	let (_, run) = create_run(&first);
	first
		.store
		.write(|tx| {
			tx.insert_effect(&NewEffect {
				effect_id: Uuid::now_v7(),
				command_id: Uuid::now_v7(),
				run_id: Some(run.run_id.0),
				kind: EffectKindRecord::StartRun,
				safety: EffectSafetyRecord::Ambiguous,
			})
		})
		.unwrap();
	let mut interrupted_adapter = RecordingAdapter::new(EffectResult::Unknown);
	first.reconcile_effects(&mut interrupted_adapter).unwrap();
	let interrupted = interrupted_adapter.executed[0].clone();
	drop(first);

	let second = start_core(&path);
	let mut unknown_adapter = RecordingAdapter::new(EffectResult::Completed);
	let second_pass = second.reconcile_effects(&mut unknown_adapter).unwrap();

	assert_eq!(unknown_adapter.reconciled, vec![interrupted.clone()]);
	assert_eq!(unknown_adapter.executed, vec![]);
	assert_eq!(
		second_pass,
		vec![Effect {
			state: EffectState::OutcomeUnknown,
			..interrupted
		}]
	);
	drop(second);
	assert_no_work_remains(&path);
}
