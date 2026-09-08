//! Claude Code's own token and rate-limit accounting, in Jet's vocabulary
//! (Craft 1.8, ADR-0023). The complete native event still travels beside
//! every report.

use std::time::{SystemTime, UNIX_EPOCH};

use jet_protocol::{
	CraftObservedUsage, CraftQuotaScope, CraftQuotaUnit, CraftQuotaWindow,
	CraftUsage, CraftUsageEstimation, CraftUsageFinality,
	CraftUsageMeasurement, CraftUsageTokens,
};
use serde_json::Value;

/// A share is hundredths of a percent, so a filled fraction crosses the
/// wire without a floating point.
const SHARE_LIMIT: u64 = 10_000;

/// Claude Code reports one unified limit rather than a window per Model,
/// and names it only by reporting it.
const UNIFIED_WINDOW: &str = "unified";

/// What one native event says about Usage, for the turn the host
/// delivered as `turn`.
///
/// A completed turn carries its own totals, so nothing is read from the
/// assistant messages inside it: those are per-message counts that Jet
/// could neither add up nor replace without guessing.
pub(crate) fn reports(
	event: &Value,
	turn: &str,
	now: SystemTime,
) -> Vec<CraftUsage> {
	if turn.is_empty() {
		return Vec::new();
	}
	match event.get("type").and_then(Value::as_str) {
		Some("result") => consumed(event, turn),
		Some("rate_limit_event") => window(event, now).into_iter().collect(),
		Some(_) | None => Vec::new(),
	}
}

/// The turn's totals, per Model where the Harness broke them down. The
/// two are never both reported: one already covers the other.
fn consumed(event: &Value, turn: &str) -> Vec<CraftUsage> {
	if let Some(models) = event.get("modelUsage").and_then(Value::as_object)
		&& !models.is_empty()
	{
		return models
			.iter()
			.map(|(model, usage)| {
				observed(
					usage,
					turn,
					Some(format!("{turn}:{model}")),
					Some(model.clone()),
				)
			})
			.collect();
	}
	event
		.get("usage")
		.map(|usage| observed(usage, turn, None, None))
		.into_iter()
		.collect()
}

fn observed(
	usage: &Value,
	turn: &str,
	native_usage_id: Option<String>,
	model: Option<String>,
) -> CraftUsage {
	CraftUsage::Observed {
		observed: CraftObservedUsage {
			measurement: CraftUsageMeasurement::Turn {
				turn: turn.to_owned(),
				native_usage_id,
			},
			model,
			estimation: CraftUsageEstimation::Measured,
			// A result event is the turn's own boundary, so its counts no
			// longer move.
			finality: CraftUsageFinality::Final,
			tokens: tokens(usage),
		},
	}
}

/// The unified limit as the Provider filled it. Claude Code states a
/// fraction of the window rather than a countable limit, so it crosses as
/// a share, and states when the window refills as an instant, which the
/// Craft turns back into the remaining time the host expects.
fn window(event: &Value, now: SystemTime) -> Option<CraftUsage> {
	let info = event.get("rate_limit_info")?;
	let used = info.get("utilization").and_then(Value::as_f64)?;
	Some(CraftUsage::Quota {
		quota: CraftQuotaWindow {
			window: UNIFIED_WINDOW.to_owned(),
			scope: CraftQuotaScope::ProviderAccount,
			unit: CraftQuotaUnit::Share,
			used: basis_points(used),
			limit: Some(SHARE_LIMIT),
			window_seconds: None,
			resets_in_seconds: info
				.get("resets_at")
				.and_then(Value::as_u64)
				.and_then(|resets_at| remaining(resets_at, now)),
			estimation: CraftUsageEstimation::Measured,
			finality: CraftUsageFinality::Interim,
		},
	})
}

/// How long is left of a window the Harness dated. A window that has
/// already passed has nothing left rather than a negative time.
fn remaining(resets_at_unix_s: u64, now: SystemTime) -> Option<u64> {
	let now = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
	Some(resets_at_unix_s.saturating_sub(now))
}

/// The counts, under either spelling Claude Code uses for them. Tokens
/// written to the Provider's cache count as input, which is what they
/// were charged as.
fn tokens(usage: &Value) -> CraftUsageTokens {
	CraftUsageTokens {
		input: count(usage, "input_tokens", "inputTokens").saturating_add(
			count(
				usage,
				"cache_creation_input_tokens",
				"cacheCreationInputTokens",
			),
		),
		cached_input: count(
			usage,
			"cache_read_input_tokens",
			"cacheReadInputTokens",
		),
		output: count(usage, "output_tokens", "outputTokens"),
		reasoning: 0,
	}
}

fn count(usage: &Value, field: &str, camel: &str) -> u64 {
	usage
		.get(field)
		.or_else(|| usage.get(camel))
		.and_then(Value::as_u64)
		.unwrap_or_default()
}

/// A filled fraction as hundredths of a percent, bounded by the share a
/// window can be filled to.
fn basis_points(fraction: f64) -> u64 {
	let hundredths = (fraction * 10_000.0).round();
	if hundredths.is_finite() && hundredths > 0.0 {
		(hundredths as u64).min(SHARE_LIMIT)
	} else {
		0
	}
}
