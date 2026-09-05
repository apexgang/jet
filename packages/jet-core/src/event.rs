//! Journal Events as the core sees them (ADR-0020, ADR-0096).

use std::time::SystemTime;

use jet_store::{
	EventClass, EventRecord, NewEvent, RetentionPolicy, RunLifecycle,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::account::{AccountBindingId, CredentialSource, ProviderId};
use crate::audit::AuditEpoch;
use crate::conversation::{ConversationId, RunId};
use crate::error::CoreError;
use crate::setting::{SettingKey, SettingScope, SettingValue};
use crate::{Actor, system_time};

/// Most Events one `Query::Events` page returns.
pub(crate) const EVENT_PAGE_LIMIT: usize = 256;

/// Schema version of every payload this core writes.
const PAYLOAD_VERSION: u32 = 1;

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

/// One page of journal Events, fenced by the journal position it was read
/// at (ADR-0092). The page is the last one when its final Event's sequence
/// equals `cursor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPage {
	/// Newest Event sequence in the journal when the page was read.
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

/// What an Event records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum EventKind {
	/// A Conversation came into existence.
	#[serde(rename = "conversation.created")]
	ConversationCreated {
		/// Its retention choice.
		retention: RetentionPolicy,
	},
	/// A Run was recorded in the `created` state.
	#[serde(rename = "run.created")]
	RunCreated {},
	/// One scope stored a Setting value.
	#[serde(rename = "setting.changed")]
	SettingChanged {
		/// The Setting that changed.
		key: SettingKey,
		/// The scope that stores the new value.
		scope: SettingScope,
		/// The value that scope now stores.
		value: SettingValue,
	},
	/// One scope stopped storing its own value for a Setting.
	#[serde(rename = "setting.cleared")]
	SettingCleared {
		/// The Setting that was cleared.
		key: SettingKey,
		/// The scope that no longer stores a value.
		scope: SettingScope,
	},
	/// A Provider account was bound to this Plane. The Event names the
	/// binding, its Provider, and the backend that resolves its Credential;
	/// no part of the Credential itself is recorded (ADR-0076).
	#[serde(rename = "account.bound")]
	AccountBound {
		/// The binding that was established.
		binding_id: AccountBindingId,
		/// The Provider it authenticates to.
		provider: ProviderId,
		/// The backend that resolves its Credential.
		credential_source: CredentialSource,
	},
	/// An Account binding was removed from this Plane.
	#[serde(rename = "account.unbound")]
	AccountUnbound {
		/// The binding that was removed.
		binding_id: AccountBindingId,
	},
	/// An owner began a new authority epoch of the Security audit after it
	/// failed to validate (ADR-0105). The gap it leaves behind is recorded
	/// in the audit itself; the journal only says that it happened.
	#[serde(rename = "audit.epoch_begun")]
	AuditEpochBegun {
		/// The epoch that now holds the chain the Plane vouches for.
		epoch: AuditEpoch,
	},
	/// A Run moved to a later lifecycle state.
	#[serde(rename = "run.lifecycle_changed")]
	RunLifecycleChanged {
		/// The state it left.
		from: RunLifecycle,
		/// The state it entered.
		to: RunLifecycle,
	},
	/// An Event this core cannot interpret: a kind or payload version
	/// written by a newer core that shared the store (ADR-0073). It is
	/// retained and forwarded as recorded so a previous release still serves
	/// the whole journal and clients render it generically (ADR-0094).
	#[serde(skip)]
	Unrecognized(EventPayload),
}

impl EventKind {
	/// The journal form of this kind.
	///
	/// # Errors
	///
	/// Returns an `internal` [`CoreError`] if the kind cannot be encoded,
	/// which indicates a programming error.
	pub fn encode(&self) -> Result<EventPayload, CoreError> {
		match self {
			Self::Unrecognized(payload) => Ok(payload.clone()),
			Self::ConversationCreated { .. }
			| Self::RunCreated {}
			| Self::RunLifecycleChanged { .. }
			| Self::SettingChanged { .. }
			| Self::SettingCleared { .. }
			| Self::AccountBound { .. }
			| Self::AccountUnbound { .. }
			| Self::AuditEpochBegun { .. } => {
				let encoded = serde_json::to_value(self).map_err(|error| {
					CoreError::internal("event.unencodable", error.to_string())
				})?;
				let serde_json::Value::Object(mut fields) = encoded else {
					return Err(CoreError::internal(
						"event.unencodable",
						"not an object",
					));
				};
				let Some(serde_json::Value::String(kind)) =
					fields.remove("kind")
				else {
					return Err(CoreError::internal(
						"event.unencodable",
						"no kind",
					));
				};
				let payload = fields.remove("payload").unwrap_or_else(|| {
					serde_json::Value::Object(serde_json::Map::new())
				});
				Ok(EventPayload {
					kind,
					payload_version: PAYLOAD_VERSION,
					payload,
				})
			}
		}
	}

	/// Interprets a journal row. Only a payload that is not JSON is an
	/// integrity failure; a kind or version this core does not know becomes
	/// [`EventKind::Unrecognized`].
	fn decode(record: &EventRecord) -> Result<Self, CoreError> {
		let payload: serde_json::Value = serde_json::from_str(&record.payload)
			.map_err(|error| {
				CoreError::internal("event.malformed", error.to_string())
			})?;
		if record.payload_version == PAYLOAD_VERSION
			&& let Ok(kind) = serde_json::from_value(serde_json::json!({
				"kind": record.kind,
				"payload": payload,
			})) {
			return Ok(kind);
		}
		Ok(Self::Unrecognized(EventPayload {
			kind: record.kind.clone(),
			payload_version: record.payload_version,
			payload,
		}))
	}

	pub(crate) fn to_record(
		&self,
		actor: &Actor,
		subject: EventSubject,
		recorded_at_unix_ms: i64,
	) -> Result<NewEvent, CoreError> {
		let EventPayload {
			kind,
			payload_version,
			payload,
		} = self.encode()?;
		let (conversation_id, run_id) = match subject {
			EventSubject::Plane => (None, None),
			EventSubject::Conversation(conversation_id) => {
				(Some(conversation_id), None)
			}
			EventSubject::Run {
				conversation_id,
				run_id,
			} => (Some(conversation_id), Some(run_id)),
		};
		Ok(NewEvent {
			event_id: Uuid::now_v7(),
			actor: actor.record(),
			recorded_at_unix_ms,
			conversation_id: conversation_id.map(|id| id.0),
			run_id: run_id.map(|id| id.0),
			kind,
			payload_version,
			payload: payload.to_string(),
			class: EventClass::Semantic,
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
