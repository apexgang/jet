//! Stable core errors (ADR-0068).

use jet_store::StoreError;
use serde::{Deserialize, Serialize};

use crate::{EventSequence, Revision, Run, RunId};

/// Stable metadata explaining how a client must restart a stale read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestartMetadata {
	/// Required Event replay is no longer retained.
	CursorExpired {
		/// Oldest cursor from which continuous replay remains possible.
		minimum_available_cursor: EventSequence,
		/// Current Event high-water cursor for a replacement snapshot.
		current_snapshot_revision: EventSequence,
	},
	/// The supplied Event cursor belongs to a later or different Plane.
	CursorAhead {
		/// Current Event high-water cursor for the replacement snapshot.
		current_snapshot_revision: EventSequence,
	},
	/// A later page no longer belongs to the current projection state.
	PaginationStale {
		/// Current Event high-water cursor for the replacement first page.
		current_snapshot_revision: EventSequence,
	},
}

/// Structured action a caller may take to recover from an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryAction {
	/// Refresh the current state of a Run before preparing another Command.
	RefreshRun {
		/// Run whose current state should be queried.
		run_id: RunId,
	},
	/// Discard stale read state and obtain a fresh fenced snapshot.
	RestartSnapshot {
		/// Stable reason and cursor values needed to restart honestly.
		metadata: RestartMetadata,
	},
}

/// Structured current state returned for a stale Revision precondition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionConflict {
	/// Revision that is authoritative now.
	pub current_revision: Revision,
	/// Safe current state with which the caller can refresh.
	pub safe_state: ConflictState,
}

/// Safe resource state attached to a Revision conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictState {
	/// The current Run.
	Run(Run),
}

/// Stable error categories clients may branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// A core failure with a stable category, code, and safe message.
///
/// Native detail (SQLite, OS) is kept in `detail` for diagnostics only and
/// never reaches clients.
#[derive(
	Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error,
)]
#[error("{code}: {message}")]
pub struct CoreError {
	/// Category the caller may branch on.
	pub category: ErrorCategory,
	/// Domain code such as `store.unavailable`.
	pub code: String,
	/// Whether repeating the request may succeed.
	pub retryable: bool,
	/// Safe human-readable description.
	pub message: String,
	/// Redacted native detail for local diagnostics.
	pub detail: Option<String>,
	/// Current resource state when an expected Revision was stale.
	pub revision_conflict: Option<RevisionConflict>,
	/// Structured actions that can safely recover from this error.
	pub recovery_actions: Vec<RecoveryAction>,
}

impl CoreError {
	pub(crate) fn cursor_expired(
		minimum_available_cursor: EventSequence,
		current_snapshot_revision: EventSequence,
	) -> Self {
		Self::restart_read(
			"event.cursor_expired",
			"the Event cursor is older than retained replay",
			RestartMetadata::CursorExpired {
				minimum_available_cursor,
				current_snapshot_revision,
			},
		)
	}

	pub(crate) fn pagination_stale(
		current_snapshot_revision: EventSequence,
	) -> Self {
		Self::restart_read(
			"pagination.stale",
			"the paginated snapshot changed; restart from its first page",
			RestartMetadata::PaginationStale {
				current_snapshot_revision,
			},
		)
	}

	pub(crate) fn cursor_ahead(
		current_snapshot_revision: EventSequence,
	) -> Self {
		Self::restart_read(
			"event.cursor_ahead",
			"the Event cursor is ahead of this Plane; restart from a fresh snapshot",
			RestartMetadata::CursorAhead {
				current_snapshot_revision,
			},
		)
	}

	fn restart_read(
		code: &'static str,
		message: &'static str,
		metadata: RestartMetadata,
	) -> Self {
		Self {
			category: ErrorCategory::Conflict,
			code: code.into(),
			retryable: false,
			message: message.into(),
			detail: None,
			revision_conflict: None,
			recovery_actions: vec![RecoveryAction::RestartSnapshot {
				metadata,
			}],
		}
	}

	pub(crate) fn invalid_input(
		code: &'static str,
		message: impl Into<String>,
	) -> Self {
		Self {
			category: ErrorCategory::InvalidInput,
			code: code.into(),
			retryable: false,
			message: message.into(),
			detail: None,
			revision_conflict: None,
			recovery_actions: vec![],
		}
	}

	pub(crate) fn not_found(
		code: &'static str,
		message: impl Into<String>,
	) -> Self {
		Self {
			category: ErrorCategory::NotFound,
			code: code.into(),
			retryable: false,
			message: message.into(),
			detail: None,
			revision_conflict: None,
			recovery_actions: vec![],
		}
	}

	pub(crate) fn conflict(
		code: &'static str,
		message: impl Into<String>,
	) -> Self {
		Self {
			category: ErrorCategory::Conflict,
			code: code.into(),
			retryable: false,
			message: message.into(),
			detail: None,
			revision_conflict: None,
			recovery_actions: vec![],
		}
	}

	pub(crate) fn revision_conflict(
		code: &'static str,
		message: impl Into<String>,
		revision_conflict: RevisionConflict,
	) -> Self {
		let recovery_actions = match revision_conflict.safe_state {
			ConflictState::Run(run) => {
				vec![RecoveryAction::RefreshRun { run_id: run.run_id }]
			}
		};
		Self {
			category: ErrorCategory::Conflict,
			code: code.into(),
			retryable: false,
			message: message.into(),
			detail: None,
			revision_conflict: Some(revision_conflict),
			recovery_actions,
		}
	}

	/// A stable refusal because the peers' cores or protocols disagree.
	pub(crate) fn incompatible(
		code: &'static str,
		message: impl Into<String>,
	) -> Self {
		Self {
			category: ErrorCategory::Incompatible,
			code: code.into(),
			retryable: false,
			message: message.into(),
			detail: None,
			revision_conflict: None,
			recovery_actions: vec![],
		}
	}

	pub(crate) fn internal(
		code: &'static str,
		detail: impl Into<String>,
	) -> Self {
		Self {
			category: ErrorCategory::Internal,
			code: code.into(),
			retryable: false,
			message: "an internal invariant failed".into(),
			detail: Some(detail.into()),
			revision_conflict: None,
			recovery_actions: vec![],
		}
	}

	pub(crate) fn is_authoritative_result(&self) -> bool {
		self.detail.is_none()
	}
}

impl From<StoreError> for CoreError {
	fn from(error: StoreError) -> Self {
		match error {
			StoreError::CursorExpired {
				minimum_available_cursor,
				current_snapshot_revision,
			} => Self::cursor_expired(
				EventSequence(minimum_available_cursor),
				EventSequence(current_snapshot_revision),
			),
			StoreError::CursorAhead {
				current_snapshot_revision,
			} => Self::cursor_ahead(EventSequence(current_snapshot_revision)),
			StoreError::Unavailable(detail) => Self {
				category: ErrorCategory::Unavailable,
				code: "store.unavailable".into(),
				retryable: true,
				message: "the Plane store is unavailable".into(),
				detail: Some(detail),
				revision_conflict: None,
				recovery_actions: vec![],
			},
			StoreError::Integrity(detail) => Self {
				category: ErrorCategory::Internal,
				code: "store.integrity".into(),
				retryable: false,
				message: "the Plane store failed an integrity check".into(),
				detail: Some(detail),
				revision_conflict: None,
				recovery_actions: vec![],
			},
		}
	}
}
