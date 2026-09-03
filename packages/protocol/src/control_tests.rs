use pretty_assertions::assert_eq;
use serde::Deserialize;

use super::{ControlError, MAX_NESTING_DEPTH, decode_control, encode_control};
use crate::message::{ClientMessage, QueryRequest};

#[derive(Debug, PartialEq, Deserialize)]
struct Probe {
	name: String,
}

#[test]
fn nesting_deeper_than_the_limit_is_rejected() {
	let deep = format!(
		"{}{}",
		"[".repeat(MAX_NESTING_DEPTH + 1),
		"]".repeat(MAX_NESTING_DEPTH + 1)
	);

	let error =
		decode_control::<serde_json::Value>(deep.as_bytes()).unwrap_err();
	assert_eq!(
		error,
		ControlError::TooDeep {
			limit: MAX_NESTING_DEPTH
		}
	);
}

#[test]
fn nesting_at_the_limit_is_accepted_and_brackets_in_strings_are_ignored() {
	let at_limit = format!(
		"{}\"[[[{{{{\"{}",
		"[".repeat(MAX_NESTING_DEPTH),
		"]".repeat(MAX_NESTING_DEPTH)
	);

	let value =
		decode_control::<serde_json::Value>(at_limit.as_bytes()).unwrap();
	assert_eq!(value.to_string().len(), at_limit.len());
}

#[test]
fn unknown_message_kinds_are_rejected() {
	let error = decode_control::<ClientMessage>(
		br#"{"kind":"launch_missiles","id":1}"#,
	)
	.unwrap_err();
	assert!(
		matches!(error, ControlError::Malformed(_)),
		"unexpected error: {error:?}"
	);
}

#[test]
fn unknown_optional_fields_from_newer_minors_are_ignored() {
	let message = decode_control::<ClientMessage>(
		br#"{"kind":"query","id":7,"query":{"type":"status","verbose":true},"trace":"x"}"#,
	)
	.unwrap();
	assert_eq!(
		message,
		ClientMessage::Query {
			id: 7,
			query: QueryRequest::Status
		}
	);
}

#[test]
fn encoded_control_messages_decode_back_to_the_same_message() {
	let message = ClientMessage::Query {
		id: 3,
		query: QueryRequest::Status,
	};

	let bytes = encode_control(&message).unwrap();
	let decoded = decode_control::<ClientMessage>(&bytes).unwrap();

	assert_eq!(decoded, message);
}

#[test]
fn plain_objects_decode_into_typed_values() {
	assert_eq!(
		decode_control::<Probe>(br#"{"name":"jet"}"#).unwrap(),
		Probe { name: "jet".into() }
	);
}
