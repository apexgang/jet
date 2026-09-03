//! Control messages exchanged after the handshake.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Correlates a client request with its server reply.
pub type RequestId = u64;

/// Control message sent by a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientMessage {
	/// Run a Query and return its snapshot.
	Query {
		/// Correlation identifier echoed in the reply.
		id: RequestId,
		/// The Query to run.
		query: QueryRequest,
	},
}

/// Control message sent by `jetd`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerMessage {
	/// Successful Query snapshot.
	QueryResult {
		/// Identifier of the request being answered.
		id: RequestId,
		/// The snapshot.
		result: QueryResponse,
	},
	/// A request failed, or the connection is being refused.
	Error {
		/// Identifier of the request that failed, if any.
		id: Option<RequestId>,
		/// Stable error body.
		error: WireError,
	},
}

/// Queries a client may run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueryRequest {
	/// Snapshot of the Plane's daemon status.
	Status,
}

/// Query snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueryResponse {
	/// Snapshot of the Plane's daemon status.
	Status(PlaneStatus),
}

/// Wire form of the Plane status snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaneStatus {
	/// Durable identity of the Plane, created when its store was created.
	pub plane_id: Uuid,
	/// How many times an authoritative `jetd` has started on this Plane.
	pub daemon_starts: u64,
	/// When the current `jetd` started, in signed Unix milliseconds.
	pub started_at_unix_ms: i64,
	/// Version of the running core.
	pub core_version: String,
}

/// Stable error categories exposed to clients (ADR-0068).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct WireError {
	/// Category the client may branch on.
	pub category: ErrorCategory,
	/// Domain-specific code such as `protocol.unsupported_version`.
	pub code: String,
	/// Whether repeating the same request may succeed.
	pub retryable: bool,
	/// Safe human-readable description free of native error strings.
	pub message: String,
}

#[cfg(test)]
#[path = "message_tests.rs"]
mod tests;
