//! The Usage half of the translation seam (ADR-0049, ADR-0023). It also
//! carries the Craft protocol's Usage report inward, since that report is
//! a wire vocabulary too and the core never sees one.

use jet_core::{
	AccountBindingId, ConversationId, ModelConsumption, ModelId,
	ObservedConsumption, ObservedUsage, PlaneUsage, QuotaMeasure, QuotaReport,
	QuotaScope, QuotaUnit, QuotaWindow, RunId, UsageEstimation, UsageFinality,
	UsageFreshness, UsageMeasurement, UsageReport, UsageSelection, UsageTokens,
};
use jet_protocol as wire;

use super::unix_ms;

/// What one Usage Query covers.
pub(super) fn selection(selection: wire::UsageSelection) -> UsageSelection {
	match selection {
		wire::UsageSelection::Plane => UsageSelection::Plane,
		wire::UsageSelection::Binding { binding_id } => {
			UsageSelection::Binding(AccountBindingId(binding_id))
		}
		wire::UsageSelection::Conversation { conversation_id } => {
			UsageSelection::Conversation(ConversationId(conversation_id))
		}
		wire::UsageSelection::Run { run_id } => {
			UsageSelection::Run(RunId(run_id))
		}
	}
}

/// One Plane's Usage snapshot as a client reads it.
pub(super) fn plane(usage: PlaneUsage) -> wire::PlaneUsage {
	wire::PlaneUsage {
		cursor: usage.cursor.0,
		plane_id: usage.plane_id.0,
		quota_windows: usage.quota_windows.into_iter().map(window).collect(),
		consumption: consumption(usage.consumption),
	}
}

fn window(window: QuotaWindow) -> wire::QuotaWindow {
	wire::QuotaWindow {
		binding_id: window.binding_id.0,
		provider: window.provider.0,
		window: window.window,
		scope: scope(window.scope),
		conversation_id: window.conversation_id.map(|id| id.0),
		run_id: window.run_id.map(|id| id.0),
		measure: measure(window.measure),
		window_seconds: window.window_seconds,
		resets_at_unix_ms: window.resets_at.map(unix_ms),
		estimation: estimation(window.estimation),
		finality: finality(window.finality),
		observed_at_unix_ms: unix_ms(window.observed_at),
		freshness: match window.freshness {
			UsageFreshness::Fresh => wire::UsageFreshness::Fresh,
			UsageFreshness::Stale => wire::UsageFreshness::Stale,
			UsageFreshness::Unreachable { reason } => {
				wire::UsageFreshness::Unreachable { reason }
			}
		},
	}
}

fn consumption(consumption: ObservedConsumption) -> wire::ObservedConsumption {
	wire::ObservedConsumption {
		tokens: tokens(consumption.tokens),
		measurements: consumption.measurements,
		estimated: consumption.estimated,
		interim: consumption.interim,
		last_observed_at_unix_ms: consumption.last_observed_at.map(unix_ms),
		models: consumption.models.into_iter().map(model).collect(),
	}
}

fn model(model: ModelConsumption) -> wire::ModelConsumption {
	wire::ModelConsumption {
		model: model.model.map(|ModelId(model)| model),
		tokens: tokens(model.tokens),
		measurements: model.measurements,
		estimated: model.estimated,
		interim: model.interim,
		last_observed_at_unix_ms: unix_ms(model.last_observed_at),
	}
}

fn tokens(tokens: UsageTokens) -> wire::UsageTokens {
	wire::UsageTokens {
		input: tokens.input,
		cached_input: tokens.cached_input,
		output: tokens.output,
		reasoning: tokens.reasoning,
	}
}

fn scope(scope: QuotaScope) -> wire::QuotaScope {
	match scope {
		QuotaScope::ProviderAccount => wire::QuotaScope::ProviderAccount,
		QuotaScope::Model(ModelId(model)) => wire::QuotaScope::Model { model },
	}
}

fn measure(measure: QuotaMeasure) -> wire::QuotaMeasure {
	wire::QuotaMeasure {
		unit: match measure.unit {
			QuotaUnit::Tokens => wire::QuotaUnit::Tokens,
			QuotaUnit::Requests => wire::QuotaUnit::Requests,
			QuotaUnit::Credits => wire::QuotaUnit::Credits,
			QuotaUnit::Share => wire::QuotaUnit::Share,
		},
		used: measure.used,
		limit: measure.limit,
	}
}

fn estimation(estimation: UsageEstimation) -> wire::UsageEstimation {
	match estimation {
		UsageEstimation::Measured => wire::UsageEstimation::Measured,
		UsageEstimation::Estimated => wire::UsageEstimation::Estimated,
	}
}

fn finality(finality: UsageFinality) -> wire::UsageFinality {
	match finality {
		UsageFinality::Interim => wire::UsageFinality::Interim,
		UsageFinality::Final => wire::UsageFinality::Final,
	}
}

/// What a Craft reported, in the core's vocabulary. Nothing here is
/// authority: the host decides which Account binding the report belongs
/// to and when the Plane observed it.
pub(crate) fn report(usage: wire::CraftUsage) -> UsageReport {
	match usage {
		wire::CraftUsage::Observed { observed } => {
			UsageReport::Observed(ObservedUsage {
				measurement: match observed.measurement {
					wire::CraftUsageMeasurement::Turn {
						turn,
						native_usage_id,
					} => UsageMeasurement::Turn {
						turn,
						native_usage_id,
					},
					wire::CraftUsageMeasurement::Run { native_usage_id } => {
						UsageMeasurement::Run { native_usage_id }
					}
				},
				model: observed.model.map(ModelId),
				estimation: craft_estimation(observed.estimation),
				finality: craft_finality(observed.finality),
				tokens: UsageTokens {
					input: observed.tokens.input,
					cached_input: observed.tokens.cached_input,
					output: observed.tokens.output,
					reasoning: observed.tokens.reasoning,
				},
			})
		}
		wire::CraftUsage::Quota { quota } => {
			UsageReport::ProviderQuota(QuotaReport {
				window: quota.window,
				scope: match quota.scope {
					wire::CraftQuotaScope::ProviderAccount => {
						QuotaScope::ProviderAccount
					}
					wire::CraftQuotaScope::Model { model } => {
						QuotaScope::Model(ModelId(model))
					}
				},
				measure: QuotaMeasure {
					unit: match quota.unit {
						wire::CraftQuotaUnit::Tokens => QuotaUnit::Tokens,
						wire::CraftQuotaUnit::Requests => QuotaUnit::Requests,
						wire::CraftQuotaUnit::Credits => QuotaUnit::Credits,
						wire::CraftQuotaUnit::Share => QuotaUnit::Share,
					},
					used: quota.used,
					limit: quota.limit,
				},
				window_seconds: quota.window_seconds,
				resets_in_seconds: quota.resets_in_seconds,
				estimation: craft_estimation(quota.estimation),
				finality: craft_finality(quota.finality),
			})
		}
		wire::CraftUsage::Unreachable { reason } => {
			UsageReport::ProviderUnreachable { reason }
		}
	}
}

fn craft_estimation(estimation: wire::CraftUsageEstimation) -> UsageEstimation {
	match estimation {
		wire::CraftUsageEstimation::Measured => UsageEstimation::Measured,
		wire::CraftUsageEstimation::Estimated => UsageEstimation::Estimated,
	}
}

fn craft_finality(finality: wire::CraftUsageFinality) -> UsageFinality {
	match finality {
		wire::CraftUsageFinality::Interim => UsageFinality::Interim,
		wire::CraftUsageFinality::Final => UsageFinality::Final,
	}
}
