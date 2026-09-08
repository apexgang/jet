//! Normalized Usage a Craft reports for its execution (Craft 1.8,
//! ADR-0023). The Craft says what the Harness reported in Jet's
//! vocabulary; the complete native event still travels beside it.

use serde::{Deserialize, Serialize};

/// Whether the Harness or Provider stated the numbers, or the Craft
/// derived them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CraftUsageEstimation {
	/// Stated by the Harness or the Provider.
	Measured,
	/// Derived by the Craft, and never presented as a Provider's own
	/// accounting.
	Estimated,
}

/// Whether the numbers can still change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CraftUsageFinality {
	/// The work the numbers cover had not finished.
	Interim,
	/// The work is over and the numbers no longer move.
	Final,
}

/// What one consumption measurement covers, and what makes it one
/// measurement rather than another. The host replaces a repeated
/// measurement instead of adding it to the total (ADR-0023).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "covers", rename_all = "snake_case")]
pub enum CraftUsageMeasurement {
	/// One turn, named by the identity the host delivered with it.
	Turn {
		/// Correlation identity of the turn the numbers cover.
		turn: String,
		/// The Harness's or Provider's own identity for the measurement.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		native_usage_id: Option<String>,
	},
	/// The Run so far, as a cumulative total the Harness restates. A Run
	/// reporting these does not also have its turns counted.
	Run {
		/// The Harness's or Provider's own identity for the measurement.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		native_usage_id: Option<String>,
	},
}

/// The token counts one measurement carries. Tokens a Provider wrote to
/// its cache count as input; an absent count is zero.
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CraftUsageTokens {
	/// Tokens sent.
	#[serde(default)]
	pub input: u64,
	/// Tokens served from the Provider's cache.
	#[serde(default)]
	pub cached_input: u64,
	/// Tokens generated.
	#[serde(default)]
	pub output: u64,
	/// Tokens spent on reasoning, where the Provider counts them apart.
	#[serde(default)]
	pub reasoning: u64,
}

/// What the Harness reported it consumed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CraftObservedUsage {
	/// What it covers and what identifies it.
	pub measurement: CraftUsageMeasurement,
	/// The Model that did the work, when the Harness names one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub model: Option<String>,
	/// Whether the Harness stated the counts or the Craft derived them.
	pub estimation: CraftUsageEstimation,
	/// Whether the counts can still change.
	pub finality: CraftUsageFinality,
	/// The counts themselves.
	pub tokens: CraftUsageTokens,
}

/// What one Provider-reported quota window covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "covers", rename_all = "snake_case")]
pub enum CraftQuotaScope {
	/// The Provider account as a whole.
	ProviderAccount,
	/// One Model of that account.
	Model {
		/// The Model the window limits.
		model: String,
	},
}

/// What a share is measured against: hundredths of a percent, so a filled
/// fraction crosses the wire without a floating point.
pub const QUOTA_SHARE_LIMIT: u64 = 10_000;

/// The unit a Provider stated one window in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CraftQuotaUnit {
	/// Inference tokens.
	Tokens,
	/// Requests.
	Requests,
	/// Provider-defined credits.
	Credits,
	/// Hundredths of a percent of the window, out of 10,000, for a
	/// Provider that reports a filled fraction rather than a limit.
	Share,
}

/// One quota window exactly as the Provider reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CraftQuotaWindow {
	/// The Provider's own name for the window, such as its five-hour or
	/// weekly limit. The host keeps windows apart and never sums them.
	pub window: String,
	/// What the window covers.
	pub scope: CraftQuotaScope,
	/// The unit the Provider stated it in.
	pub unit: CraftQuotaUnit,
	/// How much of the window the Provider reported as consumed.
	pub used: u64,
	/// The limit the Provider stated, where it stated one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub limit: Option<u64>,
	/// How long the window lasts, where the Provider stated it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub window_seconds: Option<u64>,
	/// How long until it refills, where the Provider stated it. A Craft
	/// reports the remaining time rather than an instant, so the host
	/// converts it with the Plane's own clock.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub resets_in_seconds: Option<u64>,
	/// Whether the Provider stated the numbers or the Craft derived them.
	pub estimation: CraftUsageEstimation,
	/// Whether the window has closed.
	pub finality: CraftUsageFinality,
}

/// One normalized Usage report (Craft 1.8). Reporting it grants nothing
/// and changes no Run state; it is what the Harness said about what it
/// used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum CraftUsage {
	/// Consumption the Harness reported.
	Observed {
		/// What it consumed.
		observed: CraftObservedUsage,
	},
	/// A quota window the Provider reported.
	Quota {
		/// The window as reported.
		quota: CraftQuotaWindow,
	},
	/// A Provider that would not report its windows, so the host says so
	/// rather than presenting the windows it last saw as current.
	Unreachable {
		/// Bounded, non-secret text naming why, at most 256 characters.
		reason: String,
	},
}
