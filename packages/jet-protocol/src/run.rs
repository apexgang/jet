//! Managed Run snapshots and orthogonal active activity (ADR-0065).
use crate::{Run, RunTermination};
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ManagedProcessRole {
	/// Generic per-Run supervisor.
	Helper,
	/// Native coding Harness.
	Harness,
}

/// Observable process identity; distinct from a Conversation or Run identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ManagedProcess {
	/// OS process identifier, meaningful only on the Home Plane.
	pub pid: u32,
	/// Process responsibility.
	pub role: ManagedProcessRole,
	/// Live native title for this process alone, never an entity name (1.20).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub label: Option<String>,
	/// Whether this process is still participating in the Run.
	pub running: bool,
}

/// Durable execution projection, fenced with the Event journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunExecution {
	/// The live execution requires an interactive recovery decision.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub needs_attention: bool,
	/// Explicit No-Visa execution selection, absent for Visa Runs.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub no_visa: Option<crate::NoVisaSelection>,
	/// Explicit Visa selection; absent on legacy Runs and before minor 25.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub visa: Option<crate::VisaSelection>,
	/// Snapshot cursor.
	#[serde(with = "crate::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
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
	/// How an interactive control request ended this execution. Absent
	/// before protocol minor 19, and while no request has settled.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub termination: Option<RunTermination>,
}
