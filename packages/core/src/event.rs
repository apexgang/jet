//! Journal Events as the core sees them (ADR-0020, ADR-0096).

use std::time::SystemTime;

use jet_store::{EventRecord, NewEvent, Retention, RunLifecycle};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::conversation::{ConversationId, RunId};
use crate::error::{CoreError, ErrorCategory};
use crate::{Actor, system_time};

/// Most Events one `Query::Events` page returns.
pub const EVENT_PAGE_LIMIT: usize = 256;

/// Schema version of every payload this core writes.
const PAYLOAD_VERSION: u32 = 1;

/// Durable identity of one Event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventId(pub Uuid);

/// A position in this Plane's journal (ADR-0069). As an Event's own
/// `sequence` it is total and monotonic; as a snapshot `cursor` it is the
/// newest position the snapshot saw, so a subscription resumes strictly
/// after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventSequence(pub u64);

impl EventSequence {
	/// The position before the first Event.
	pub const ORIGIN: Self = Self(0);
}

/// One journal entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
	/// Plane-local position.
	pub sequence: EventSequence,
	/// Durable identity.
	pub event_id: EventId,
	/// Who caused the Event.
	pub actor: Actor,
	/// When it was recorded; display metadata only.
	pub recorded_at: SystemTime,
	/// The Conversation it concerns, if any.
	pub conversation_id: Option<ConversationId>,
	/// The Run it concerns, if any.
	pub run_id: Option<RunId>,
	/// What happened.
	pub kind: EventKind,
}

/// What an Event records. The serde form is the journal and wire form: an
/// indexed `kind` name beside a versioned JSON `payload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum EventKind {
	/// A Conversation came into existence.
	#[serde(rename = "conversation.created")]
	ConversationCreated {
		/// Its retention choice.
		retention: Retention,
	},
	/// A Run was recorded in the `created` state.
	#[serde(rename = "run.created")]
	RunCreated {},
	/// A Run moved to a later lifecycle state.
	#[serde(rename = "run.lifecycle_changed")]
	RunLifecycleChanged {
		/// The state it left.
		from: RunLifecycle,
		/// The state it entered.
		to: RunLifecycle,
	},
}

impl EventKind {
	/// Splits the kind into its indexed name and JSON payload.
	///
	/// # Errors
	///
	/// Returns an `internal` [`CoreError`] if the kind cannot be encoded,
	/// which indicates a programming error.
	pub fn encode(&self) -> Result<(String, serde_json::Value), CoreError> {
		let encoded = serde_json::to_value(self).map_err(|error| {
			internal("event.unencodable", error.to_string())
		})?;
		let serde_json::Value::Object(mut fields) = encoded else {
			return Err(internal("event.unencodable", "not an object".into()));
		};
		let Some(serde_json::Value::String(kind)) = fields.remove("kind")
		else {
			return Err(internal("event.unencodable", "no kind".into()));
		};
		let payload = fields.remove("payload").unwrap_or_else(|| {
			serde_json::Value::Object(serde_json::Map::new())
		});
		Ok((kind, payload))
	}

	fn decode(record: &EventRecord) -> Result<Self, CoreError> {
		if record.payload_version != PAYLOAD_VERSION {
			return Err(internal(
				"event.unsupported_payload",
				format!("payload version {}", record.payload_version),
			));
		}
		let payload: serde_json::Value = serde_json::from_str(&record.payload)
			.map_err(|error| internal("event.malformed", error.to_string()))?;
		serde_json::from_value(serde_json::json!({
			"kind": record.kind,
			"payload": payload,
		}))
		.map_err(|error| internal("event.malformed", error.to_string()))
	}

	pub(crate) fn to_record(
		&self,
		actor: &Actor,
		conversation_id: ConversationId,
		run_id: Option<RunId>,
	) -> Result<NewEvent, CoreError> {
		let (kind, payload) = self.encode()?;
		Ok(NewEvent {
			event_id: Uuid::now_v7(),
			actor: actor.record(),
			conversation_id: Some(conversation_id.0),
			run_id: run_id.map(|id| id.0),
			kind,
			payload_version: PAYLOAD_VERSION,
			payload: payload.to_string(),
		})
	}
}

impl TryFrom<EventRecord> for Event {
	type Error = CoreError;

	fn try_from(record: EventRecord) -> Result<Self, CoreError> {
		let kind = EventKind::decode(&record)?;
		Ok(Self {
			sequence: EventSequence(record.sequence),
			event_id: EventId(record.event_id),
			actor: Actor::from_record(record.actor),
			recorded_at: system_time(record.recorded_at_unix_ms),
			conversation_id: record.conversation_id.map(ConversationId),
			run_id: record.run_id.map(RunId),
			kind,
		})
	}
}

fn internal(code: &'static str, detail: String) -> CoreError {
	CoreError {
		category: ErrorCategory::Internal,
		code,
		retryable: false,
		message: "a journal Event could not be encoded or decoded".into(),
		detail: Some(detail),
	}
}
