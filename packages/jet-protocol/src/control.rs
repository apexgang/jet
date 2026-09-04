//! Strict JSON codec for control frames.
//!
//! Control payloads are parsed with a nesting bound that is enforced before
//! the JSON parser allocates any structure (ADR-0089).

use serde::{Serialize, de::DeserializeOwned};

/// Maximum nesting depth of arrays and objects in one control frame.
pub const MAX_NESTING_DEPTH: usize = 64;

/// Failure while encoding or decoding a control payload.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ControlError {
	/// The payload nests arrays or objects deeper than the protocol allows.
	#[error("control payload nests deeper than {limit} levels")]
	TooDeep {
		/// The enforced depth limit.
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
	if nesting_depth(bytes) > MAX_NESTING_DEPTH {
		return Err(ControlError::TooDeep {
			limit: MAX_NESTING_DEPTH,
		});
	}
	serde_json::from_slice(bytes)
		.map_err(|error| ControlError::Malformed(error.to_string()))
}

/// Deepest array/object nesting reached anywhere in the payload, ignoring
/// brackets inside string literals.
fn nesting_depth(bytes: &[u8]) -> usize {
	let mut depth = 0usize;
	let mut deepest = 0usize;
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
			b'"' => in_string = true,
			b'[' | b'{' => {
				depth += 1;
				deepest = deepest.max(depth);
			}
			b']' | b'}' => depth = depth.saturating_sub(1),
			_ => {}
		}
	}
	deepest
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
