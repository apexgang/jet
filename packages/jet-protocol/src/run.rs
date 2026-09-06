//! Managed Run snapshots and orthogonal active activity (ADR-0065).
use crate::Run;
use serde::{Deserialize, Serialize};

/// Why an active Run is working or waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RunActivity {
	/// Performing work.
	Working,
	/// Needs user input.
	WaitingForUser,
	/// Needs an approval decision.
	WaitingForApproval,
	/// Needs authentication.
	WaitingForAuth,
	/// Needs available quota.
	WaitingForQuota,
	/// Reconnecting to its execution.
	Reconnecting,
}

/// The role of a process owned by a Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedProcessRole {
	/// Generic per-Run supervisor.
	Helper,
	/// Native coding Harness.
	Harness,
}

/// Observable process identity; distinct from a Conversation or Run identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProcess {
	/// OS process identifier, meaningful only on the Home Plane.
	pub pid: u32,
	/// Process responsibility.
	pub role: ManagedProcessRole,
	/// Whether this process is still participating in the Run.
	pub running: bool,
}

/// Durable execution projection, fenced with the Event journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunExecution {
	/// Snapshot cursor.
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// Authoritative lifecycle and revision.
	pub run: Run,
	/// Present only while the Run is active.
	pub activity: Option<RunActivity>,
	/// Processes retained as historical identities after completion.
	pub processes: Vec<ManagedProcess>,
	/// Last native Conversation identity reported by its Craft.
	pub native_conversation: Option<String>,
	/// Native exit status when the OS supplied one.
	pub exit_code: Option<i32>,
}
