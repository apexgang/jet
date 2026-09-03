//! Stable core errors (ADR-0068).

use jet_store::StoreError;
use serde::{Deserialize, Serialize};

use crate::{Revision, Run};

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
}

impl CoreError {
	pub(crate) fn invalid_input(code: &'static str, message: &str) -> Self {
		Self {
			category: ErrorCategory::InvalidInput,
			code: code.into(),
			retryable: false,
			message: message.into(),
			detail: None,
			revision_conflict: None,
		}
	}

	pub(crate) fn not_found(code: &'static str, message: &str) -> Self {
		Self {
			category: ErrorCategory::NotFound,
			code: code.into(),
			retryable: false,
			message: message.into(),
			detail: None,
			revision_conflict: None,
		}
	}

	pub(crate) fn conflict(code: &'static str, message: String) -> Self {
		Self {
			category: ErrorCategory::Conflict,
			code: code.into(),
			retryable: false,
			message,
			detail: None,
			revision_conflict: None,
		}
	}

	pub(crate) fn revision_conflict(
		code: &'static str,
		message: &str,
		revision_conflict: RevisionConflict,
	) -> Self {
		Self {
			category: ErrorCategory::Conflict,
			code: code.into(),
			retryable: false,
			message: message.into(),
			detail: None,
			revision_conflict: Some(revision_conflict),
		}
	}

	pub(crate) fn internal(code: &'static str, detail: String) -> Self {
		Self {
			category: ErrorCategory::Internal,
			code: code.into(),
			retryable: false,
			message: "an internal invariant failed".into(),
			detail: Some(detail),
			revision_conflict: None,
		}
	}

	pub(crate) fn is_authoritative_result(&self) -> bool {
		self.detail.is_none()
	}
}

impl From<StoreError> for CoreError {
	fn from(error: StoreError) -> Self {
		match error {
			StoreError::Unavailable(detail) => Self {
				category: ErrorCategory::Unavailable,
				code: "store.unavailable".into(),
				retryable: true,
				message: "the Plane store is unavailable".into(),
				detail: Some(detail),
				revision_conflict: None,
			},
			StoreError::Integrity(detail) => Self {
				category: ErrorCategory::Internal,
				code: "store.integrity".into(),
				retryable: false,
				message: "the Plane store failed an integrity check".into(),
				detail: Some(detail),
				revision_conflict: None,
			},
		}
	}
}
