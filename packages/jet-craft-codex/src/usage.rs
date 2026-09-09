//! Codex's own token and rate-limit accounting, in Jet's vocabulary
//! (Craft 1.7, ADR-0023). The complete native event still travels beside
//! every report.

use jet_protocol::{
	CraftObservedUsage, CraftQuotaScope, CraftQuotaUnit, CraftQuotaWindow,
	CraftUsage, CraftUsageEstimation, CraftUsageFinality,
	CraftUsageMeasurement, CraftUsageTokens, QUOTA_SHARE_LIMIT,
};
use serde_json::Value;

/// What one `thread/tokenUsage/updated` notification says about `turn`.
///
/// Only the last turn's counts are reported. Codex's cumulative total
/// covers its whole thread, which a resumed Conversation continues, so
/// reporting it as this Run's total would count earlier Runs again.
pub(crate) fn reports(
	value: &Value,
	turn: &str,
	finality: CraftUsageFinality,
) -> Vec<CraftUsage> {
	if turn.is_empty() {
		return Vec::new();
	}
	let mut reports = Vec::new();
	if let Some(last) = value.pointer("/params/tokenUsage/last") {
		reports.push(CraftUsage::Observed {
			observed: CraftObservedUsage {
				measurement: CraftUsageMeasurement::Turn {
					turn: turn.to_owned(),
					native_usage_id: text(value, "/params/turnId"),
				},
				model: text(value, "/params/model"),
				estimation: CraftUsageEstimation::Measured,
				finality,
				tokens: tokens(last),
			},
		});
	}
	for (name, pointer) in [
		("primary", "/params/rateLimits/primary"),
		("secondary", "/params/rateLimits/secondary"),
	] {
		if let Some(limit) = value.pointer(pointer)
			&& let Some(window) = window(name, limit)
		{
			reports.push(window);
		}
	}
	reports
}

/// One rate-limit window as the Provider filled it. Codex states a
/// percentage rather than a countable limit, so it crosses as a share.
fn window(name: &str, value: &Value) -> Option<CraftUsage> {
	let used = value.get("usedPercent").and_then(Value::as_f64)?;
	Some(CraftUsage::Quota {
		quota: CraftQuotaWindow {
			window: name.to_owned(),
			scope: CraftQuotaScope::ProviderAccount,
			unit: CraftQuotaUnit::Share,
			used: basis_points(used),
			limit: Some(QUOTA_SHARE_LIMIT),
			window_seconds: value
				.get("windowMinutes")
				.and_then(Value::as_u64)
				.and_then(|minutes| minutes.checked_mul(60)),
			resets_in_seconds: value
				.get("resetsInSeconds")
				.and_then(Value::as_u64),
			estimation: CraftUsageEstimation::Measured,
			finality: CraftUsageFinality::Interim,
		},
	})
}

fn tokens(value: &Value) -> CraftUsageTokens {
	CraftUsageTokens {
		input: count(value, "inputTokens"),
		cached_input: count(value, "cachedInputTokens"),
		output: count(value, "outputTokens"),
		reasoning: count(value, "reasoningOutputTokens"),
	}
}

fn count(value: &Value, field: &str) -> u64 {
	value.get(field).and_then(Value::as_u64).unwrap_or_default()
}

fn text(value: &Value, pointer: &str) -> Option<String> {
	value
		.pointer(pointer)
		.and_then(Value::as_str)
		.filter(|text| !text.is_empty())
		.map(str::to_owned)
}

/// A Provider percentage as hundredths of a percent, bounded by the share
/// a window can be filled to.
fn basis_points(percent: f64) -> u64 {
	let hundredths = (percent * 100.0).round();
	if hundredths.is_finite() && hundredths > 0.0 {
		(hundredths as u64).min(QUOTA_SHARE_LIMIT)
	} else {
		0
	}
}
