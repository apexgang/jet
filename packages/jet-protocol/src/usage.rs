//! Usage record wire types (Jet 1.28, ADR-0023).
//!
//! A snapshot says which Plane it came from and how far each Provider
//! response can be trusted, because no Plane can answer for a Provider
//! account as a whole: only a GUI holding several Planes can group them,
//! and only over the Planes it is connected to (ADR-0016).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What a Usage Query covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum UsageSelection {
	/// Every Account binding and Conversation on the Plane.
	Plane,
	/// One Plane-local Account binding.
	Binding {
		/// The binding.
		binding_id: Uuid,
	},
	/// One Conversation. Quota windows belong to an Account binding rather
	/// than to a Conversation, so this answers with consumption alone.
	Conversation {
		/// The Conversation.
		conversation_id: Uuid,
	},
	/// One Run, answered the same way as its Conversation.
	Run {
		/// The Run.
		run_id: Uuid,
	},
}

/// Whether Jet measured a record's numbers or derived them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum UsageEstimation {
	/// Reported by the Harness or the Provider.
	Measured,
	/// Derived by Jet, and never a Provider's own accounting.
	Estimated,
}

/// Whether a record's numbers can still change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum UsageFinality {
	/// The work it covers had not finished when it was reported.
	Interim,
	/// The work it covers is over.
	Final,
}

/// Whether a Provider-reported window still stands for what the Provider
/// would say now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UsageFreshness {
	/// Read recently enough to stand for the Provider's current state.
	Fresh,
	/// Older than the interval a Plane refreshes on; it is history rather
	/// than a current reading (ADR-0045).
	Stale,
	/// The Provider did not answer the last time the Plane asked.
	Unreachable {
		/// Why it did not answer.
		reason: String,
	},
}

/// What one Provider-reported quota window covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "covers", rename_all = "snake_case")]
pub enum QuotaScope {
	/// The Provider account as a whole.
	ProviderAccount,
	/// One Model of that account.
	Model {
		/// The Model the window limits.
		model: String,
	},
}

/// The unit a Provider stated one window in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum QuotaUnit {
	/// Inference tokens.
	Tokens,
	/// Requests.
	Requests,
	/// Provider-defined credits.
	Credits,
	/// Hundredths of a percent of the window, out of 10,000.
	Share,
}

/// How full a Provider says one window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct QuotaMeasure {
	/// The unit the Provider stated it in.
	pub unit: QuotaUnit,
	/// How much of the window it reported as consumed.
	pub used: u64,
	/// The limit it stated, where it stated one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub limit: Option<u64>,
}

/// The freshest Provider response about one quota window. Windows are
/// reported one by one and are never added together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct QuotaWindow {
	/// The Plane-local Account binding it belongs to.
	pub binding_id: Uuid,
	/// The Provider that reported it.
	pub provider: String,
	/// The Provider's own name for the window.
	pub window: String,
	/// What the window covers.
	pub scope: QuotaScope,
	/// How full the Provider said it was.
	pub measure: QuotaMeasure,
	/// How long the window lasts, where the Provider stated it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub window_seconds: Option<u64>,
	/// When it refills, where the Provider stated it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub resets_at_unix_ms: Option<i64>,
	/// Whether the Provider measured it or Jet derived it.
	pub estimation: UsageEstimation,
	/// Whether the window has closed.
	pub finality: UsageFinality,
	/// When the Plane observed the response.
	pub observed_at_unix_ms: i64,
	/// Whether it still stands for what the Provider would say now.
	pub freshness: UsageFreshness,
}

/// The token counts one total carries.
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UsageTokens {
	/// Tokens sent.
	pub input: u64,
	/// Tokens served from the Provider's cache.
	pub cached_input: u64,
	/// Tokens generated.
	pub output: u64,
	/// Tokens spent on reasoning, where the Provider counts them apart.
	pub reasoning: u64,
}

/// Deduplicated consumption for one Model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ModelConsumption {
	/// The Model, absent where the Harness named none.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub model: Option<String>,
	/// The counts.
	pub tokens: UsageTokens,
	/// How many deduplicated measurements contributed.
	pub measurements: u64,
	/// How many of them Jet estimated rather than measured.
	pub estimated: u64,
	/// How many of them can still change.
	pub interim: u64,
	/// When the newest contributing measurement was observed.
	pub last_observed_at_unix_ms: i64,
}

/// Deduplicated Jet-observed consumption, with the uncertainty in it left
/// visible rather than folded into the total.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ObservedConsumption {
	/// The counts across every Model in the selection.
	pub tokens: UsageTokens,
	/// How many deduplicated measurements contributed.
	pub measurements: u64,
	/// How many of them Jet estimated rather than measured.
	pub estimated: u64,
	/// How many of them can still change.
	pub interim: u64,
	/// When the newest contributing measurement was observed.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub last_observed_at_unix_ms: Option<i64>,
	/// The same consumption per Model.
	pub models: Vec<ModelConsumption>,
}

/// What one Plane knows about Usage for the selected scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PlaneUsage {
	/// Newest Event sequence visible when the snapshot was read, carried
	/// as a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub cursor: u64,
	/// The Plane every record here was observed on. A total covers this
	/// Plane alone (ADR-0016).
	pub plane_id: Uuid,
	/// The freshest Provider response about each window of each selected
	/// Account binding.
	pub quota_windows: Vec<QuotaWindow>,
	/// Deduplicated Jet-observed consumption for the selection.
	pub consumption: ObservedConsumption,
}
