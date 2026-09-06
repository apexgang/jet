//! Wire form of journal Events. The `kind` and `payload` pair mirrors the
//! journal row (ADR-0096); clients retain kinds they do not know opaquely
//! and render them generically (ADR-0094).

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

/// Responsible execution origin, when legacy client authorization is insufficient.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventOrigin {
	/// Semantic observations from the pinned Harness.
	Harness {
		/// Owning Run identity.
		run_id: Uuid,
	},
	/// Internal process supervision.
	RunSupervisor {
		/// Owning Run identity.
		run_id: Uuid,
	},
}

/// One journal entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
	/// Plane-local monotonic position, carried as a decimal string
	/// (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub sequence: u64,
	/// Durable identity.
	pub event_id: Uuid,
	/// Legacy client authorization attribution. Use `origin` when present
	/// to identify who supplied an execution observation.
	pub actor: Actor,
	/// Actual origin of an execution observation; additive for older readers.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub origin: Option<EventOrigin>,
	/// When it was recorded, in signed Unix milliseconds. Display metadata
	/// only; never an ordering authority.
	pub recorded_at_unix_ms: i64,
	/// The Conversation it concerns, if any.
	pub conversation_id: Option<Uuid>,
	/// The Run it concerns, if any.
	pub run_id: Option<Uuid>,
	/// Indexed kind name such as `run.lifecycle_changed`.
	pub kind: String,
	/// Schema version of `payload` for this `kind` (ADR-0096).
	pub payload_version: u32,
	/// Kind-specific JSON payload.
	pub payload: serde_json::Value,
}
