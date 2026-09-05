//! Typed rows exchanged with the store. Enum variants carry their durable
//! column spelling, which also serves as their JSON spelling.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::StoreError;

/// Whether Jet keeps a Conversation after its final Run (ADR-0001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionPolicy {
	/// Keep the Conversation and its history indefinitely. The default.
	Retain,
	/// Forget the Conversation once it has no active Run and no other
	/// protected state.
	ForgetAfterFinalRun,
}

/// Whether an Event is durable Conversation history or compactable
/// operational noise (ADR-0078).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventClass {
	/// Semantic history follows its Conversation's retention policy.
	Semantic,
	/// Superseded operational noise may be removed after snapshot coverage.
	Operational,
}

/// Opaque evidence that a durable snapshot covers Events through a sequence.
///
/// Only a write transaction over the durable normalized projection can mint
/// this value; compaction callers cannot substitute an asserted integer
/// (ADR-0078).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedSnapshotCoverage {
	pub(crate) plane_id: Uuid,
	pub(crate) sequence: u64,
}

impl VerifiedSnapshotCoverage {
	pub(crate) fn parts(self) -> (Uuid, u64) {
		(self.plane_id, self.sequence)
	}
}

impl EventClass {
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Semantic => "semantic",
			Self::Operational => "operational",
		}
	}
}

/// Mutually exclusive lifecycle of one Run (ADR-0065).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunLifecycle {
	/// Recorded but not yet launching.
	Created,
	/// Launching its Harness.
	Starting,
	/// Executing; activity is reported separately.
	Active,
	/// Ending gracefully.
	Stopping,
	/// Terminal: finished its work.
	Completed,
	/// Terminal: ended with an error.
	Failed,
	/// Terminal: ended on request.
	Canceled,
	/// Terminal: its execution can no longer be observed.
	Lost,
}

impl RunLifecycle {
	/// Whether this lifecycle state is one of the four terminal results.
	#[must_use]
	pub fn is_terminal(self) -> bool {
		match self {
			Self::Created | Self::Starting | Self::Active | Self::Stopping => {
				false
			}
			Self::Completed | Self::Failed | Self::Canceled | Self::Lost => {
				true
			}
		}
	}

	/// The durable spelling, also used in messages and JSON.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Created => "created",
			Self::Starting => "starting",
			Self::Active => "active",
			Self::Stopping => "stopping",
			Self::Completed => "completed",
			Self::Failed => "failed",
			Self::Canceled => "canceled",
			Self::Lost => "lost",
		}
	}

	pub(crate) fn parse(text: &str) -> Option<Self> {
		[
			Self::Created,
			Self::Starting,
			Self::Active,
			Self::Stopping,
			Self::Completed,
			Self::Failed,
			Self::Canceled,
			Self::Lost,
		]
		.into_iter()
		.find(|lifecycle| lifecycle.as_str() == text)
	}
}

impl RetentionPolicy {
	/// The durable spelling, also used in JSON.
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Retain => "retain",
			Self::ForgetAfterFinalRun => "forget_after_final_run",
		}
	}

	pub(crate) fn parse(text: &str) -> Option<Self> {
		[Self::Retain, Self::ForgetAfterFinalRun]
			.into_iter()
			.find(|retention| retention.as_str() == text)
	}
}

/// The authenticated origin of a Command or Event (ADR-0063).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorRecord {
	/// An interactive GUI client identified by its durable Client identity.
	InteractiveClient {
		/// The client's durable identity.
		client_id: Uuid,
	},
}

impl ActorRecord {
	pub(crate) fn columns(self) -> (&'static str, Uuid) {
		match self {
			Self::InteractiveClient { client_id } => {
				("interactive_client", client_id)
			}
		}
	}

	pub(crate) fn parse(kind: &str, id: &str) -> Result<Self, StoreError> {
		match kind {
			"interactive_client" => Ok(Self::InteractiveClient {
				client_id: parse_uuid("actor_id", id)?,
			}),
			_ => Err(column_error(
				"actor_kind",
				format!("unknown actor {kind:?} with id {id:?}"),
			)),
		}
	}
}

/// A durable receipt for one accepted Actor-scoped Command identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandReceiptRecord {
	/// The authenticated Actor that submitted the Command.
	pub actor: ActorRecord,
	/// The Actor-scoped Command identity.
	pub command_id: Uuid,
	/// SHA-256 digest of the request content, discarded after thirty days.
	pub request_digest: Option<[u8; 32]>,
	/// When the Command was accepted.
	pub recorded_at_unix_ms: i64,
	/// Version of the private outcome encoding, discarded after thirty days.
	pub outcome_version: Option<u32>,
	/// Encoded authoritative outcome, discarded after thirty days.
	pub outcome: Option<String>,
}

/// A Command receipt to record in the accepting state transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCommandReceipt {
	/// The authenticated Actor that submitted the Command.
	pub actor: ActorRecord,
	/// The Actor-scoped Command identity.
	pub command_id: Uuid,
	/// SHA-256 digest of the request content.
	pub request_digest: [u8; 32],
	/// When the Command was accepted.
	pub recorded_at_unix_ms: i64,
	/// Version of the private outcome encoding.
	pub outcome_version: u32,
	/// Encoded authoritative outcome.
	pub outcome: String,
}

/// Closed durable spelling of external work understood by this release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKindRecord {
	/// Start one Run's managed processes.
	StartRun,
}

impl EffectKindRecord {
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::StartRun => "run.start",
		}
	}

	pub(crate) fn parse(text: &str) -> Option<Self> {
		match text {
			"run.start" => Some(Self::StartRun),
			_ => None,
		}
	}
}

/// Durable lifecycle of one external-work Effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectStateRecord {
	/// Committed but never handed to its Adapter.
	Pending,
	/// Handed to its Adapter without a durably recorded outcome yet.
	InFlight,
	/// External work completed successfully.
	Completed,
	/// External work returned a definite failure.
	Failed,
	/// Reconciliation could not establish a safe outcome.
	OutcomeUnknown,
}

/// Evidence that determines whether an interrupted Effect may be repeated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectSafetyRecord {
	/// The operation cannot mutate external state.
	ReadOnly {
		/// Maximum number of attempts allowed by policy.
		max_attempts: u32,
	},
	/// The target deduplicates attempts under one stable external key.
	Idempotent {
		/// Key supplied unchanged to the external target on every attempt.
		external_key: Uuid,
		/// Maximum number of attempts allowed by policy.
		max_attempts: u32,
	},
	/// The operation may mutate state and cannot be safely deduplicated.
	Ambiguous,
}

impl EffectSafetyRecord {
	pub(crate) fn columns(self) -> (&'static str, Option<Uuid>, u32) {
		match self {
			Self::ReadOnly { max_attempts } => {
				("read_only", None, max_attempts)
			}
			Self::Idempotent {
				external_key,
				max_attempts,
			} => ("idempotent", Some(external_key), max_attempts),
			Self::Ambiguous => ("ambiguous", None, 1),
		}
	}
}

impl EffectStateRecord {
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Pending => "pending",
			Self::InFlight => "in_flight",
			Self::Completed => "completed",
			Self::Failed => "failed",
			Self::OutcomeUnknown => "outcome_unknown",
		}
	}

	pub(crate) fn parse(text: &str) -> Option<Self> {
		[
			Self::Pending,
			Self::InFlight,
			Self::Completed,
			Self::Failed,
			Self::OutcomeUnknown,
		]
		.into_iter()
		.find(|state| state.as_str() == text)
	}
}

/// A newly committed request for external work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEffect {
	/// Globally unique identity reused for every attempt.
	pub effect_id: Uuid,
	/// Actor-scoped Command identity that initiated the Effect.
	pub command_id: Uuid,
	/// Run affected by the work.
	pub run_id: Option<Uuid>,
	/// Closed Effect kind understood by the core.
	pub kind: EffectKindRecord,
	/// Evidence and retry bound governing interrupted attempts.
	pub safety: EffectSafetyRecord,
}

/// One durable external-work Effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectRecord {
	/// Globally unique identity reused for every attempt.
	pub effect_id: Uuid,
	/// Actor-scoped Command identity that initiated the Effect.
	pub command_id: Uuid,
	/// Run affected by the work.
	pub run_id: Option<Uuid>,
	/// Closed Effect kind understood by the core.
	pub kind: EffectKindRecord,
	/// Evidence and retry bound governing interrupted attempts.
	pub safety: EffectSafetyRecord,
	/// Durable lifecycle state.
	pub state: EffectStateRecord,
	/// Number of times the Effect was handed to its Adapter.
	pub attempt_count: u32,
}

/// A Conversation to insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewConversation {
	/// Globally unique identity chosen by the caller.
	pub conversation_id: Uuid,
	/// Retention choice.
	pub retention: RetentionPolicy,
	/// When the caller recorded the Conversation.
	pub created_at_unix_ms: i64,
}

/// Current state of one Conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationRecord {
	/// Globally unique identity.
	pub conversation_id: Uuid,
	/// Retention choice.
	pub retention: RetentionPolicy,
	/// When the Conversation was recorded.
	pub created_at_unix_ms: i64,
}

/// Opaque-to-callers key for continuing a Conversation keyset page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationPageKey(pub(crate) i64);

/// Where a Conversation page begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationPageStart {
	/// Read the first page of the snapshot.
	First,
	/// Continue strictly after a key returned by the previous page.
	After(ConversationPageKey),
}

/// A Run to insert in the `created` lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewRun {
	/// Globally unique identity chosen by the caller.
	pub run_id: Uuid,
	/// The Conversation this Run executes.
	pub conversation_id: Uuid,
	/// When the caller recorded the Run.
	pub created_at_unix_ms: i64,
}

/// Current state of one Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunRecord {
	/// Globally unique identity.
	pub run_id: Uuid,
	/// The Conversation this Run executes.
	pub conversation_id: Uuid,
	/// Monotonic version for conflict-sensitive Run Commands.
	pub revision: u64,
	/// Current lifecycle state.
	pub lifecycle: RunLifecycle,
	/// When the Run was recorded.
	pub created_at_unix_ms: i64,
	/// When the Run reached a terminal state, if it has.
	pub ended_at_unix_ms: Option<i64>,
}

/// An Event to append to the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
	/// Globally unique identity chosen by the caller.
	pub event_id: Uuid,
	/// Who caused the Event.
	pub actor: ActorRecord,
	/// When the caller recorded the Event; display metadata only
	/// (ADR-0069).
	pub recorded_at_unix_ms: i64,
	/// The Conversation the Event concerns, if any.
	pub conversation_id: Option<Uuid>,
	/// The Run the Event concerns, if any.
	pub run_id: Option<Uuid>,
	/// Indexed kind such as `run.lifecycle_changed`.
	pub kind: String,
	/// Version of the payload schema for `kind`.
	pub payload_version: u32,
	/// Bounded JSON payload.
	pub payload: String,
	/// Retention class governing whether compaction may remove this Event.
	pub class: EventClass,
}

/// One journal row (ADR-0096).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
	/// Plane-local monotonic position; never reused (ADR-0069).
	pub sequence: u64,
	/// Globally unique identity.
	pub event_id: Uuid,
	/// Who caused the Event.
	pub actor: ActorRecord,
	/// When the Event was recorded.
	pub recorded_at_unix_ms: i64,
	/// The Conversation the Event concerns, if any.
	pub conversation_id: Option<Uuid>,
	/// The Run the Event concerns, if any.
	pub run_id: Option<Uuid>,
	/// Indexed kind such as `run.lifecycle_changed`.
	pub kind: String,
	/// Version of the payload schema for `kind`.
	pub payload_version: u32,
	/// Bounded JSON payload.
	pub payload: String,
}

/// Where one Setting value is stored (ADR-0085). A resolution reads the
/// Plane's values and the values of the scope it addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingScopeRecord {
	/// Values that apply everywhere on the Plane.
	Plane,
	/// Values that apply inside one registered Project.
	Project {
		/// The Project the values apply to.
		project_id: Uuid,
	},
	/// Values that apply inside one Conversation.
	Conversation {
		/// The Conversation the values apply to.
		conversation_id: Uuid,
	},
}

impl SettingScopeRecord {
	pub(crate) fn columns(self) -> (&'static str, Option<Uuid>) {
		match self {
			Self::Plane => ("plane", None),
			Self::Project { project_id } => ("project", Some(project_id)),
			Self::Conversation { conversation_id } => {
				("conversation", Some(conversation_id))
			}
		}
	}

	pub(crate) fn parse(
		scope: &str,
		scope_id: Option<&str>,
	) -> Result<Self, StoreError> {
		match (scope, scope_id) {
			("plane", None) => Ok(Self::Plane),
			("project", Some(id)) => Ok(Self::Project {
				project_id: parse_uuid("scope_id", id)?,
			}),
			("conversation", Some(id)) => Ok(Self::Conversation {
				conversation_id: parse_uuid("scope_id", id)?,
			}),
			(scope, _) => Err(column_error(
				"scope",
				format!("unknown or incomplete Setting scope {scope:?}"),
			)),
		}
	}
}

/// One Setting value as one scope stores it. The core owns the key
/// vocabulary and the value encoding; the store keeps both as bounded text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingRecord {
	/// Durable key spelling such as `git.auto_commit`.
	pub key: String,
	/// The scope that stores this value.
	pub scope: SettingScopeRecord,
	/// Encoded value, bounded by the store.
	pub value: String,
	/// When the value was last written.
	pub updated_at_unix_ms: i64,
}

/// Reports a column whose stored value no longer parses. Every conversion
/// failure inside the store is an integrity failure.
pub(crate) fn column_error(column: &str, message: String) -> StoreError {
	StoreError::Integrity(format!("column {column}: {message}"))
}

/// One fixed-width blob column, as the width its algorithm or hash fixes.
pub(crate) fn parse_bytes<const N: usize>(
	column: &str,
	bytes: Vec<u8>,
) -> Result<[u8; N], StoreError> {
	let length = bytes.len();
	bytes.try_into().map_err(|_| {
		column_error(column, format!("the value has {length} bytes"))
	})
}

pub(crate) fn parse_uuid(column: &str, text: &str) -> Result<Uuid, StoreError> {
	Uuid::parse_str(text)
		.map_err(|error| column_error(column, format!("not a UUID: {error}")))
}

pub(crate) fn parse_optional_uuid(
	column: &str,
	text: Option<&str>,
) -> Result<Option<Uuid>, StoreError> {
	text.map(|text| parse_uuid(column, text)).transpose()
}
