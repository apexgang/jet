use std::time::SystemTime;

use jet_store::{
	ActorRecord, CommandReceiptRecord, RetentionPolicy, RunLifecycle,
};
use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{
	OUTCOME_VERSION, PREVIOUS_OUTCOME_VERSION, encode_result, outcome_version,
	replay,
};
use crate::{
	CommandOutcome, ConflictState, Conversation, ConversationId,
	ConversationOrigin, CoreError, Name, NameSource, Revision,
	RevisionConflict, Run, RunId, WorkingTree,
};

const DIGEST: [u8; 32] = [7; 32];

fn receipt(version: u32, outcome: String) -> CommandReceiptRecord {
	CommandReceiptRecord {
		actor: ActorRecord::InteractiveClient {
			client_id: Uuid::nil(),
		},
		command_id: Uuid::now_v7(),
		request_digest: Some(DIGEST),
		recorded_at_unix_ms: 0,
		outcome_version: Some(version),
		outcome: Some(outcome),
	}
}

fn deterministic_name(kind: &str, id: Uuid) -> Name {
	Name {
		value: format!("{kind} {}", &id.simple().to_string()[..8]),
		source: NameSource::Deterministic,
	}
}

#[test]
fn version_one_entity_outcomes_replay_with_their_deterministic_names() {
	let conversation_id = ConversationId(
		Uuid::parse_str("12345678-1234-5678-9234-567812345678").unwrap(),
	);
	let run_id =
		RunId(Uuid::parse_str("87654321-1234-5678-9234-567812345678").unwrap());
	let conversation = Conversation {
		conversation_id,
		revision: Revision(1),
		retention: RetentionPolicy::Retain,
		working_tree: WorkingTree::NoProject,
		origin: ConversationOrigin::New,
		name: deterministic_name("Conversation", conversation_id.0),
		created_at: SystemTime::UNIX_EPOCH,
	};
	let run = Run {
		run_id,
		conversation_id,
		revision: Revision(3),
		lifecycle: RunLifecycle::Active,
		name: deterministic_name("Run", run_id.0),
		created_at: SystemTime::UNIX_EPOCH,
		ended_at: None,
	};
	let mut old_conversation = serde_json::to_value(Ok::<_, CoreError>(
		CommandOutcome::ConversationCreated(conversation.clone()),
	))
	.unwrap();
	let conversation_payload = old_conversation["Ok"]["ConversationCreated"]
		.as_object_mut()
		.unwrap();
	conversation_payload.remove("revision");
	conversation_payload.remove("name");
	let mut old_run = serde_json::to_value(Ok::<_, CoreError>(
		CommandOutcome::RunTransitioned(run.clone()),
	))
	.unwrap();
	old_run["Ok"]["RunTransitioned"]
		.as_object_mut()
		.unwrap()
		.remove("name");

	let replayed_conversation = replay(
		receipt(1, serde_json::to_string(&old_conversation).unwrap()),
		DIGEST,
		1,
	)
	.unwrap();
	let replayed_run = replay(
		receipt(1, serde_json::to_string(&old_run).unwrap()),
		DIGEST,
		1,
	)
	.unwrap();

	assert_eq!(
		(replayed_conversation, replayed_run),
		(
			Ok(CommandOutcome::ConversationCreated(conversation)),
			Ok(CommandOutcome::RunTransitioned(run)),
		)
	);
}

#[test]
fn current_receipts_round_trip_without_compatibility_rewriting() {
	let result = Err(CoreError::invalid_input("test.refused", "refused"));
	let encoded = encode_result(&result).unwrap();

	let replayed =
		replay(receipt(OUTCOME_VERSION, encoded), DIGEST, 1).unwrap();

	assert_eq!(replayed, result);
}

#[test]
fn version_one_revision_conflicts_replay_with_their_deterministic_run_name() {
	let conversation_id = ConversationId(Uuid::now_v7());
	let run_id =
		RunId(Uuid::parse_str("87654321-1234-5678-9234-567812345678").unwrap());
	let run = Run {
		run_id,
		conversation_id,
		revision: Revision(3),
		lifecycle: RunLifecycle::Active,
		name: deterministic_name("Run", run_id.0),
		created_at: SystemTime::UNIX_EPOCH,
		ended_at: None,
	};
	let expected = CoreError::revision_conflict(
		"run.revision_conflict",
		"the Run changed",
		RevisionConflict {
			current_revision: run.revision,
			safe_state: ConflictState::Run(run.clone()),
		},
	);
	let mut previous =
		serde_json::to_value(Err::<CommandOutcome, _>(expected.clone()))
			.unwrap();
	previous["Err"]["revision_conflict"]["safe_state"]["Run"]
		.as_object_mut()
		.unwrap()
		.remove("name");

	let replayed = replay(
		receipt(
			PREVIOUS_OUTCOME_VERSION,
			serde_json::to_string(&previous).unwrap(),
		),
		DIGEST,
		1,
	)
	.unwrap();

	assert_eq!(replayed, Err(expected));
}

#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
enum PreviousCommandOutcome {
	ConversationCreated(PreviousConversation),
}

#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
struct PreviousConversation {
	conversation_id: ConversationId,
	retention: RetentionPolicy,
	working_tree: WorkingTree,
	origin: ConversationOrigin,
	created_at: SystemTime,
}

#[test]
fn established_outcomes_stay_readable_by_the_rollback_release() {
	let conversation_id = ConversationId(Uuid::now_v7());
	let conversation = Conversation {
		conversation_id,
		revision: Revision(1),
		retention: RetentionPolicy::Retain,
		working_tree: WorkingTree::NoProject,
		origin: ConversationOrigin::New,
		name: deterministic_name("Conversation", conversation_id.0),
		created_at: SystemTime::UNIX_EPOCH,
	};
	let result = Ok::<_, CoreError>(CommandOutcome::ConversationCreated(
		conversation.clone(),
	));
	let encoded = encode_result(&result).unwrap();
	let decoded: Result<PreviousCommandOutcome, CoreError> =
		serde_json::from_str(&encoded).unwrap();

	assert_eq!(outcome_version(&result), PREVIOUS_OUTCOME_VERSION);
	assert_eq!(
		decoded,
		Ok(PreviousCommandOutcome::ConversationCreated(
			PreviousConversation {
				conversation_id,
				retention: conversation.retention,
				working_tree: conversation.working_tree,
				origin: conversation.origin,
				created_at: conversation.created_at,
			}
		))
	);
	assert_eq!(
		outcome_version(&Ok(CommandOutcome::ConversationNamed(conversation))),
		OUTCOME_VERSION
	);
}
