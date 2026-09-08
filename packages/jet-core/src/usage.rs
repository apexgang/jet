//! Normalized Usage records: what a Craft may report, and what a Query
//! answers with (ADR-0023, ADR-0045).
//!
//! Two things are measured here and they are never mixed. A Provider
//! reports how full a quota window is; Jet observes what a Harness said it
//! consumed. Each record carries where it came from, what it covers,
//! whether Jet measured or estimated it, whether it can still change, and
//! the Account binding, Model, Conversation, Run, Plane, and time it
//! belongs to.
//!
//! Nothing here claims more than one Plane knows. A Plane answers for its
//! own bindings; assembling connected Planes into one Provider account is
//! the GUI's claim to make, and it can only make it about the Planes it is
//! actually connected to (ADR-0016).

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{
	AccountBindingId, ConversationId, EventSequence, PlaneId, ProviderId, RunId,
};

/// How long a Provider-reported window stands for what the Provider would
/// say now. It is the interval an idle Plane refreshes on (ADR-0045):
/// past it, the window is history rather than a current reading.
pub(crate) const USAGE_FRESHNESS_MS: i64 = 15 * 60 * 1000;

/// Longest Provider window name, Model name, or unavailability reason a
/// record carries. Each is bounded metadata, not a payload.
pub(crate) const MAX_USAGE_TEXT: usize = 128;

/// Longest reason text an unreachable Provider report carries.
pub(crate) const MAX_REASON_TEXT: usize = 256;

/// An inference model made available through a Provider account.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelId(pub String);

/// Where one Usage record came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
	/// A Provider's own accounting of one of its quota windows.
	ProviderQuota,
	/// Jet's own accounting of what a Harness reported it consumed.
	JetObserved,
	/// A Provider that did not answer for one of its windows.
	ProviderUnreachable,
}

/// Whether Jet measured a record's numbers or derived them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageEstimation {
	/// Reported by the Harness or the Provider.
	Measured,
	/// Derived by Jet, and never presented as a Provider's own accounting.
	Estimated,
}

/// Whether a record's numbers can still change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageFinality {
	/// The work it covers had not finished when it was reported.
	Interim,
	/// The work it covers is over and the numbers no longer move.
	Final,
}

/// The token counts one Jet-observed measurement carries. Tokens a
/// Provider wrote to its cache count as input; the Harness's own
/// vocabulary stays in the journalled native event beside this record.
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
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

/// What one Jet-observed measurement covers, and what makes it one
/// measurement rather than another. Repeating a measurement replaces it;
/// nothing is added to a number that already covers it (ADR-0023).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageMeasurement {
	/// One turn, identified by the Harness's own usage identity when it
	/// supplies one and by the turn it covers otherwise.
	Turn {
		/// The turn the Harness was working on.
		turn: String,
		/// The Harness's or Provider's own identity for the measurement.
		native_usage_id: Option<String>,
	},
	/// The Run so far, as a cumulative total the Harness restates. A Run
	/// that reports these does not also have its turns added in.
	Run {
		/// The Harness's or Provider's own identity for the measurement.
		native_usage_id: Option<String>,
	},
}

/// One Jet-observed consumption measurement as a Craft reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedUsage {
	/// What it covers and what identifies it.
	pub measurement: UsageMeasurement,
	/// The Model that did the work, when the Harness names one.
	pub model: Option<ModelId>,
	/// Whether the Harness measured the counts or Jet derived them.
	pub estimation: UsageEstimation,
	/// Whether the counts can still change.
	pub finality: UsageFinality,
	/// The counts themselves.
	pub tokens: UsageTokens,
}

/// The unit a Provider stated one window in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaUnit {
	/// Inference tokens.
	Tokens,
	/// Requests.
	Requests,
	/// Provider-defined credits.
	Credits,
	/// Hundredths of a percent of the window, out of 10,000. It is what a
	/// Provider that reports a filled fraction rather than a countable
	/// limit supplies.
	Share,
}

/// How full a Provider says one window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaMeasure {
	/// The unit the Provider stated it in.
	pub unit: QuotaUnit,
	/// How much of the window it reported as consumed.
	pub used: u64,
	/// The limit it stated, where it stated one. A share always has
	/// 10,000.
	pub limit: Option<u64>,
}

/// What one Provider-reported quota window covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuotaScope {
	/// The Provider account as a whole.
	ProviderAccount,
	/// One Model of that account.
	Model(ModelId),
}

/// One Provider-reported quota window as a Craft reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaReport {
	/// The Provider's own name for the window, such as its five-hour or
	/// weekly limit. Windows are read freshest-first and never summed
	/// into one another.
	pub window: String,
	/// What the window covers.
	pub scope: QuotaScope,
	/// How full the Provider says it is.
	pub measure: QuotaMeasure,
	/// How long the window lasts, where the Provider states it.
	pub window_seconds: Option<u64>,
	/// How long until it refills, where the Provider states it. The Plane
	/// converts it with its own clock, so a Craft never asserts a time.
	pub resets_in_seconds: Option<u64>,
	/// Whether the Provider measured it or Jet derived it.
	pub estimation: UsageEstimation,
	/// Whether the window has closed.
	pub finality: UsageFinality,
}

/// What a Craft reports about the Usage of one execution (ADR-0023).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageReport {
	/// What the Harness said it consumed.
	Observed(ObservedUsage),
	/// A quota window the Provider reported.
	ProviderQuota(QuotaReport),
	/// A Provider that would not report its windows. The Plane says so
	/// rather than presenting the last windows it saw as current.
	ProviderUnreachable {
		/// Bounded, non-secret text naming why.
		reason: String,
	},
}

/// Whether a Provider-reported window still stands for what the Provider
/// would say now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageFreshness {
	/// Read recently enough to stand for the Provider's current state.
	Fresh,
	/// Older than the interval a Plane refreshes on. It is history, and
	/// never a current reading (ADR-0045).
	Stale,
	/// The Provider did not answer the last time the Plane asked.
	Unreachable {
		/// Why it did not answer, as the Craft reported it.
		reason: String,
	},
}

/// One Provider-reported quota window, as the freshest response about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaWindow {
	/// The Plane-local Account binding it belongs to.
	pub binding_id: AccountBindingId,
	/// The Provider that reported it.
	pub provider: ProviderId,
	/// The Provider's own name for the window.
	pub window: String,
	/// What the window covers.
	pub scope: QuotaScope,
	/// How full the Provider said it was.
	pub measure: QuotaMeasure,
	/// How long the window lasts, where the Provider stated it.
	pub window_seconds: Option<u64>,
	/// When it refills, where the Provider stated it.
	pub resets_at: Option<SystemTime>,
	/// Whether the Provider measured it or Jet derived it.
	pub estimation: UsageEstimation,
	/// Whether the window has closed.
	pub finality: UsageFinality,
	/// When the Plane observed the response.
	pub observed_at: SystemTime,
	/// Whether it still stands for what the Provider would say now.
	pub freshness: UsageFreshness,
}

/// Deduplicated Jet-observed consumption for one Model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelConsumption {
	/// The Model, absent where the Harness named none.
	pub model: Option<ModelId>,
	/// The counts.
	pub tokens: UsageTokens,
	/// How many deduplicated measurements contributed.
	pub measurements: u64,
	/// How many of them Jet estimated rather than measured.
	pub estimated: u64,
	/// How many of them can still change.
	pub interim: u64,
	/// When the newest contributing measurement was observed.
	pub last_observed_at: SystemTime,
}

/// Deduplicated Jet-observed consumption for one selection, with the
/// uncertainty in it left visible rather than folded into the total.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
	pub last_observed_at: Option<SystemTime>,
	/// The same consumption per Model.
	pub models: Vec<ModelConsumption>,
}

/// What one Usage Query covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageSelection {
	/// Every Account binding and Conversation on this Plane.
	Plane,
	/// One Plane-local Account binding.
	Binding(AccountBindingId),
	/// One Conversation. Quota windows belong to an Account binding rather
	/// than to a Conversation, so this selection answers with consumption.
	Conversation(ConversationId),
	/// One Run, answered the same way as its Conversation.
	Run(RunId),
}

/// What one Plane knows about Usage for the selected scope, fenced by the
/// journal position the snapshot was read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneUsage {
	/// Newest Event sequence visible when the snapshot was read.
	pub cursor: EventSequence,
	/// The Plane every record here was observed on. A total covers this
	/// Plane alone: no Plane knows what another one consumed, so none of
	/// them answers for a Provider account as a whole (ADR-0016).
	pub plane_id: PlaneId,
	/// The freshest Provider response about each window of each selected
	/// Account binding. Two windows are never added together.
	pub quota_windows: Vec<QuotaWindow>,
	/// Deduplicated Jet-observed consumption for the selection.
	pub consumption: ObservedConsumption,
}
