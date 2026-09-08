//! Explicit, bounded operations on paired destination Workspaces (ADR-0028).
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Origin retained separately from the paired installation's authentication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct NoVisaOrigin {
	/// Home Plane, which continues to own the Conversation.
	pub plane_id: Uuid,
	/// Originating Conversation.
	pub conversation_id: Uuid,
	/// Originating Harness Run.
	pub run_id: Uuid,
}

/// One request from a trusted Home daemon over its paired connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RemoteToolRequest {
	/// Stable identity; an uncertain mutation must never get a new identity automatically.
	pub operation_id: Uuid,
	/// Actor provenance derived by the Home daemon from the accepted Run.
	pub origin: NoVisaOrigin,
	/// Expected destination, checked before accessing its state.
	pub destination_plane_id: Uuid,
	/// Registered destination Workspace.
	pub workspace_id: Uuid,
	/// Accepted Craft permissions, supplied by the Home daemon, never the Craft.
	pub permissions: Vec<crate::BrokerPermission>,
	/// Closed operation vocabulary; arbitrary Commands cannot be forwarded.
	pub action: RemoteToolAction,
}

/// Operations available through the remote broker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteToolAction {
	/// One reviewed input batch in a real PTY, closed after the batch exits.
	Terminal {
		/// Workspace-relative cwd; empty selects its root.
		directory: String,
		/// Exact terminal input, shown in destination review.
		input: String,
		/// Fixed terminal dimensions.
		rows: u16,
		/// Fixed terminal dimensions.
		columns: u16,
	},
	/// Argument-array process invocation, reviewed exactly once at destination.
	Process {
		/// Executable followed by literal arguments; never a shell command string.
		arguments: Vec<String>,
		/// Workspace-relative cwd; empty selects its root.
		directory: String,
		/// Explicit changes to a minimal environment.
		environment: Vec<RemoteEnvironment>,
	},
	/// Read-only Git inspection with fixed arguments and hooks disabled.
	Git {
		/// Closed inspection vocabulary.
		operation: RemoteGitOperation,
	},
	/// Visibly labelled shell syntax, requiring exact destination review.
	Shell {
		/// Workspace-relative directory; empty selects the Workspace root.
		directory: String,
		/// Explicit environment changes, applied to a minimal environment.
		environment: Vec<RemoteEnvironment>,
		/// Shell source, never interpolated into SSH or a process argument array.
		script: String,
	},
	/// Atomically replace at most 64 KiB inside the Workspace.
	WriteFile {
		/// Workspace-relative file path.
		path: String,
		/// New UTF-8 content.
		content: String,
	},
	/// Read at most 64 KiB of UTF-8 file content.
	ReadFile {
		/// Validated Workspace-relative path.
		path: String,
	},
}

/// Bounded response from one destination operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteToolResult {
	/// This action has not executed and needs a destination user decision.
	ApprovalRequired {
		/// Stable identity of the action to inspect and review.
		operation_id: uuid::Uuid,
	},
	/// Bounded process output after native exit.
	Process {
		/// Native exit status, absent when terminated by a signal.
		exit_code: Option<i32>,
		/// At most 64 KiB, rendered with replacement for invalid UTF-8.
		stdout: String,
		/// At most 64 KiB, rendered with replacement for invalid UTF-8.
		stderr: String,
	},
	/// The file replacement and directory sync completed.
	Written,
	/// UTF-8 content inside the selected Workspace.
	File {
		/// At most 64 KiB.
		content: String,
	},
}

/// Explicit destination environment change; absent values remove a variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RemoteEnvironment {
	/// Variable name, without equals signs or NULs.
	pub name: String,
	/// Explicit value or removal.
	pub value: Option<String>,
}

/// A user decision on one immutable remote action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RemoteToolDecision {
	/// Permit exactly this stored action once.
	AllowOnce,
	/// Refuse this action permanently.
	Deny,
}

/// A Craft can select only a pinned destination and action, never authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CraftRemoteTool {
	/// Stable identity retained across Craft source replay.
	pub operation_id: Uuid,
	/// Selected destination.
	pub destination_plane_id: Uuid,
	/// Selected registered Workspace.
	pub workspace_id: Uuid,
	/// Exact requested operation.
	pub action: RemoteToolAction,
}
/// A remote failure belongs to this call, not the origin Run or another Plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteToolOutcome {
	/// The destination returned a result, possibly awaiting exact review.
	Completed {
		/// Destination outcome.
		result: RemoteToolResult,
	},
	/// The request failed or has an unknown outcome; never retry mutations blindly.
	Failed {
		/// Stable, bounded error without transport secrets.
		error: crate::WireError,
	},
}

/// Read-only Git operations with bounded output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RemoteGitOperation {
	/// Porcelain v1 status, including untracked paths.
	Status,
	/// Working-tree diff without external diff or text conversion commands.
	Diff,
}
