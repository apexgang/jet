//! Journal Events as the core sees them (ADR-0020, ADR-0096).

mod encoding;

mod kind;
pub use kind::EventKind;

pub(crate) mod query;

use crate::{
	Actor, ClientId,
	conversation::{ConversationId, RunId},
	error::CoreError,
};
use jet_store::EventRecord;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use uuid::Uuid;

/// Most Events one `Query::Events` page returns.
pub(crate) const EVENT_PAGE_LIMIT: usize = 256;

/// Durable identity of one Event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventId(pub Uuid);

/// A position in this Plane's journal (ADR-0069). As an Event's own
/// `sequence` it is total and monotonic; as a snapshot `cursor` it is the
/// newest position the snapshot saw, so a subscription resumes strictly
/// after it.
#[derive(
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
	Serialize,
	Deserialize,
)]
pub struct EventSequence(pub u64);

/// What an Event is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventSubject {
	/// The Plane itself, with no Conversation of its own.
	Plane,
	/// The Conversation as a whole.
	Conversation(ConversationId),
	/// One Run of a Conversation.
	Run {
		/// The Run's Conversation.
		conversation_id: ConversationId,
		/// The Run itself.
		run_id: RunId,
	},
}

/// Responsible origin of a journal Event. This grants no Command authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventActor {
	/// An enabled Auto-continue policy admitted or settled input.
	AutoContinue {
		/// Client that authorized the execution.
		authorized_by: ClientId,
	},
	/// A Scheduled task admitted or settled deterministic input.
	ScheduledTask {
		/// Responsible schedule.
		schedule_id: Uuid,
		/// Client that enabled it.
		authorized_by: ClientId,
	},
	/// An interactive client caused a committed change.
	InteractiveClient {
		/// Durable installation identity.
		client_id: ClientId,
	},
	/// Semantic output or attention from the pinned Harness.
	Harness {
		/// Execution that retains the accepted Craft identity and digest.
		run_id: RunId,
		/// Client that authorized the execution; not the origin of this Event.
		authorized_by: ClientId,
	},
	/// An internal process supervision observation.
	RunSupervisor {
		/// Execution being supervised.
		run_id: RunId,
		/// Client that authorized the execution.
		authorized_by: ClientId,
	},
}
impl EventActor {
	fn from_record(record: &EventRecord) -> Result<Self, CoreError> {
		let jet_store::ActorRecord::InteractiveClient { client_id } =
			record.actor;
		let authorized_by = ClientId(client_id);
		let payload: serde_json::Value = serde_json::from_str(&record.payload)
			.map_err(|e| {
				CoreError::internal("event.malformed", e.to_string())
			})?;
		if payload
			.get("_jet_auto_continue")
			.and_then(serde_json::Value::as_bool)
			== Some(true)
		{
			return Ok(Self::AutoContinue { authorized_by });
		}
		let Some(origin) = payload.get("_jet_origin") else {
			return Ok(Self::InteractiveClient {
				client_id: authorized_by,
			});
		};
		let invalid = || {
			CoreError::internal(
				"event.invalid_origin",
				"invalid Run attribution",
			)
		};
		if origin.get("type").and_then(serde_json::Value::as_str)
			== Some("scheduled_task")
		{
			let schedule_id = origin
				.get("schedule_id")
				.and_then(serde_json::Value::as_str)
				.ok_or_else(invalid)?
				.parse()
				.map_err(|_| invalid())?;
			return Ok(Self::ScheduledTask {
				schedule_id,
				authorized_by,
			});
		}
		let run_id: Uuid = origin
			.get("run_id")
			.and_then(serde_json::Value::as_str)
			.ok_or_else(invalid)?
			.parse()
			.map_err(|_| invalid())?;
		if Some(run_id) != record.run_id {
			return Err(invalid());
		}
		match origin.get("type").and_then(serde_json::Value::as_str) {
			Some("harness") => Ok(Self::Harness {
				run_id: RunId(run_id),
				authorized_by,
			}),
			Some("run_supervisor") => Ok(Self::RunSupervisor {
				run_id: RunId(run_id),
				authorized_by,
			}),
			_ => Err(invalid()),
		}
	}
	fn provenance(
		&self,
	) -> (jet_store::ActorRecord, Option<serde_json::Value>) {
		let (client_id, origin) = match self {
			Self::InteractiveClient { client_id } => (*client_id, None),
			Self::AutoContinue { authorized_by } => (*authorized_by, None),
			Self::ScheduledTask {
				schedule_id,
				authorized_by,
			} => (
				*authorized_by,
				Some(
					serde_json::json!({"type":"scheduled_task", "schedule_id": schedule_id}),
				),
			),
			Self::Harness {
				run_id,
				authorized_by,
			} => (
				*authorized_by,
				Some(serde_json::json!({"type":"harness","run_id":run_id.0})),
			),
			Self::RunSupervisor {
				run_id,
				authorized_by,
			} => (
				*authorized_by,
				Some(
					serde_json::json!({"type":"run_supervisor","run_id":run_id.0}),
				),
			),
		};
		(
			jet_store::ActorRecord::InteractiveClient {
				client_id: client_id.0,
			},
			origin,
		)
	}
}
impl From<Actor> for EventActor {
	fn from(actor: Actor) -> Self {
		Self::InteractiveClient {
			client_id: actor.client_id(),
		}
	}
}

/// One journal entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
	/// Plane-local position.
	pub sequence: EventSequence,
	/// Durable identity.
	pub event_id: EventId,
	/// Who caused the Event.
	pub actor: EventActor,
	/// When it was recorded; display metadata only.
	pub recorded_at: SystemTime,
	/// The Conversation it concerns, if any.
	pub conversation_id: Option<ConversationId>,
	/// The Run it concerns, if any.
	pub run_id: Option<RunId>,
	/// What happened.
	pub kind: EventKind,
}

/// One page of journal Events, fenced by the journal position it was read
/// at (ADR-0092). The page is the last one when its final Event's sequence
/// equals `cursor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPage {
	/// Newest Event sequence visible to this query when the page was read.
	pub cursor: EventSequence,
	/// The Events strictly after the requested position, in sequence order.
	pub events: Vec<Event>,
}

/// The journal form of an Event's content: an indexed kind name beside a
/// versioned JSON payload (ADR-0096). `jetd` forwards it without
/// interpretation; the wire schema of each kind is this JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPayload {
	/// Indexed kind name such as `run.lifecycle_changed`.
	pub kind: String,
	/// Schema version of `payload` for this `kind`.
	pub payload_version: u32,
	/// Kind-specific JSON payload.
	pub payload: serde_json::Value,
}
