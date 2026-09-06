//! Craft v1 Commands and native-event envelopes (ADR-0002, ADR-0052).

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;

use crate::PresentationBlock;

/// Explicit decision on exactly one native approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CraftApprovalDecision {
	/// Allow only the identified request under existing policy.
	AllowOnce,
	/// Deny the identified request.
	Deny,
}

/// Structured action input; unknown security-sensitive variants fail closed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CraftAction {
	/// Invoke an action offered by the Harness in this execution.
	Invoke {
		/// Native action identity.
		action_id: String,
		/// Harness-native input, interpreted only by the Craft.
		input: serde_json::Value,
	},
	/// Answer an approval after Jet has authorized this exact decision.
	Approval {
		/// Native approval request identity.
		request_id: String,
		/// No implicit blanket or persistent approval exists.
		decision: CraftApprovalDecision,
	},
}

/// Host-to-Craft Commands, admitted by Jet before crossing this boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CraftCommand {
	/// Submit one admitted turn.
	Turn {
		/// Correlation identity; delivery alone is not a durable receipt.
		id: String,
		/// Harness input.
		text: String,
	},
	/// Route an authenticated user action to its native handler.
	Action {
		/// Correlation identity.
		id: String,
		/// Structured native action.
		action: CraftAction,
	},
	/// Release this execution connection; never deletes a Conversation or
	/// terminates the Craft process while other execution connections exist.
	Shutdown,
}

/// Craft-to-host events. Native content remains the authoritative payload.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CraftEvent {
	/// Complete native event accompanied by optional portable views.
	Output {
		/// Original JSON bytes, including unknown data and numeric precision.
		native_event: Box<RawValue>,
		/// Presentation accompanies, never replaces, the native event.
		#[serde(default)]
		presentation: Vec<PresentationBlock>,
	},
	/// Native Command completion; the host persists it before acknowledging.
	Completed {
		/// Command correlation identity.
		id: String,
		/// Native identity retained for an explicitly resumed later execution.
		native_conversation: String,
	},
}

impl<'de> Deserialize<'de> for CraftEvent {
	fn deserialize<D: Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		// Internally tagged serde enums buffer through a value tree and lose
		// RawValue bytes. Dispatch on the tag, then parse the original bytes.
		let raw = Box::<RawValue>::deserialize(deserializer)?;
		#[derive(Deserialize)]
		struct Kind {
			kind: String,
		}
		let kind: Kind = crate::decode_control(raw.get().as_bytes())
			.map_err(serde::de::Error::custom)?;
		match kind.kind.as_str() {
			"output" => {
				#[derive(Deserialize)]
				struct Output {
					native_event: Box<RawValue>,
					#[serde(default)]
					presentation: Vec<PresentationBlock>,
				}
				let output: Output =
					crate::decode_control(raw.get().as_bytes())
						.map_err(serde::de::Error::custom)?;
				Ok(Self::Output {
					native_event: output.native_event,
					presentation: output.presentation,
				})
			}
			"completed" => {
				#[derive(Deserialize)]
				struct Completed {
					id: String,
					native_conversation: String,
				}
				let completed: Completed =
					crate::decode_control(raw.get().as_bytes())
						.map_err(serde::de::Error::custom)?;
				Ok(Self::Completed {
					id: completed.id,
					native_conversation: completed.native_conversation,
				})
			}
			_ => Err(serde::de::Error::custom("unknown Craft event kind")),
		}
	}
}
