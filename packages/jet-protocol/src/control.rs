//! Strict JSON codec for control frames.
//!
//! Control payloads are parsed with a nesting bound that is enforced before
//! the JSON parser allocates any structure (ADR-0089).

use serde::{Serialize, de::DeserializeOwned};

use crate::frame::MAX_CONTROL_FRAME;

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
#[path = "control_tests.rs"]
mod tests;
