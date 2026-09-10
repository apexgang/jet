//! Immutable daily Scheduled tasks attached to retained Conversations (ADR-0024).
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One intended local occurrence, resolved once and persisted before delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ScheduleFiring {
	/// Deterministic identity, also the admitted Turn identity.
	pub firing_id: Uuid,
	/// Original local date and time, including a nonexistent or repeated time.
	pub intended_local: String,
	/// Selected UTC instant in signed Unix milliseconds.
	pub due_at_unix_ms: i64,
}
/// An enabled daily Scheduled task. Cancel and create anew to change its rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ScheduledTask {
	/// Immutable schedule identity.
	pub schedule_id: Uuid,
	/// Durable owning Conversation.
	pub conversation_id: Uuid,
	/// Client that authorized scheduled input.
	pub authorized_by: Uuid,
	/// Original IANA zone, independent of the Plane's current zone.
	pub time_zone: String,
	/// Daily local time in HH:MM:SS form.
	pub local_time: String,
	/// Turn input, bounded to 8192 UTF-8 bytes.
	pub prompt: String,
	/// Next intended occurrence, retained unchanged across restarts.
	pub next: ScheduleFiring,
}
/// Enabled schedules and their snapshot fence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ScheduledTasks {
	/// Plane Event high-water cursor in this read transaction.
	#[serde(with = "crate::transport::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub cursor: u64,
	/// At most 32 schedules in the addressed Conversation.
	pub tasks: Vec<ScheduledTask>,
}
