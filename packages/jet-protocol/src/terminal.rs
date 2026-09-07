//! Workspace terminal control. Input and output use raw numbered data frames.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Durable terminal lifecycle, independent from Runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
	/// The durable launch Effect is pending.
	Opening,
	/// A live helper owns the PTY.
	Open,
	/// An explicit close is pending.
	Closing,
	/// The PTY ended; reconnect never launches another shell.
	Closed,
	/// Identity or transport could not be established safely.
	Unavailable,
}

/// One Workspace-owned PTY; no Run identity is involved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTerminal {
	/// Stable terminal identity.
	pub terminal_id: Uuid,
	/// Owning Workspace.
	pub workspace_id: Uuid,
	/// Observed lifecycle.
	pub state: TerminalState,
}

/// Immutable host-written launch boundary for a terminal-role helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalConfig {
	/// Terminal identity.
	pub terminal_id: Uuid,
	/// Workspace identity.
	pub workspace_id: Uuid,
	/// Canonical registered Workspace root.
	pub root: String,
	/// Registered Project root at admission.
	pub project_root: String,
	/// OS boot that accepted this launch.
	pub boot: String,
	/// Server-chosen shell executable.
	pub shell: String,
	/// Initial height.
	pub rows: u16,
	/// Initial width.
	pub columns: u16,
}

/// Atomic, owner-only terminal helper identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalDescriptor {
	/// Private helper protocol version, checked independently of product versions.
	pub protocol: crate::ProtocolVersion,
	/// Execution role, always `terminal`.
	pub role: String,
	/// Immutable launch boundary.
	pub config: TerminalConfig,
	/// Fresh process instance.
	pub instance: Uuid,
	/// OS process identity.
	pub pid: u32,
	/// Kernel-observed start identity.
	pub process_start: String,
	/// Accepted helper digest.
	pub sha256: String,
	/// Deployed product version.
	pub version: String,
}

/// One instance-bound operation on the private terminal helper protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalHelperRequest {
	/// Selected terminal helper protocol.
	pub protocol: crate::ProtocolVersion,
	/// Expected helper instance.
	pub instance: Uuid,
	/// Operation to perform.
	pub action: TerminalHelperAction,
}

/// Private terminal helper operations. Never interpreted by a Craft.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalHelperAction {
	/// Read a bounded range without changing another client's cursor.
	Read {
		/// Next requested byte.
		after: u64,
		/// Maximum returned bytes.
		limit: u32,
	},
	/// Write the following raw data frame to the PTY.
	Input,
	/// Change the PTY dimensions.
	Resize {
		/// Height in cells.
		rows: u16,
		/// Width in cells.
		columns: u16,
	},
	/// Close and reap the PTY process.
	Close,
}

/// Reply header followed by a raw data frame when `length` is nonzero.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalHelperReply {
	/// First returned byte; a larger value than requested explicitly identifies a gap.
	pub offset: u64,
	/// End of produced output.
	pub produced: u64,
	/// Raw payload length.
	pub length: u32,
	/// PTY output has ended.
	pub closed: bool,
}
