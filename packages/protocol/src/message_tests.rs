use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{
	ClientMessage, ErrorCategory, PlaneStatus, QueryRequest, QueryResponse,
	ServerMessage, WireError,
};
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
	};
	assert_eq!(
		json(&hello),
		r#"{"protocol":{"min":1,"max":1},"codec":"json-v1","client_id":"00000000-0000-0000-0000-000000000000"}"#
	);
}

#[test]
fn server_hello_variants_have_the_agreed_wire_shape() {
	let welcome = ServerHello::Welcome {
		protocol: 1,
		codec: "json-v1".into(),
		max_control_frame: 1_048_576,
		max_data_frame: 262_144,
	};
	let rejected = ServerHello::Rejected {
		error: WireError {
			category: ErrorCategory::Incompatible,
			code: "protocol.unsupported_version".into(),
			retryable: false,
			message: "no common protocol version".into(),
		},
	};
	assert_eq!(
		(json(&welcome), json(&rejected)),
		(
			r#"{"kind":"welcome","protocol":1,"codec":"json-v1","max_control_frame":1048576,"max_data_frame":262144}"#.to_string(),
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
		},
	};
	assert_eq!(
		json(&error),
		r#"{"kind":"error","id":4,"error":{"category":"unavailable","code":"store.unavailable","retryable":true,"message":"the Plane store is unavailable"}}"#
	);
}
