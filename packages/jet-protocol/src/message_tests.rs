use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{
	ClientMessage, ErrorCategory, EventPage, PlaneStatus, QueryRequest,
	QueryResponse, RecoveryAction, ServerMessage, WireError, raw_command,
};
use crate::audit::{
	AuditBreach, AuditEntry, AuditHead, AuditOutcome, AuditRisk, AuditTarget,
	SecurityAudit, SecurityState,
};
use crate::capability::{CapabilityObservation, ToolAvailability};
use crate::conversation::{
	CommandRequest, CommandResponse, ConflictState, Conversation,
	ConversationSnapshot, RetentionPolicy, RevisionConflict, Run, RunLifecycle,
};
use crate::event::{Actor, Event};
use crate::handshake::{ClientHello, ServerHello, VersionRange};
use crate::pairing::{
	ClientPublicKey, PairedClient, PairedClientAccess, PairingGate,
	PairingKeyAlgorithm, PairingMethod, PairingProgress, PairingSnapshot,
	PendingPairing,
};
use crate::project::{
	Checkout, EntryKind, GitLink, Project, ProjectEntry, ProjectList,
	ProjectPreview, Registrability, Repository, Worktree,
};
use crate::promotion::{
	ChangeKind, ConflictKind, PromotedChange, PromotionBinding,
	PromotionConflict, PromotionDestination, PromotionPreview,
};
use crate::workspace::{
	BaseSelection, SeedSelection, WorkingTreeRequest, Workspace, WorkspaceBase,
	WorkspaceSeed,
};
use crate::{ControlError, decode_control};

fn json(value: &impl serde::Serialize) -> String {
	serde_json::to_string(value).unwrap()
}

#[test]
fn client_hello_has_the_agreed_wire_shape() {
	let hello = ClientHello {
		protocol: VersionRange { min: 1, max: 1 },
		minor: 0,
		codec: "json-v1".into(),
		client_id: Uuid::nil(),
		max_control_frame: 1_048_576,
		max_data_frame: 262_144,
		capabilities: vec![],
	};
	assert_eq!(
		json(&hello),
		r#"{"protocol":{"min":1,"max":1},"minor":0,"codec":"json-v1","client_id":"00000000-0000-0000-0000-000000000000","max_control_frame":1048576,"max_data_frame":262144,"capabilities":[]}"#
	);
}

#[test]
fn server_hello_variants_have_the_agreed_wire_shape() {
	let welcome = ServerHello::Welcome {
		protocol: 1,
		minor: 0,
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
			restart: None,
			recovery_actions: vec![],
		},
	};
	assert_eq!(
		(json(&welcome), json(&rejected)),
		(
			r#"{"kind":"welcome","protocol":1,"minor":0,"codec":"json-v1","max_control_frame":1048576,"max_data_frame":262144,"capabilities":[]}"#.to_string(),
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
			cursor: Some(0),
			plane_id: Uuid::nil(),
			daemon_starts: 2,
			started_at_unix_ms: 1_700_000_000_000,
			core_version: "0.1.0".into(),
			security: None,
		}),
	};
	assert_eq!(
		(json(&query), json(&result)),
		(
			r#"{"kind":"query","id":1,"query":{"type":"status"}}"#.to_string(),
			r#"{"kind":"query_result","id":1,"result":{"type":"status","cursor":"0","plane_id":"00000000-0000-0000-0000-000000000000","daemon_starts":2,"started_at_unix_ms":1700000000000,"core_version":"0.1.0"}}"#.to_string(),
		)
	);
}

#[test]
fn a_minor_zero_status_without_a_fence_remains_readable() {
	let message = decode_control::<ServerMessage>(
		br#"{"kind":"query_result","id":1,"result":{"type":"status","plane_id":"00000000-0000-0000-0000-000000000000","daemon_starts":2,"started_at_unix_ms":1700000000000,"core_version":"0.1.0"}}"#,
	)
	.unwrap();

	let ServerMessage::QueryResult {
		result: QueryResponse::Status(status),
		..
	} = message
	else {
		panic!("expected a status result");
	};
	assert_eq!(status.cursor, None);
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
			restart: None,
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
			retention: RetentionPolicy::Retain,
			working_tree: WorkingTreeRequest::NoProject,
		},
	};
	let result = ServerMessage::CommandResult {
		id: 2,
		result: CommandResponse::ConversationCreated(Conversation {
			conversation_id: Uuid::nil(),
			retention: RetentionPolicy::Retain,
			working_tree: None,
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
			restart: None,
			recovery_actions: vec![RecoveryAction::RefreshRun {
				run_id: Uuid::nil(),
			}],
		},
	};

	assert_eq!(
		(json(&command), json(&conflict)),
		(
			r#"{"kind":"command","id":5,"command_id":"00000000-0000-0000-0000-000000000000","command":{"type":"transition_run","run_id":"00000000-0000-0000-0000-000000000000","expected_revision":"2","lifecycle":"active"}}"#.to_string(),
			r#"{"kind":"error","id":5,"error":{"category":"conflict","code":"run.revision_conflict","retryable":false,"message":"the Run changed since the Command was prepared","revision_conflict":{"current_revision":"3","safe_state":{"type":"run","run":{"run_id":"00000000-0000-0000-0000-000000000000","conversation_id":"00000000-0000-0000-0000-000000000000","revision":"3","lifecycle":"active","created_at_unix_ms":1,"ended_at_unix_ms":null}}},"recovery_actions":[{"type":"refresh_run","run_id":"00000000-0000-0000-0000-000000000000"}]}}"#.to_string(),
		)
	);
}

#[test]
fn a_create_conversation_command_retains_by_default() {
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
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRequest::NoProject,
			},
		}
	);
}

#[test]
fn raw_command_keeps_the_exact_command_bytes_of_a_frame() {
	let frame = br#"{"kind":"command","id":2,"command_id":"00000000-0000-0000-0000-000000000000","command":{ "type" : "create_conversation" }}"#;
	let typed: ClientMessage = decode_control(frame).unwrap();

	let raw = raw_command(frame).unwrap();
	let missing = raw_command(br#"{"kind":"query","id":1}"#).unwrap_err();

	assert_eq!(
		(typed, raw.get()),
		(
			ClientMessage::Command {
				id: 2,
				command_id: Uuid::nil(),
				command: CommandRequest::CreateConversation {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRequest::NoProject,
				},
			},
			r#"{ "type" : "create_conversation" }"#
		)
	);
	assert!(matches!(missing, ControlError::Malformed(_)), "{missing:?}");
}

#[test]
fn conversation_snapshots_and_event_pages_have_the_agreed_wire_shape() {
	let snapshot = QueryResponse::Conversation(ConversationSnapshot {
		cursor: 3,
		conversation: Conversation {
			conversation_id: Uuid::nil(),
			retention: RetentionPolicy::ForgetAfterFinalRun,
			working_tree: None,
			created_at_unix_ms: 1,
		},
		workspace: None,
		runs: vec![Run {
			run_id: Uuid::nil(),
			conversation_id: Uuid::nil(),
			revision: 4,
			lifecycle: RunLifecycle::Completed,
			created_at_unix_ms: 2,
			ended_at_unix_ms: Some(3),
		}],
	});
	let events = QueryResponse::Events(EventPage {
		cursor: 3,
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
			payload_version: 1,
			payload: serde_json::json!({"from": "active", "to": "completed"}),
		}],
	});
	assert_eq!(
		(json(&snapshot), json(&events)),
		(
			r#"{"type":"conversation","cursor":"3","conversation":{"conversation_id":"00000000-0000-0000-0000-000000000000","retention":"forget_after_final_run","created_at_unix_ms":1},"runs":[{"run_id":"00000000-0000-0000-0000-000000000000","conversation_id":"00000000-0000-0000-0000-000000000000","revision":"4","lifecycle":"completed","created_at_unix_ms":2,"ended_at_unix_ms":3}]}"#.to_string(),
			r#"{"type":"events","cursor":"3","events":[{"sequence":"3","event_id":"00000000-0000-0000-0000-000000000000","actor":{"type":"interactive_client","client_id":"00000000-0000-0000-0000-000000000000"},"recorded_at_unix_ms":3,"conversation_id":"00000000-0000-0000-0000-000000000000","run_id":null,"kind":"run.lifecycle_changed","payload_version":1,"payload":{"from":"active","to":"completed"}}]}"#.to_string(),
		)
	);
}

#[test]
fn sequences_and_revisions_round_trip_as_decimal_strings() {
	let query = ClientMessage::Query {
		id: 8,
		query: QueryRequest::Events { after: u64::MAX },
	};

	let encoded = json(&query);
	let decoded: ClientMessage = decode_control(encoded.as_bytes()).unwrap();

	assert_eq!(
		(encoded, decoded),
		(
			r#"{"kind":"query","id":8,"query":{"type":"events","after":"18446744073709551615"}}"#.to_string(),
			query
		)
	);
}

#[test]
fn only_canonical_decimal_strings_are_accepted_for_sequences() {
	let decode = |after: &str| {
		decode_control::<QueryRequest>(
			format!(r#"{{"type":"events","after":{after}}}"#).as_bytes(),
		)
	};

	let rejected = ["3", r#""""#, r#""+3""#, r#""03""#, r#""-1""#, r#""3.0""#]
		.map(|after| decode(after).map(|_| after).unwrap_err());

	assert_eq!(decode(r#""0""#).unwrap(), QueryRequest::Events { after: 0 });
	assert!(
		rejected
			.iter()
			.all(|error| matches!(error, ControlError::Malformed(_))),
		"{rejected:?}"
	);
}

#[test]
fn the_security_audit_query_and_page_have_the_agreed_wire_shape() {
	let query = ClientMessage::Query {
		id: 7,
		query: QueryRequest::SecurityAudit { after: 0 },
	};
	let result = ServerMessage::QueryResult {
		id: 7,
		result: QueryResponse::SecurityAudit(SecurityAudit {
			cursor: 1,
			entries: vec![AuditEntry {
				sequence: 1,
				epoch: 1,
				record_id: Uuid::nil(),
				recorded_at_unix_ms: 1_700_000_000_000,
				plane_id: Uuid::nil(),
				actor: Actor::InteractiveClient {
					client_id: Uuid::nil(),
				},
				target: AuditTarget {
					kind: "account_binding".into(),
					reference: "00".repeat(32),
					identity: None,
				},
				decision: "account.bound".into(),
				risk: AuditRisk::Elevated,
				outcome: AuditOutcome::Succeeded,
			}],
		}),
	};
	assert_eq!(
		(json(&query), json(&result)),
		(
			r#"{"kind":"query","id":7,"query":{"type":"security_audit","after":"0"}}"#.to_string(),
			format!(
				r#"{{"kind":"query_result","id":7,"result":{{"type":"security_audit","cursor":"1","entries":[{{"sequence":"1","epoch":"1","record_id":"00000000-0000-0000-0000-000000000000","recorded_at_unix_ms":1700000000000,"plane_id":"00000000-0000-0000-0000-000000000000","actor":{{"type":"interactive_client","client_id":"00000000-0000-0000-0000-000000000000"}},"target":{{"kind":"account_binding","reference":"{}"}},"decision":"account.bound","risk":"elevated","outcome":"succeeded"}}]}}}}"#,
				"00".repeat(32)
			),
		)
	);
}

#[test]
fn beginning_an_audit_epoch_has_the_agreed_wire_shape() {
	let command = ClientMessage::Command {
		id: 8,
		command_id: Uuid::nil(),
		command: CommandRequest::BeginAuditEpoch,
	};
	let result = ServerMessage::CommandResult {
		id: 8,
		result: CommandResponse::AuditEpochBegun { epoch: 2 },
	};
	assert_eq!(
		(json(&command), json(&result)),
		(
			r#"{"kind":"command","id":8,"command_id":"00000000-0000-0000-0000-000000000000","command":{"type":"begin_audit_epoch"}}"#.to_string(),
			r#"{"kind":"command_result","id":8,"result":{"type":"audit_epoch_begun","epoch":"2"}}"#.to_string(),
		)
	);
}

#[test]
fn a_degraded_plane_reports_its_evidence_in_the_agreed_wire_shape() {
	let status = PlaneStatus {
		cursor: Some(4),
		plane_id: Uuid::nil(),
		daemon_starts: 2,
		started_at_unix_ms: 1_700_000_000_000,
		core_version: "0.2.0".into(),
		security: Some(SecurityState::Degraded {
			breach: AuditBreach::HeadNotInStore,
			epoch: 1,
			head: Some(AuditHead {
				epoch: 1,
				sequence: 2,
				entry_hash: "ab".repeat(32),
			}),
			store_sequence: 1,
		}),
	};
	assert_eq!(
		json(&status),
		format!(
			r#"{{"cursor":"4","plane_id":"00000000-0000-0000-0000-000000000000","daemon_starts":2,"started_at_unix_ms":1700000000000,"core_version":"0.2.0","security":{{"state":"degraded","breach":{{"breach":"head_not_in_store"}},"epoch":"1","head":{{"epoch":"1","sequence":"2","entry_hash":"{}"}},"store_sequence":"1"}}}}"#,
			"ab".repeat(32)
		)
	);
}

/// A Setting value the audit retention window needs, in the shape a client
/// that negotiated minor 5 reads it.
#[test]
fn a_whole_number_setting_value_has_the_agreed_wire_shape() {
	assert_eq!(
		json(&crate::setting::SettingValue::Count(365)),
		r#"{"type":"count","value":365}"#
	);
}

#[test]
fn pairing_commands_and_snapshots_have_the_agreed_wire_shape() {
	let claim = CommandRequest::ClaimPairing {
		secret: "1234-5678".into(),
		key: ClientPublicKey {
			algorithm: PairingKeyAlgorithm::Ed25519,
			key: [17; 32],
		},
	};
	let snapshot = QueryResponse::Pairing(PairingSnapshot {
		cursor: 4,
		gate: PairingGate::Open,
		pending: Some(PendingPairing {
			offer_id: Uuid::nil(),
			method: PairingMethod::QrPayload {
				endpoint: "alex@studio.example".into(),
			},
			progress: PairingProgress::AwaitingConfirmation {
				client_id: Uuid::nil(),
				authentication_string: "418-273".into(),
			},
			attempts_remaining: 5,
			opened_at_unix_ms: 1,
			expires_at_unix_ms: 121,
		}),
		clients: vec![PairedClient {
			client_id: Uuid::nil(),
			key: ClientPublicKey {
				algorithm: PairingKeyAlgorithm::Ed25519,
				key: [34; 32],
			},
			pairing_protocol: "jet.pairing.v1".into(),
			access: PairedClientAccess::Enabled,
			paired_at_unix_ms: 2,
		}],
	});

	assert_eq!(
		(json(&claim), json(&snapshot)),
		(
			r#"{"type":"claim_pairing","secret":"1234-5678","key":{"algorithm":"ed25519","key":"1111111111111111111111111111111111111111111111111111111111111111"}}"#.to_owned(),
			r#"{"type":"pairing","cursor":"4","gate":"open","pending":{"offer_id":"00000000-0000-0000-0000-000000000000","method":{"method":"qr_payload","endpoint":"alex@studio.example"},"progress":{"progress":"awaiting_confirmation","client_id":"00000000-0000-0000-0000-000000000000","authentication_string":"418-273"},"attempts_remaining":5,"opened_at_unix_ms":1,"expires_at_unix_ms":121},"clients":[{"client_id":"00000000-0000-0000-0000-000000000000","key":{"algorithm":"ed25519","key":"2222222222222222222222222222222222222222222222222222222222222222"},"pairing_protocol":"jet.pairing.v1","access":"enabled","paired_at_unix_ms":2}]}"#.to_owned()
		)
	);
}

/// A key crosses as lowercase hexadecimal of exactly the width its
/// algorithm fixes, so one key has one encoding and anything else is a
/// protocol error rather than something the Plane decides about later.
#[test]
fn only_lowercase_hexadecimal_of_the_fixed_width_is_a_key() {
	let claim = |key: &str| {
		format!(
			r#"{{"kind":"command","id":1,"command_id":"00000000-0000-0000-0000-000000000000","command":{{"type":"claim_pairing","secret":"1234-5678","key":{{"algorithm":"ed25519","key":"{key}"}}}}}}"#
		)
	};
	let lowercase =
		"1111111111111111111111111111111111111111111111111111111111111111";
	let uppercase =
		"AAAA111111111111111111111111111111111111111111111111111111111111";
	let short = "1111";

	let accepted = decode_control::<ClientMessage>(claim(lowercase).as_bytes());
	let refused = [
		decode_control::<ClientMessage>(claim(uppercase).as_bytes()),
		decode_control::<ClientMessage>(claim(short).as_bytes()),
	];

	assert_eq!(
		(
			accepted.is_ok(),
			refused.iter().all(|result| matches!(
				result,
				Err(ControlError::Malformed(_))
			))
		),
		(true, true)
	);
}

#[test]
fn project_registration_and_listing_have_the_agreed_wire_shape() {
	let command = ClientMessage::Command {
		id: 9,
		command_id: Uuid::nil(),
		command: CommandRequest::RegisterProject {
			path: "/home/jet/repo".into(),
		},
	};
	let project = Project {
		project_id: Uuid::nil(),
		root: "/home/jet/repo".into(),
		registered_by: Actor::InteractiveClient {
			client_id: Uuid::nil(),
		},
		registered_at_unix_ms: 1_700_000_000_000,
	};
	let result = ServerMessage::CommandResult {
		id: 9,
		result: CommandResponse::ProjectRegistered(project.clone()),
	};
	let query = ClientMessage::Query {
		id: 10,
		query: QueryRequest::Projects,
	};
	let list = ServerMessage::QueryResult {
		id: 10,
		result: QueryResponse::Projects(ProjectList {
			cursor: 3,
			projects: vec![project],
		}),
	};
	assert_eq!(
		(json(&command), json(&result), json(&query), json(&list)),
		(
			r#"{"kind":"command","id":9,"command_id":"00000000-0000-0000-0000-000000000000","command":{"type":"register_project","path":"/home/jet/repo"}}"#.to_string(),
			r#"{"kind":"command_result","id":9,"result":{"type":"project_registered","project_id":"00000000-0000-0000-0000-000000000000","root":"/home/jet/repo","registered_by":{"type":"interactive_client","client_id":"00000000-0000-0000-0000-000000000000"},"registered_at_unix_ms":1700000000000}}"#.to_string(),
			r#"{"kind":"query","id":10,"query":{"type":"projects"}}"#.to_string(),
			r#"{"kind":"query_result","id":10,"result":{"type":"projects","cursor":"3","projects":[{"project_id":"00000000-0000-0000-0000-000000000000","root":"/home/jet/repo","registered_by":{"type":"interactive_client","client_id":"00000000-0000-0000-0000-000000000000"},"registered_at_unix_ms":1700000000000}]}}"#.to_string(),
		)
	);
}

#[test]
fn a_project_preview_has_the_agreed_wire_shape() {
	let query = ClientMessage::Query {
		id: 11,
		query: QueryRequest::PreviewProject {
			path: "/home/jet/repo".into(),
			observation: CapabilityObservation::Fresh,
		},
	};
	let result = ServerMessage::QueryResult {
		id: 11,
		result: QueryResponse::ProjectPreview(ProjectPreview {
			root: "/home/jet/repo".into(),
			registrability: Registrability::Registrable {
				repository: Repository {
					worktree: Worktree::Linked {
						common_dir: "/home/jet/main/.git".into(),
					},
					checkout: Checkout::Sparse,
					submodules: vec![GitLink {
						path: "vendor/child".into(),
						commit: "0".repeat(40),
					}],
					lfs: ToolAvailability::Missing,
				},
			},
		}),
	};
	let refused = QueryResponse::ProjectPreview(ProjectPreview {
		root: "/home/jet/repo/src".into(),
		registrability: Registrability::InsideWorkingTree {
			toplevel: "/home/jet/repo".into(),
		},
	});
	assert_eq!(
		(json(&query), json(&result), json(&refused)),
		(
			r#"{"kind":"query","id":11,"query":{"type":"preview_project","path":"/home/jet/repo","observation":{"type":"fresh"}}}"#.to_string(),
			format!(
				r#"{{"kind":"query_result","id":11,"result":{{"type":"project_preview","root":"/home/jet/repo","registrability":{{"verdict":"registrable","repository":{{"worktree":{{"kind":"linked","common_dir":"/home/jet/main/.git"}},"checkout":"sparse","submodules":[{{"path":"vendor/child","commit":"{}"}}],"lfs":{{"status":"missing"}}}}}}}}}}"#,
				"0".repeat(40)
			),
			r#"{"type":"project_preview","root":"/home/jet/repo/src","registrability":{"verdict":"inside_working_tree","toplevel":"/home/jet/repo"}}"#.to_string(),
		)
	);
}

#[test]
fn a_project_entry_has_the_agreed_wire_shape() {
	let query = ClientMessage::Query {
		id: 12,
		query: QueryRequest::ProjectEntry {
			project_id: Uuid::nil(),
			path: "docs/adr/0101.md".into(),
		},
	};
	let result = ServerMessage::QueryResult {
		id: 12,
		result: QueryResponse::ProjectEntry(ProjectEntry {
			cursor: 3,
			project_id: Uuid::nil(),
			path: "docs/adr/0101.md".into(),
			kind: EntryKind::File { bytes: 512 },
		}),
	};
	assert_eq!(
		(json(&query), json(&result)),
		(
			r#"{"kind":"query","id":12,"query":{"type":"project_entry","project_id":"00000000-0000-0000-0000-000000000000","path":"docs/adr/0101.md"}}"#.to_string(),
			r#"{"kind":"query_result","id":12,"result":{"type":"project_entry","cursor":"3","project_id":"00000000-0000-0000-0000-000000000000","path":"docs/adr/0101.md","kind":{"type":"file","bytes":512}}}"#.to_string(),
		)
	);
}

/// A seeded Workspace request names its seed, an unseeded one leaves the
/// field out as every request before minor 10 did, and a Workspace names
/// what it was seeded with (ADR-0019, ADR-0025).
#[test]
fn workspace_seeds_have_the_agreed_wire_shape() {
	let seeded = CommandRequest::CreateConversation {
		retention: RetentionPolicy::Retain,
		working_tree: WorkingTreeRequest::Workspace {
			project_id: Uuid::nil(),
			base: BaseSelection::Head,
			seed: SeedSelection::Paths {
				paths: vec!["src/lib.rs".into()],
			},
		},
	};
	let unseeded = CommandRequest::CreateConversation {
		retention: RetentionPolicy::Retain,
		working_tree: WorkingTreeRequest::Workspace {
			project_id: Uuid::nil(),
			base: BaseSelection::Head,
			seed: SeedSelection::None,
		},
	};
	let workspace = Workspace {
		workspace_id: Uuid::nil(),
		conversation_id: Uuid::nil(),
		project_id: Uuid::nil(),
		root: "/home/jet/.jet/workspaces/x".into(),
		base: WorkspaceBase {
			selection: BaseSelection::Head,
			commit: "0123456789abcdef0123456789abcdef01234567".into(),
		},
		seed: Some(WorkspaceSeed {
			tree: "89abcdef0123456789abcdef0123456789abcdef".into(),
			changed_paths: 2,
		}),
		created_at_unix_ms: 1,
	};
	let decoded: CommandRequest =
		serde_json::from_str(&json(&unseeded)).unwrap();

	assert_eq!(
		(json(&seeded), json(&unseeded), json(&workspace), decoded),
		(
			r#"{"type":"create_conversation","retention":"retain","working_tree":{"kind":"workspace","project_id":"00000000-0000-0000-0000-000000000000","base":{"kind":"head"},"seed":{"kind":"paths","paths":["src/lib.rs"]}}}"#.to_string(),
			r#"{"type":"create_conversation","retention":"retain","working_tree":{"kind":"workspace","project_id":"00000000-0000-0000-0000-000000000000","base":{"kind":"head"}}}"#.to_string(),
			r#"{"workspace_id":"00000000-0000-0000-0000-000000000000","conversation_id":"00000000-0000-0000-0000-000000000000","project_id":"00000000-0000-0000-0000-000000000000","root":"/home/jet/.jet/workspaces/x","base":{"selection":{"kind":"head"},"commit":"0123456789abcdef0123456789abcdef01234567"},"seed":{"tree":"89abcdef0123456789abcdef0123456789abcdef","changed_paths":2},"created_at_unix_ms":1}"#.to_string(),
			unseeded,
		)
	);
}

/// A promotion preview names its destination, binds what it compared and
/// whom it was shown to, and lists every change and conflict as data
/// (ADR-0025).
#[test]
fn a_promotion_preview_has_the_agreed_wire_shape() {
	let query = ClientMessage::Query {
		id: 13,
		query: QueryRequest::PreviewPromotion {
			workspace_id: Uuid::nil(),
			destination: PromotionDestination::Branch {
				name: "release".into(),
			},
		},
	};
	let result = ServerMessage::QueryResult {
		id: 13,
		result: QueryResponse::PromotionPreview(PromotionPreview {
			cursor: 3,
			binding: PromotionBinding {
				workspace_id: Uuid::nil(),
				destination: PromotionDestination::LocalCheckout,
				base_commit: "0".repeat(40),
				workspace_tree: "1".repeat(40),
				destination_commit: "2".repeat(40),
				destination_tree: "3".repeat(40),
				result_tree: "4".repeat(40),
				actor: Uuid::nil(),
			},
			destination_dirty: true,
			changed_paths: 2,
			changes: vec![
				PromotedChange {
					path: "src/lib.rs".into(),
					kind: ChangeKind::Modified,
				},
				PromotedChange {
					path: "docs/new.md".into(),
					kind: ChangeKind::Added,
				},
			],
			conflicts: vec![PromotionConflict {
				path: "src/lib.rs".into(),
				kind: ConflictKind::Diverged,
			}],
		}),
	};
	assert_eq!(
		(json(&query), json(&result)),
		(
			r#"{"kind":"query","id":13,"query":{"type":"preview_promotion","workspace_id":"00000000-0000-0000-0000-000000000000","destination":{"kind":"branch","name":"release"}}}"#.to_string(),
			format!(
				r#"{{"kind":"query_result","id":13,"result":{{"type":"promotion_preview","cursor":"3","binding":{{"workspace_id":"00000000-0000-0000-0000-000000000000","destination":{{"kind":"local_checkout"}},"base_commit":"{}","workspace_tree":"{}","destination_commit":"{}","destination_tree":"{}","result_tree":"{}","actor":"00000000-0000-0000-0000-000000000000"}},"destination_dirty":true,"changed_paths":2,"changes":[{{"path":"src/lib.rs","kind":"modified"}},{{"path":"docs/new.md","kind":"added"}}],"conflicts":[{{"path":"src/lib.rs","kind":"diverged"}}]}}}}"#,
				"0".repeat(40),
				"1".repeat(40),
				"2".repeat(40),
				"3".repeat(40),
				"4".repeat(40),
			),
		)
	);
}
