//! Wire form of journal Events. The `kind` and `payload` pair mirrors the
//! journal row (ADR-0096); clients ignore kinds they do not know
//! (ADR-0094).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Who caused an Event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Actor {
	/// An interactive GUI client.
	InteractiveClient {
		/// The client's durable identity.
		client_id: Uuid,
	},
}

/// One journal entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
	/// Plane-local monotonic position.
	pub sequence: u64,
	/// Durable identity.
	pub event_id: Uuid,
	/// Who caused the Event.
	pub actor: Actor,
	/// When it was recorded, in signed Unix milliseconds. Display metadata
	/// only; never an ordering authority.
	pub recorded_at_unix_ms: i64,
	/// The Conversation it concerns, if any.
	pub conversation_id: Option<Uuid>,
	/// The Run it concerns, if any.
	pub run_id: Option<Uuid>,
	/// Indexed kind name such as `run.lifecycle_changed`.
	pub kind: String,
	/// Kind-specific JSON payload.
	pub payload: serde_json::Value,
}
