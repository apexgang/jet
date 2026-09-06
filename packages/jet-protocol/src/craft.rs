//! Craft v1 Commands and native-event envelopes (ADR-0002, ADR-0052).

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;

use crate::PresentationBlock;

/// Explicit decision on exactly one native approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CraftApprovalDecision {
	/// Allow only the identified request under existing policy.
	AllowOnce,
	/// Deny the identified request.
	Deny,
}

/// Structured action input; unknown security-sensitive variants fail closed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CraftCommand {
	/// Start a Run through its host-provisioned helper (Craft 1.1, runs).
	Start {
		/// Initial Command identity.
		id: String,
		/// Initial input for the Harness.
		text: String,
		/// Owner-only endpoint of this Run's helper.
		helper_socket: String,
	},
	/// Confirm that preceding semantic output is durable (Craft 1.1, runs).
	Acknowledge {
		/// Source record the Craft may now release through the helper.
		source_offset: u64,
	},
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CraftEvent {
	/// The helper definitively failed to launch a native Harness.
	RunLaunchFailed,
	/// Native process identities supplied by the Run-role helper.
	RunStarted {
		/// Helper OS process identity.
		helper_pid: u32,
		/// Harness OS process identity.
		harness_pid: u32,
	},
	/// Current reason an active Run is working or waiting.
	Activity {
		/// Orthogonal to the Run lifecycle.
		activity: crate::RunActivity,
	},
	/// End of a native source record; earlier Events must commit first.
	Progress {
		/// End offset acknowledged through the Craft only after durable commit.
		source_offset: u64,
	},
	/// The Harness ended, independently from a single turn's completion.
	RunEnded {
		/// Native exit code, absent for a signal termination.
		exit_code: Option<i32>,
	},
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
			"run_launch_failed" | "run_started" | "activity" | "progress"
			| "run_ended" => {
				#[derive(Deserialize)]
				#[serde(tag = "kind", rename_all = "snake_case")]
				enum State {
					RunLaunchFailed,
					RunStarted { helper_pid: u32, harness_pid: u32 },
					Activity { activity: crate::RunActivity },
					Progress { source_offset: u64 },
					RunEnded { exit_code: Option<i32> },
				}
				let state: State = crate::decode_control(raw.get().as_bytes())
					.map_err(serde::de::Error::custom)?;
				Ok(match state {
					State::RunLaunchFailed => Self::RunLaunchFailed,
					State::RunStarted {
						helper_pid,
						harness_pid,
					} => Self::RunStarted {
						helper_pid,
						harness_pid,
					},
					State::Activity { activity } => Self::Activity { activity },
					State::Progress { source_offset } => {
						Self::Progress { source_offset }
					}
					State::RunEnded { exit_code } => {
						Self::RunEnded { exit_code }
					}
				})
			}
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
