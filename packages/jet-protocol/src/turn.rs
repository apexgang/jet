//! Durable admission identities and observable Turn queue state (ADR-0037).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The independently coalesced input class; it grants no Actor authority.
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TurnSource {
	/// User input is never replaced.
	#[default]
	User,
	/// The newest pending Scheduled-task input.
	Schedule,
	/// The newest pending Auto-continue input.
	AutoContinue,
}
/// A Turn's execution or final admission outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
	/// Waiting in authoritative sequence order.
	Queued,
	/// Claimed durably before native delivery; never redelivered blindly.
	Active,
	/// Native completion was committed.
	Completed,
	/// A newer input took the same background slot.
	Superseded,
	/// User input canceled pending Auto-continue, or an Interrupt turn
	/// ended this claimed work while its Run stayed alive.
	Canceled,
	/// Its own client withdrew queued user work.
	Withdrawn,
	/// The Run ended without a successful turn completion.
	Failed,
	/// Delivery or execution cannot currently be reconciled.
	OutcomeUnknown,
}
/// One durable input identity; the queue order is its sequence, never wall time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Turn {
	/// Plane-assigned identity, also used for Craft correlation.
	pub turn_id: Uuid,
	/// Monotonic within this Conversation, including replaced entries.
	#[serde(with = "crate::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub sequence: u64,
	/// Authenticated client that authorized this input.
	pub client_id: Uuid,
	/// Slot selection, independent from authenticated attribution.
	pub source: TurnSource,
	/// Current state or recorded final outcome.
	pub state: TurnState,
	/// Execution that claimed this input, when any.
	pub run_id: Option<Uuid>,
}
/// A bounded queue snapshot. Vector order is queue position, active input first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TurnQueue {
	/// Event cursor from the same read transaction.
	#[serde(with = "crate::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub cursor: u64,
	/// At most 128 entries; prompts remain in the admission Events.
	pub turns: Vec<Turn>,
}
