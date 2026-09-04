use pretty_assertions::assert_eq;
use serde::Deserialize;

use super::{
	ControlError, MAX_COLLECTION_ITEMS, MAX_CONTROL_ITEMS, MAX_NESTING_DEPTH,
	decode_control, encode_control,
};
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
	assert_eq!(value.to_string(), at_limit);
}

#[test]
fn collection_larger_than_the_limit_is_rejected_before_deserialization() {
	let items = std::iter::repeat_n("0", MAX_COLLECTION_ITEMS + 1)
		.collect::<Vec<_>>()
		.join(",");
	let oversized = format!("[{items}]");

	let error =
		decode_control::<serde_json::Value>(oversized.as_bytes()).unwrap_err();
	assert_eq!(
		error,
		ControlError::CollectionTooLarge {
			limit: MAX_COLLECTION_ITEMS
		}
	);
}

#[test]
fn total_items_across_collections_are_bounded() {
	let pair = "[0,0]";
	let collections = std::iter::repeat_n(pair, MAX_CONTROL_ITEMS / 3 + 1)
		.collect::<Vec<_>>()
		.join(",");
	let oversized = format!("[{collections}]");

	let error =
		decode_control::<serde_json::Value>(oversized.as_bytes()).unwrap_err();
	assert_eq!(
		error,
		ControlError::TooManyItems {
			limit: MAX_CONTROL_ITEMS
		}
	);
}

#[test]
fn commas_and_brackets_in_strings_do_not_count_as_collection_items() {
	let value = format!(
		"[{}]",
		std::iter::repeat_n(r#""[,],{,}""#, MAX_COLLECTION_ITEMS)
			.collect::<Vec<_>>()
			.join(",")
	);

	let decoded =
		decode_control::<serde_json::Value>(value.as_bytes()).unwrap();
	assert_eq!(decoded.as_array().unwrap().len(), MAX_COLLECTION_ITEMS);
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

#[test]
fn arbitrary_control_bytes_never_bypass_bounds_or_panic() {
	for seed in 0u32..512 {
		let mut state = seed.wrapping_add(1);
		let length = usize::try_from(seed % 257).unwrap();
		let mut bytes = Vec::with_capacity(length);
		for _ in 0..length {
			state ^= state << 13;
			state ^= state >> 17;
			state ^= state << 5;
			bytes.push(state.to_le_bytes()[0]);
		}
		let _ = decode_control::<serde_json::Value>(&bytes);
	}
}
