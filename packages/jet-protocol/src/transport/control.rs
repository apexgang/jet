//! Strict JSON codec for control frames.
//!
//! Control payloads are parsed with a nesting bound that is enforced before
//! the JSON parser allocates any structure (ADR-0089).

use crate::transport::frame::MAX_CONTROL_FRAME;
use serde::{Serialize, de::DeserializeOwned};

/// Maximum nesting depth of arrays and objects in one control frame.
pub const MAX_NESTING_DEPTH: usize = 64;
/// Maximum number of direct entries in one JSON array or object.
pub const MAX_COLLECTION_ITEMS: usize = 4_096;
/// Maximum number of entries across all arrays and objects in one frame.
pub const MAX_CONTROL_ITEMS: usize = 8_192;

/// Failure while encoding or decoding a control payload.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ControlError {
	/// The payload is larger than a control frame may carry.
	#[error(
		"control payload of {declared} bytes exceeds the {limit} byte limit"
	)]
	Oversized {
		/// Payload length presented to the decoder.
		declared: usize,
		/// Enforced control-frame limit.
		limit: usize,
	},
	/// The payload nests arrays or objects deeper than the protocol allows.
	#[error("control payload nests deeper than {limit} levels")]
	TooDeep {
		/// The enforced depth limit.
		limit: usize,
	},
	/// One array or object contains too many direct entries.
	#[error("control collection contains more than {limit} entries")]
	CollectionTooLarge {
		/// Enforced per-collection item limit.
		limit: usize,
	},
	/// The payload contains too many collection entries in total.
	#[error("control payload contains more than {limit} collection entries")]
	TooManyItems {
		/// Enforced whole-payload item limit.
		limit: usize,
	},
	/// The payload is not a valid message of the expected type.
	#[error("malformed control payload: {0}")]
	Malformed(String),
}

/// Serializes a control message to its JSON bytes.
///
/// # Errors
///
/// Returns [`ControlError::Malformed`] if the value cannot be represented as
/// JSON, which indicates a programming error in the message types.
pub fn encode_control<T: Serialize>(
	value: &T,
) -> Result<Vec<u8>, ControlError> {
	serde_json::to_vec(value)
		.map_err(|error| ControlError::Malformed(error.to_string()))
}

/// Parses a control payload after checking its nesting depth.
///
/// # Errors
///
/// Returns [`ControlError::TooDeep`] before parsing when the payload exceeds
/// [`MAX_NESTING_DEPTH`], or [`ControlError::Malformed`] when the JSON does
/// not describe a `T`, including unknown message kinds (ADR-0094).
pub fn decode_control<T: DeserializeOwned>(
	bytes: &[u8],
) -> Result<T, ControlError> {
	// ASVS 1.4.2, 2.2.1, 2.2.2, and 15.2.2: validate byte,
	// nesting, and collection limits at the trusted transport boundary
	// before deserialization can allocate an attacker-shaped value tree.
	if bytes.len() > MAX_CONTROL_FRAME {
		return Err(ControlError::Oversized {
			declared: bytes.len(),
			limit: MAX_CONTROL_FRAME,
		});
	}
	validate_shape(bytes)?;
	serde_json::from_slice(bytes)
		.map_err(|error| ControlError::Malformed(error.to_string()))
}

#[derive(Debug, Default)]
struct Collection {
	items: usize,
}

fn validate_shape(bytes: &[u8]) -> Result<(), ControlError> {
	let mut collections = Vec::<Collection>::with_capacity(MAX_NESTING_DEPTH);
	let mut total_items = 0usize;
	let mut in_string = false;
	let mut escaped = false;
	for &byte in bytes {
		if in_string {
			match byte {
				_ if escaped => escaped = false,
				b'\\' => escaped = true,
				b'"' => in_string = false,
				_ => {}
			}
			continue;
		}
		match byte {
			b'"' => {
				if let Some(collection) = collections.last_mut()
					&& collection.items == 0
				{
					increment_items(collection, &mut total_items)?;
				}
				in_string = true;
			}
			b'[' | b'{' => {
				if let Some(parent) = collections.last_mut()
					&& parent.items == 0
				{
					increment_items(parent, &mut total_items)?;
				}
				if collections.len() == MAX_NESTING_DEPTH {
					return Err(ControlError::TooDeep {
						limit: MAX_NESTING_DEPTH,
					});
				}
				collections.push(Collection::default());
			}
			b',' => {
				if let Some(collection) = collections.last_mut() {
					increment_items(collection, &mut total_items)?;
				}
			}
			b']' | b'}' => {
				collections.pop();
			}
			byte if byte.is_ascii_whitespace() => {}
			_ => {
				if let Some(collection) = collections.last_mut()
					&& collection.items == 0
				{
					increment_items(collection, &mut total_items)?;
				}
			}
		}
	}
	Ok(())
}

fn increment_items(
	collection: &mut Collection,
	total_items: &mut usize,
) -> Result<(), ControlError> {
	collection.items += 1;
	*total_items += 1;
	if collection.items > MAX_COLLECTION_ITEMS {
		return Err(ControlError::CollectionTooLarge {
			limit: MAX_COLLECTION_ITEMS,
		});
	}
	if *total_items > MAX_CONTROL_ITEMS {
		return Err(ControlError::TooManyItems {
			limit: MAX_CONTROL_ITEMS,
		});
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use pretty_assertions::assert_eq;
	use serde::Deserialize;

	use super::{
		ControlError, MAX_COLLECTION_ITEMS, MAX_CONTROL_ITEMS,
		MAX_NESTING_DEPTH, decode_control, encode_control,
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

		let error = decode_control::<serde_json::Value>(oversized.as_bytes())
			.unwrap_err();
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

		let error = decode_control::<serde_json::Value>(oversized.as_bytes())
			.unwrap_err();
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
				query: QueryRequest::Status,
				timeout_ms: None,
			}
		);
	}

	#[test]
	fn encoded_control_messages_decode_back_to_the_same_message() {
		let message = ClientMessage::Query {
			id: 3,
			query: QueryRequest::Status,
			timeout_ms: None,
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
}
