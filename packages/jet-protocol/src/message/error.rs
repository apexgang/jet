//! Stable wire failures and actionable recovery metadata.

use crate::conversation::RevisionConflict;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable error categories exposed to clients (ADR-0068).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
	/// The request was malformed or violated a precondition.
	InvalidInput,
	/// The Actor may not perform the request.
	Unauthorized,
	/// The request conflicts with current state.
	Conflict,
	/// A required resource is temporarily unavailable.
	Unavailable,
	/// The peers cannot agree on a protocol, codec, or version.
	Incompatible,
	/// The request was throttled.
	RateLimited,
	/// The addressed resource does not exist.
	NotFound,
	/// The result of external work could not be established.
	OutcomeUnknown,
	/// An unexpected internal failure.
	Internal,
}

/// Stable error body carried by every failed reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireError {
	/// Category the client may branch on.
	pub category: ErrorCategory,
	/// Domain-specific code such as `protocol.unsupported_version`.
	pub code: String,
	/// Whether repeating the same request may succeed.
	pub retryable: bool,
	/// Safe human-readable description free of native error strings.
	pub message: String,
	/// Current resource state when an expected Revision was stale.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub revision_conflict: Option<RevisionConflict>,
	/// Structured metadata when a stale read must restart.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub restart: Option<RestartMetadata>,
	/// Structured actions that can safely recover from this error.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub recovery_actions: Vec<RecoveryAction>,
}

/// Structured action a client may take to recover from an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecoveryAction {
	/// Refresh a file before preparing another direct edit.
	RefreshFile {
		/// Registered root to refresh.
		target: crate::FileTarget,
		/// Validated path relative to that root.
		path: String,
		/// Exact file state now authoritative.
		current_revision: crate::FileRevision,
	},
	/// Refresh current Conversation state before preparing another Command.
	RefreshConversation {
		/// Conversation whose current state should be queried.
		conversation_id: Uuid,
	},
	/// Refresh current Run state before preparing another Command.
	RefreshRun {
		/// Run whose current state should be queried.
		run_id: Uuid,
	},
	/// Reconnect and resume the semantic Event stream after this cursor.
	ResumeEvents {
		/// Last Event cursor the disconnected client received completely.
		#[serde(with = "crate::transport::decimal")]
		#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
		after: u64,
	},
}

/// Stable metadata explaining why a snapshot must be restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum RestartMetadata {
	/// Required Event replay is no longer retained.
	CursorExpired {
		/// Oldest cursor from which continuous replay remains possible.
		#[serde(with = "crate::transport::decimal")]
		#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
		minimum_available_cursor: u64,
		/// Current Event high-water cursor for a replacement snapshot.
		#[serde(with = "crate::transport::decimal")]
		#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
		current_snapshot_revision: u64,
	},
	/// The supplied Event cursor belongs to a later or different Plane.
	CursorAhead {
		/// Current Event high-water cursor for the replacement snapshot.
		#[serde(with = "crate::transport::decimal")]
		#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
		current_snapshot_revision: u64,
	},
	/// A later page no longer belongs to the current projection state.
	PaginationStale {
		/// Current Event high-water cursor for the replacement first page.
		#[serde(with = "crate::transport::decimal")]
		#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
		current_snapshot_revision: u64,
	},
}
