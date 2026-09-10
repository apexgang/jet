//! Encodes and decodes versioned Event journal records.

use super::{
	Event, EventActor, EventId, EventKind, EventPayload, EventSequence,
	EventSubject,
};
use crate::{
	Actor,
	conversation::{ConversationId, RunId},
	error::CoreError,
	system_time,
};
use jet_store::{EventClass, EventRecord, NewEvent};
use uuid::Uuid;

/// Schema version of every payload this core writes.
pub(super) const PAYLOAD_VERSION: u32 = 1;

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
			Self::AutoContinueChanged { .. }
			| Self::AutoContinueConfigured { .. }
			| Self::ScheduleCreated { .. }
			| Self::ScheduleCanceled { .. }
			| Self::ScheduleFired { .. }
			| Self::UserEditApplied { .. }
			| Self::ReviewSubmitted { .. }
			| Self::ConversationNameChanged { .. }
			| Self::RunNameChanged { .. }
			| Self::ChangeEvidenceRecorded { .. }
			| Self::ChangeCheckpointRecorded { .. }
			| Self::ArtifactPublished { .. }
			| Self::TurnInput { .. }
			| Self::TurnChanged { .. }
			| Self::HandoffCreated { .. }
			| Self::ConversationCreated { .. }
			| Self::TerminalStateChanged { .. }
			| Self::RunControlRequested { .. }
			| Self::RunTerminated { .. }
			| Self::RunActivityChanged { .. }
			| Self::RunProcessesChanged { .. }
			| Self::ApprovalRequested { .. }
			| Self::ApprovalReviewed { .. }
			| Self::ApprovalRetryAuthorized { .. }
			| Self::RunOutput { .. }
			| Self::RunNativeConversation { .. }
			| Self::ConversationImported { .. }
			| Self::RunCreated { .. }
			| Self::RunLifecycleChanged { .. }
			| Self::SettingChanged { .. }
			| Self::SettingCleared { .. }
			| Self::AccountBound { .. }
			| Self::AccountUnbound { .. }
			| Self::UsageRecorded { .. }
			| Self::AuditEpochBegun { .. }
			| Self::PairingGateChanged { .. }
			| Self::PairingOffered { .. }
			| Self::PairingClaimed { .. }
			| Self::PairingConfirmed { .. }
			| Self::PairingCompleted { .. }
			| Self::PairingOfferEnded { .. }
			| Self::PairedClientAccessChanged { .. }
			| Self::PairedClientRevoked { .. }
			| Self::ProjectRegistered { .. }
			| Self::WorkspaceCreated { .. }
			| Self::WorkspaceSeeded { .. }
			| Self::WorkspacePromotionRecorded { .. }
			| Self::WorkspacePromotionSettled { .. } => {
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
		self.to_record_as(actor.clone().into(), subject, recorded_at_unix_ms)
	}

	pub(crate) fn to_record_as(
		&self,
		actor: EventActor,
		subject: EventSubject,
		recorded_at_unix_ms: i64,
	) -> Result<NewEvent, CoreError> {
		let EventPayload {
			kind,
			payload_version,
			payload,
		} = self.encode()?;
		let auto_continue = matches!(actor, EventActor::AutoContinue { .. });
		let (actor, origin) = actor.provenance();
		let mut payload = payload;
		if auto_continue && let Some(fields) = payload.as_object_mut() {
			fields.insert(
				"_jet_auto_continue".into(),
				serde_json::Value::Bool(true),
			);
		}
		if let Some(origin) = origin {
			// Additive metadata leaves legacy actor columns and payload decoders
			// readable during rollback. Origin is always derived by Core.
			payload
				.as_object_mut()
				.ok_or_else(|| {
					CoreError::internal(
						"event.unencodable",
						"payload is not an object",
					)
				})?
				.insert("_jet_origin".into(), origin);
		}
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
			actor,
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
			actor: EventActor::from_record(&record)?,
			recorded_at: system_time(record.recorded_at_unix_ms),
			conversation_id: record.conversation_id.map(ConversationId),
			run_id: record.run_id.map(RunId),
			kind,
		})
	}
}
