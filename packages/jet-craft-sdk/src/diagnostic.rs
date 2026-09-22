//! Content-free stderr diagnostics beside stable Craft error codes.

use crate::CraftError;

impl CraftError {
	/// Report a rejection reason on local stderr without changing the wire code.
	/// The reason must be static so peer content cannot enter diagnostics.
	pub fn invalid_message(reason: &'static str) -> Self {
		eprintln!("jet-craft-sdk: {reason}");
		Self::InvalidMessage
	}

	/// Report the expected message type and a content-free parser diagnostic.
	/// Serde's native error can quote arbitrary peer values, so only a known
	/// category and numeric position survive. Shape errors contain only limits.
	pub fn invalid_control_message<T>(
		operation: &'static str,
		error: &jet_protocol::ControlError,
	) -> Self {
		let expected = std::any::type_name::<T>();
		match error {
			jet_protocol::ControlError::Malformed(detail) => {
				let reason = [
					"unknown variant",
					"unknown field",
					"missing field",
					"duplicate field",
					"invalid type",
					"invalid value",
				]
				.into_iter()
				.find(|prefix| detail.starts_with(prefix))
				.unwrap_or("invalid JSON or message schema");
				let position = detail
					.rsplit_once(" at line ")
					.and_then(|(_, position)| position.split_once(" column "))
					.and_then(|(line, column)| {
						Some((
							line.parse::<usize>().ok()?,
							column.parse::<usize>().ok()?,
						))
					});
				if let Some((line, column)) = position {
					eprintln!(
						"jet-craft-sdk: {operation} {expected}: malformed control payload: {reason} at line {line} column {column}"
					);
				} else {
					eprintln!(
						"jet-craft-sdk: {operation} {expected}: malformed control payload: {reason}"
					);
				}
			}
			jet_protocol::ControlError::Oversized { .. }
			| jet_protocol::ControlError::TooDeep { .. }
			| jet_protocol::ControlError::CollectionTooLarge { .. }
			| jet_protocol::ControlError::TooManyItems { .. } => {
				eprintln!("jet-craft-sdk: {operation} {expected}: {error}");
			}
		}
		Self::InvalidMessage
	}
}
