use jet_store::{EventRecord, ForkContextEvents};
use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{
	ForkContextEntry, ForkContextRole, ForkLaunchContext, capture_context,
};
use crate::event::EventSubject;
use crate::test_support::actor;
use crate::{
	ClientId, ConversationId, EventKind, RunId, Turn, TurnSource, TurnState,
};

#[test]
fn fork_context_orders_delivery_and_omits_unlaunched_input() {
	let conversation_id = ConversationId(Uuid::now_v7());
	let run_id = RunId(Uuid::now_v7());
	let unlaunched_run_id = RunId(Uuid::now_v7());
	let executed_turn_id = Uuid::now_v7();
	let queued_turn_id = Uuid::now_v7();
	let unlaunched_turn_id = Uuid::now_v7();
	let subject = EventSubject::Run {
		conversation_id,
		run_id,
	};
	let events = vec![
		record(
			1,
			EventKind::TurnInput {
				turn_id: executed_turn_id,
				text: "first ".into(),
			},
			subject,
		),
		record(
			2,
			EventKind::TurnInput {
				turn_id: executed_turn_id,
				text: "second".into(),
			},
			subject,
		),
		record(
			3,
			EventKind::TurnChanged {
				turn: Turn {
					turn_id: executed_turn_id,
					sequence: 1,
					client_id: ClientId(Uuid::now_v7()),
					source: TurnSource::User,
					state: TurnState::Active,
					run_id: Some(run_id),
				},
			},
			subject,
		),
		record(
			4,
			EventKind::TurnInput {
				turn_id: queued_turn_id,
				text: "second executed instruction".into(),
			},
			subject,
		),
		record(
			5,
			EventKind::RunOutput {
				native_json: String::new(),
				presentation_json: vec!["assistant response".into()],
			},
			subject,
		),
		record(
			6,
			EventKind::TurnChanged {
				turn: Turn {
					turn_id: queued_turn_id,
					sequence: 2,
					client_id: ClientId(Uuid::now_v7()),
					source: TurnSource::User,
					state: TurnState::Active,
					run_id: Some(run_id),
				},
			},
			subject,
		),
		record(
			7,
			EventKind::TurnInput {
				turn_id: unlaunched_turn_id,
				text: "never reached the harness".into(),
			},
			EventSubject::Run {
				conversation_id,
				run_id: unlaunched_run_id,
			},
		),
		record(
			8,
			EventKind::TurnChanged {
				turn: Turn {
					turn_id: unlaunched_turn_id,
					sequence: 3,
					client_id: ClientId(Uuid::now_v7()),
					source: TurnSource::User,
					state: TurnState::Active,
					run_id: Some(unlaunched_run_id),
				},
			},
			EventSubject::Run {
				conversation_id,
				run_id: unlaunched_run_id,
			},
		),
	];

	assert_eq!(
		capture_context(ForkContextEvents {
			events,
			checkpoint_counts: [(run_id.0, 2)].into_iter().collect(),
			earlier_events_omitted: false,
		})
		.unwrap(),
		ForkLaunchContext {
			history_truncated: false,
			entries: vec![
				ForkContextEntry {
					role: ForkContextRole::User,
					content: "first second".into(),
					truncated: false,
				},
				ForkContextEntry {
					role: ForkContextRole::Assistant,
					content: "assistant response".into(),
					truncated: false,
				},
				ForkContextEntry {
					role: ForkContextRole::User,
					content: "second executed instruction".into(),
					truncated: false,
				},
			],
		}
	);
}

fn record(
	sequence: u64,
	kind: EventKind,
	subject: EventSubject,
) -> EventRecord {
	let event = kind.to_record(&actor(), subject, 0).unwrap();
	EventRecord {
		sequence,
		event_id: event.event_id,
		actor: event.actor,
		recorded_at_unix_ms: event.recorded_at_unix_ms,
		conversation_id: event.conversation_id,
		run_id: event.run_id,
		kind: event.kind,
		payload_version: event.payload_version,
		payload: event.payload,
	}
}
