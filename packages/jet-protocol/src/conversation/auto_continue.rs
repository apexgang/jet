//! Auto-continue wire contracts (Jet 1.34).
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Where the user configures Auto-continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AutoContinueTarget {
	/// Default for this Home Plane's Account binding.
	AccountBinding(Uuid),
	/// One quota-exhaustion episode, consumed when selected.
	Conversation(Uuid),
}
/// Explicit retry limits; absence of a configured policy resolves to off.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AutoContinuePolicy {
	/// Do not admit automatic input.
	#[default]
	Off,
	/// Retry the same native Conversation and execution selection.
	Retry {
		/// Initial fallback delay, in milliseconds.
		delay_ms: u32,
		/// Maximum exponential fallback delay, in milliseconds.
		max_delay_ms: u32,
		/// Maximum admitted retries in one exhaustion episode.
		max_retries: u32,
		/// Exact continuation input, 1 to 8192 bytes.
		message: String,
	},
}
/// Durable outcome of the most recent retry decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AutoContinueStatus {
	/// The selected policy disabled this exhaustion episode.
	Disabled,
	/// Queue capacity prevented admission; the retry count has not advanced.
	Deferred,
	/// The Turn queue holds the retry until its due time.
	Pending,
	/// The retry was claimed for native delivery.
	Dispatched,
	/// New user input canceled the retry.
	Canceled,
	/// The configured retry count was reached.
	Exhausted,
}
/// The evidence and policy retained for the latest retry decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AutoContinueRetry {
	/// Run whose structured quota condition triggered the decision.
	pub run_id: Uuid,
	/// Input that encountered this quota condition.
	pub triggering_turn: uuid::Uuid,
	/// Turn admitted by this decision, absent until queue admission succeeds.
	pub retry_turn: Option<uuid::Uuid>,

	/// Provider-reported Usage condition, retained without guessing from text.
	pub usage: AutoContinueUsage,
	/// Plane clock at the observation.
	pub observed_at_unix_ms: i64,
	/// Earliest permitted delivery, respecting the Provider reset time.
	pub due_at_unix_ms: i64,
	/// Number of automatic retries admitted in this episode.
	pub retry_count: u32,
	/// Exact policy selected for this episode, including its message.
	pub policy: AutoContinuePolicy,
	/// Where that policy was selected.
	pub selected_from: AutoContinueTarget,
	/// Latest decision outcome.
	pub status: AutoContinueStatus,
}
/// A fenced policy and retry snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AutoContinueSnapshot {
	/// Plane Event cursor in the same read transaction.
	#[serde(with = "crate::transport::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub cursor: u64,
	/// Configured policy, or off when unset.
	pub policy: AutoContinuePolicy,
	/// Latest retry decision for a Conversation.
	pub retry: Option<AutoContinueRetry>,
}

/// Structured Usage condition that triggered a retry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AutoContinueUsage {
	/// Provider window name.
	pub window: String,
	/// Scope of the quota.
	pub scope: crate::QuotaScope,
	/// Reported fill.
	pub measure: crate::QuotaMeasure,
	/// Provider window duration.
	pub window_seconds: Option<u64>,
	/// Reset countdown at observation.
	pub resets_in_seconds: Option<u64>,
	/// Report certainty.
	pub estimation: crate::UsageEstimation,
	/// Report finality.
	pub finality: crate::UsageFinality,
}
