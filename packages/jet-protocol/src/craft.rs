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
	/// Pinned remote destinations and honest capability report (Craft 1.6).
	ConfigureRemoteTools {
		/// Origin and authorized destinations.
		selection: crate::NoVisaSelection,
	},
	/// Reply to one remote call, without disrupting other destination work (1.6).
	RemoteToolResult {
		/// Original operation identity.
		operation_id: uuid::Uuid,
		/// Destination result or stable refusal.
		outcome: crate::RemoteToolOutcome,
	},
	/// Reconnect an existing helper without issuing native input (Craft 1.2).
	Recover {
		/// Original Run identity.
		id: String,
		/// Validated owner-only helper endpoint.
		helper_socket: String,
		/// Last atomically committed source boundary.
		source_offset: u64,
		/// Opaque parser state committed with that boundary.
		checkpoint: String,
	},
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
	/// Ask the Harness to cancel the identified turn without ending the
	/// Run (Craft 1.4). A Craft that negotiated 1.4 must answer every
	/// Interrupt with a `TurnEnded` boundary for that turn.
	Interrupt {
		/// Correlation identity of the turn to cancel.
		id: String,
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
	/// Request a Jet remote tool; the host supplies Actor and permissions (1.6).
	RemoteTool {
		/// Closed, destination-scoped request.
		call: crate::CraftRemoteTool,
	},
	/// Capture before a subsequent turn (1.3). Hold native input until the
	/// host acknowledges this marker's source record. Initial Start is pre-captured.
	TurnStarted,
	/// Capture completed or interrupted work without ending the Run (1.3).
	/// Do not begin another turn until this boundary is acknowledged.
	TurnEnded {
		/// Turn outcome.
		outcome: crate::TurnOutcome,
	},
	/// Correlated native file operation and exact content evidence (1.3).
	FileChanged {
		/// Origin is assigned by the host, never the Craft.
		change: crate::CraftFileChange,
	},
	/// Structured native title candidate for the owning Conversation (1.5).
	ConversationTitle {
		/// Unescaped title text.
		title: String,
	},
	/// Structured native title candidate for the current Run (1.5).
	RunTitle {
		/// Unescaped title text.
		title: String,
	},
	/// Terminal/native title for exactly one Managed process (1.5).
	ProcessTitle {
		/// OS process identity reported by `run_started`.
		pid: u32,
		/// Unescaped live label.
		title: String,
	},
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
	/// End of a native source record; earlier Events and parser state commit atomically.
	/// The host commits bounded groups with replay-prefix digests while keeping
	/// this record retained, then acknowledges after its final checkpoint.
	Progress {
		/// End offset acknowledged through the Craft only after durable commit.
		source_offset: u64,
		/// Bounded parser state required to interpret the next source record.
		#[serde(default)]
		checkpoint: String,
	},
	/// Confirmed attachment to the retained helper at the requested source boundary (1.2).
	RunRecovered {
		/// OS identity of the validated helper.
		helper_pid: u32,
		/// Requested durable source boundary.
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
			"remote_tool" => {
				#[derive(Deserialize)]
				#[serde(deny_unknown_fields)]
				struct Request {
					kind: String,
					call: crate::CraftRemoteTool,
				}
				let request: Request =
					crate::decode_control(raw.get().as_bytes())
						.map_err(serde::de::Error::custom)?;
				let _ = request.kind;
				Ok(Self::RemoteTool { call: request.call })
			}
			"conversation_title" | "run_title" | "process_title" => {
				#[derive(Deserialize)]
				#[serde(tag = "kind")]
				enum Title {
					#[serde(rename = "conversation_title")]
					Conversation { title: String },
					#[serde(rename = "run_title")]
					Run { title: String },
					#[serde(rename = "process_title")]
					Process { pid: u32, title: String },
				}
				let title: Title = crate::decode_control(raw.get().as_bytes())
					.map_err(serde::de::Error::custom)?;
				Ok(match title {
					Title::Conversation { title } => {
						Self::ConversationTitle { title }
					}
					Title::Run { title } => Self::RunTitle { title },
					Title::Process { pid, title } => {
						Self::ProcessTitle { pid, title }
					}
				})
			}
			"turn_started" | "turn_ended" | "file_changed" => {
				#[derive(Deserialize)]
				#[serde(tag = "kind", rename_all = "snake_case")]
				enum Change {
					TurnStarted,
					TurnEnded { outcome: crate::TurnOutcome },
					FileChanged { change: crate::CraftFileChange },
				}
				let change: Change =
					crate::decode_control(raw.get().as_bytes())
						.map_err(serde::de::Error::custom)?;
				Ok(match change {
					Change::TurnStarted => Self::TurnStarted,
					Change::TurnEnded { outcome } => {
						Self::TurnEnded { outcome }
					}
					Change::FileChanged { change } => {
						Self::FileChanged { change }
					}
				})
			}
			"run_launch_failed" | "run_started" | "activity" | "progress"
			| "run_ended" | "run_recovered" => {
				#[derive(Deserialize)]
				#[serde(tag = "kind", rename_all = "snake_case")]
				enum State {
					RunLaunchFailed,
					RunStarted {
						helper_pid: u32,
						harness_pid: u32,
					},
					Activity {
						activity: crate::RunActivity,
					},
					Progress {
						source_offset: u64,
						#[serde(default)]
						checkpoint: String,
					},
					RunRecovered {
						helper_pid: u32,
						source_offset: u64,
					},
					RunEnded {
						exit_code: Option<i32>,
					},
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
					State::Progress {
						source_offset,
						checkpoint,
					} => Self::Progress {
						source_offset,
						checkpoint,
					},
					State::RunRecovered {
						helper_pid,
						source_offset,
					} => Self::RunRecovered {
						helper_pid,
						source_offset,
					},
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
