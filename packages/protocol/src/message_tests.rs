use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{
	ClientMessage, ErrorCategory, PlaneStatus, QueryRequest, QueryResponse,
	RecoveryAction, ServerMessage, WireError,
};
use crate::conversation::{
	CommandRequest, CommandResponse, ConflictState, Conversation,
	ConversationSnapshot, Retention, RevisionConflict, Run, RunLifecycle,
};
use crate::event::{Actor, Event};
use crate::handshake::{ClientHello, ServerHello, VersionRange};

fn json(value: &impl serde::Serialize) -> String {
	serde_json::to_string(value).unwrap()
}

#[test]
fn client_hello_has_the_agreed_wire_shape() {
	let hello = ClientHello {
		protocol: VersionRange { min: 1, max: 1 },
		codec: "json-v1".into(),
		client_id: Uuid::nil(),
		max_control_frame: 1_048_576,
		max_data_frame: 262_144,
		capabilities: vec![],
	};
	assert_eq!(
		json(&hello),
		r#"{"protocol":{"min":1,"max":1},"codec":"json-v1","client_id":"00000000-0000-0000-0000-000000000000","max_control_frame":1048576,"max_data_frame":262144,"capabilities":[]}"#
	);
}

#[test]
fn server_hello_variants_have_the_agreed_wire_shape() {
	let welcome = ServerHello::Welcome {
		protocol: 1,
		codec: "json-v1".into(),
		max_control_frame: 1_048_576,
		max_data_frame: 262_144,
		capabilities: vec![],
	};
	let rejected = ServerHello::Rejected {
		error: WireError {
			category: ErrorCategory::Incompatible,
			code: "protocol.unsupported_version".into(),
			retryable: false,
			message: "no common protocol version".into(),
			revision_conflict: None,
			recovery_actions: vec![],
		},
	};
	assert_eq!(
		(json(&welcome), json(&rejected)),
		(
			r#"{"kind":"welcome","protocol":1,"codec":"json-v1","max_control_frame":1048576,"max_data_frame":262144,"capabilities":[]}"#.to_string(),
			r#"{"kind":"rejected","error":{"category":"incompatible","code":"protocol.unsupported_version","retryable":false,"message":"no common protocol version"}}"#.to_string(),
		)
	);
}

#[test]
fn status_query_and_result_have_the_agreed_wire_shape() {
	let query = ClientMessage::Query {
		id: 1,
		query: QueryRequest::Status,
	};
	let result = ServerMessage::QueryResult {
		id: 1,
		result: QueryResponse::Status(PlaneStatus {
			plane_id: Uuid::nil(),
			daemon_starts: 2,
			started_at_unix_ms: 1_700_000_000_000,
			core_version: "0.1.0".into(),
		}),
	};
	assert_eq!(
		(json(&query), json(&result)),
		(
			r#"{"kind":"query","id":1,"query":{"type":"status"}}"#.to_string(),
			r#"{"kind":"query_result","id":1,"result":{"type":"status","plane_id":"00000000-0000-0000-0000-000000000000","daemon_starts":2,"started_at_unix_ms":1700000000000,"core_version":"0.1.0"}}"#.to_string(),
		)
	);
}

#[test]
fn error_messages_carry_a_stable_error_body() {
	let error = ServerMessage::Error {
		id: Some(4),
		error: WireError {
			category: ErrorCategory::Unavailable,
			code: "store.unavailable".into(),
			retryable: true,
			message: "the Plane store is unavailable".into(),
			revision_conflict: None,
			recovery_actions: vec![],
		},
	};
	assert_eq!(
		json(&error),
		r#"{"kind":"error","id":4,"error":{"category":"unavailable","code":"store.unavailable","retryable":true,"message":"the Plane store is unavailable"}}"#
	);
}

#[test]
fn conversation_commands_and_results_have_the_agreed_wire_shape() {
	let command = ClientMessage::Command {
		id: 2,
		command_id: Uuid::nil(),
		command: CommandRequest::CreateConversation {
			retention: Retention::Retain,
		},
	};
	let result = ServerMessage::CommandResult {
		id: 2,
		result: CommandResponse::ConversationCreated(Conversation {
			conversation_id: Uuid::nil(),
			retention: Retention::Retain,
			created_at_unix_ms: 1_700_000_000_000,
		}),
	};
	assert_eq!(
		(json(&command), json(&result)),
		(
			r#"{"kind":"command","id":2,"command_id":"00000000-0000-0000-0000-000000000000","command":{"type":"create_conversation","retention":"retain"}}"#.to_string(),
			r#"{"kind":"command_result","id":2,"result":{"type":"conversation_created","conversation_id":"00000000-0000-0000-0000-000000000000","retention":"retain","created_at_unix_ms":1700000000000}}"#.to_string(),
		)
	);
}

#[test]
fn revision_preconditions_and_conflicts_have_the_agreed_wire_shape() {
	let run = Run {
		run_id: Uuid::nil(),
		conversation_id: Uuid::nil(),
		revision: 3,
		lifecycle: RunLifecycle::Active,
		created_at_unix_ms: 1,
		ended_at_unix_ms: None,
	};
	let command = ClientMessage::Command {
		id: 5,
		command_id: Uuid::nil(),
		command: CommandRequest::TransitionRun {
			run_id: Uuid::nil(),
			expected_revision: 2,
			lifecycle: RunLifecycle::Active,
		},
	};
	let conflict = ServerMessage::Error {
		id: Some(5),
		error: WireError {
			category: ErrorCategory::Conflict,
			code: "run.revision_conflict".into(),
			retryable: false,
			message: "the Run changed since the Command was prepared".into(),
			revision_conflict: Some(RevisionConflict {
				current_revision: 3,
				safe_state: ConflictState::Run { run },
			}),
			recovery_actions: vec![RecoveryAction::RefreshRun {
				run_id: Uuid::nil(),
			}],
		},
	};

	assert_eq!(
		(json(&command), json(&conflict)),
		(
			r#"{"kind":"command","id":5,"command_id":"00000000-0000-0000-0000-000000000000","command":{"type":"transition_run","run_id":"00000000-0000-0000-0000-000000000000","expected_revision":2,"lifecycle":"active"}}"#.to_string(),
			r#"{"kind":"error","id":5,"error":{"category":"conflict","code":"run.revision_conflict","retryable":false,"message":"the Run changed since the Command was prepared","revision_conflict":{"current_revision":3,"safe_state":{"type":"run","run":{"run_id":"00000000-0000-0000-0000-000000000000","conversation_id":"00000000-0000-0000-0000-000000000000","revision":3,"lifecycle":"active","created_at_unix_ms":1,"ended_at_unix_ms":null}}},"recovery_actions":[{"type":"refresh_run","run_id":"00000000-0000-0000-0000-000000000000"}]}}"#.to_string(),
		)
	);
}

#[test]
fn a_create_conversation_command_retains_by_default() {
	use crate::decode_control;

	let command: ClientMessage = decode_control(
		br#"{"kind":"command","id":2,"command_id":"00000000-0000-0000-0000-000000000000","command":{"type":"create_conversation"}}"#,
	)
	.unwrap();

	assert_eq!(
		command,
		ClientMessage::Command {
			id: 2,
			command_id: Uuid::nil(),
			command: CommandRequest::CreateConversation {
				retention: Retention::Retain,
			},
		}
	);
}

#[test]
fn conversation_snapshots_and_events_have_the_agreed_wire_shape() {
	let snapshot = QueryResponse::Conversation(ConversationSnapshot {
		cursor: 3,
		conversation: Conversation {
			conversation_id: Uuid::nil(),
			retention: Retention::ForgetAfterFinalRun,
			created_at_unix_ms: 1,
		},
		runs: vec![Run {
			run_id: Uuid::nil(),
			conversation_id: Uuid::nil(),
			revision: 4,
			lifecycle: RunLifecycle::Completed,
			created_at_unix_ms: 2,
			ended_at_unix_ms: Some(3),
		}],
	});
	let events = QueryResponse::Events {
		events: vec![Event {
			sequence: 3,
			event_id: Uuid::nil(),
			actor: Actor::InteractiveClient {
				client_id: Uuid::nil(),
			},
			recorded_at_unix_ms: 3,
			conversation_id: Some(Uuid::nil()),
			run_id: None,
			kind: "run.lifecycle_changed".into(),
			payload: serde_json::json!({"from": "active", "to": "completed"}),
		}],
	};
	assert_eq!(
		(json(&snapshot), json(&events)),
		(
			r#"{"type":"conversation","cursor":3,"conversation":{"conversation_id":"00000000-0000-0000-0000-000000000000","retention":"forget_after_final_run","created_at_unix_ms":1},"runs":[{"run_id":"00000000-0000-0000-0000-000000000000","conversation_id":"00000000-0000-0000-0000-000000000000","revision":4,"lifecycle":"completed","created_at_unix_ms":2,"ended_at_unix_ms":3}]}"#.to_string(),
			r#"{"type":"events","events":[{"sequence":3,"event_id":"00000000-0000-0000-0000-000000000000","actor":{"type":"interactive_client","client_id":"00000000-0000-0000-0000-000000000000"},"recorded_at_unix_ms":3,"conversation_id":"00000000-0000-0000-0000-000000000000","run_id":null,"kind":"run.lifecycle_changed","payload":{"from":"active","to":"completed"}}]}"#.to_string(),
		)
	);
}
