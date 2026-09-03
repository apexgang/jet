//! Stable core errors (ADR-0068).

use jet_store::StoreError;

/// Stable error categories clients may branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct CoreError {
	/// Category the caller may branch on.
	pub category: ErrorCategory,
	/// Domain code such as `store.unavailable`.
	pub code: &'static str,
	/// Whether repeating the request may succeed.
	pub retryable: bool,
	/// Safe human-readable description.
	pub message: String,
	/// Redacted native detail for local diagnostics.
	pub detail: Option<String>,
}

impl From<StoreError> for CoreError {
	fn from(error: StoreError) -> Self {
		match error {
			StoreError::Unavailable(detail) => Self {
				category: ErrorCategory::Unavailable,
				code: "store.unavailable",
				retryable: true,
				message: "the Plane store is unavailable".into(),
				detail: Some(detail),
			},
			StoreError::Integrity(detail) => Self {
				category: ErrorCategory::Internal,
				code: "store.integrity",
				retryable: false,
				message: "the Plane store failed an integrity check".into(),
				detail: Some(detail),
			},
		}
	}
}
