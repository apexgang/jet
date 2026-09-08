//! Closed No-Visa operation vocabulary, independent of wire versions.
use crate::{ConversationId, PlaneId, RunId, WorkspaceId};
use serde::{Deserialize, Serialize};

/// Server-derived origin carried by an authenticated Home Plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoVisaOrigin {
	/// Conversation's Home Plane.
	pub plane_id: PlaneId,
	/// Originating Conversation.
	pub conversation_id: ConversationId,
	/// Originating Harness execution.
	pub run_id: RunId,
}

/// A paired Home daemon's bounded request, independent of wire versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteToolRequest {
	/// Stable operation identity.
	pub operation_id: uuid::Uuid,
	/// Authenticated Home daemon's asserted originating Actor.
	pub origin: NoVisaOrigin,
	/// Expected local Plane.
	pub destination_plane_id: PlaneId,
	/// Registered destination Workspace.
	pub workspace_id: WorkspaceId,
	/// Accepted declarations forwarded by the Home daemon.
	pub permissions: Vec<crate::BrokerPermission>,
	/// Bounded operation.
	pub action: RemoteToolAction,
}

/// Closed remote operation vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
	/// Read bounded UTF-8 content within a Workspace.
	ReadFile {
		/// Workspace-relative path, validated before use.
		path: String,
	},
}

/// Bounded operation outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
	/// At most 64 KiB of UTF-8 content.
	File {
		/// File content.
		content: String,
	},
}

/// Explicit destination environment change; absent values remove a variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteEnvironment {
	/// Variable name, without equals signs or NULs.
	pub name: String,
	/// Explicit value or removal.
	pub value: Option<String>,
}

/// A user decision on one immutable remote action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteToolDecision {
	/// Permit exactly this stored action once.
	AllowOnce,
	/// Refuse this action permanently.
	Deny,
}

/// Read-only Git operations with bounded output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteGitOperation {
	/// Porcelain v1 status, including untracked paths.
	Status,
	/// Working-tree diff without external diff or text conversion commands.
	Diff,
}
