//! Answering what one Plane knows about Usage, and never more than that
//! (ADR-0023).
//!
//! Three things are reported honestly here rather than folded away. A
//! window the Provider has not confirmed recently is stale; a Provider
//! that would not answer is unreachable; and estimated or still-moving
//! measurements stay counted beside the total they are part of.

use jet_store::{
	ProviderReachRecord, QuotaScopeRecord, QuotaUnitRecord,
	UsageEstimationRecord, UsageFinalityRecord, UsageProviderReachRecord,
	UsageQuotaSnapshotRecord, UsageSelectionRecord, UsageTotalRecord,
};

use crate::usage::{
	ModelConsumption, ModelId, ObservedConsumption, PlaneUsage, QuotaMeasure,
	QuotaScope, QuotaUnit, QuotaWindow, USAGE_FRESHNESS_MS, UsageEstimation,
	UsageFinality, UsageFreshness, UsageSelection, UsageTokens,
};
use crate::{
	AccountBindingId, Core, CoreError, EventSequence, PlaneId, ProviderId,
	QueryResult, system_time,
};

/// Reads the selected Usage records with the journal position that fences
/// them.
///
/// # Errors
///
/// Returns a store category [`CoreError`] when the snapshot cannot be
/// read.
pub(crate) async fn usage(
	core: &Core,
	selection: UsageSelection,
) -> Result<QueryResult, CoreError> {
	let now_unix_ms = core.now_unix_ms();
	core.store
		.read(async |tx| {
			// ASVS 2.3.3: the records and the position that fences them come
			// from one SQLite snapshot.
			let cursor = EventSequence(tx.event_cursor().await?);
			let plane_id = PlaneId(tx.plane().await?.plane_id);
			let binding = match selection {
				UsageSelection::Binding(binding) => Some(binding.0),
				UsageSelection::Plane
				| UsageSelection::Conversation(_)
				| UsageSelection::Run(_) => None,
			};
			let totals = tx.usage_totals(rows(selection)).await?;
			let windows = match selection {
				UsageSelection::Plane | UsageSelection::Binding(_) => {
					tx.usage_quota_windows(binding).await?
				}
				// A quota window belongs to an Account binding rather than
				// to one Conversation, so these selections answer with what
				// was consumed and leave the account's windows to the
				// account.
				UsageSelection::Conversation(_) | UsageSelection::Run(_) => {
					Vec::new()
				}
			};
			let reach = tx.usage_provider_reach(binding).await?;
			Ok(QueryResult::Usage(Box::new(PlaneUsage {
				cursor,
				plane_id,
				quota_windows: windows
					.into_iter()
					.map(|window| quota(window, &reach, now_unix_ms))
					.collect(),
				consumption: consumption(totals),
			})))
		})
		.await
}

/// Which observation rows one selection covers.
fn rows(selection: UsageSelection) -> UsageSelectionRecord {
	match selection {
		UsageSelection::Plane => UsageSelectionRecord::default(),
		UsageSelection::Binding(binding) => UsageSelectionRecord {
			binding_id: Some(binding.0),
			..UsageSelectionRecord::default()
		},
		UsageSelection::Conversation(conversation) => UsageSelectionRecord {
			conversation_id: Some(conversation.0),
			..UsageSelectionRecord::default()
		},
		UsageSelection::Run(run) => UsageSelectionRecord {
			run_id: Some(run.0),
			..UsageSelectionRecord::default()
		},
	}
}

/// One stored window beside the freshness the Plane can claim for it.
fn quota(
	snapshot: UsageQuotaSnapshotRecord,
	reach: &[UsageProviderReachRecord],
	now_unix_ms: i64,
) -> QuotaWindow {
	let freshness = freshness(
		reach
			.iter()
			.find(|record| record.binding_id == snapshot.binding_id),
		snapshot.observed_at_unix_ms,
		now_unix_ms,
	);
	QuotaWindow {
		binding_id: AccountBindingId(snapshot.binding_id),
		provider: ProviderId(snapshot.provider),
		window: snapshot.window_id,
		scope: match (snapshot.scope, snapshot.model) {
			(QuotaScopeRecord::Model, Some(model)) => {
				QuotaScope::Model(ModelId(model))
			}
			(QuotaScopeRecord::ProviderAccount, _)
			| (QuotaScopeRecord::Model, None) => QuotaScope::ProviderAccount,
		},
		measure: QuotaMeasure {
			unit: match snapshot.unit {
				QuotaUnitRecord::Tokens => QuotaUnit::Tokens,
				QuotaUnitRecord::Requests => QuotaUnit::Requests,
				QuotaUnitRecord::Credits => QuotaUnit::Credits,
				QuotaUnitRecord::Share => QuotaUnit::Share,
			},
			used: snapshot.used,
			limit: snapshot.limit_amount,
		},
		window_seconds: snapshot.window_seconds,
		resets_at: snapshot.resets_at_unix_ms.map(system_time),
		estimation: estimation(snapshot.estimation),
		finality: finality(snapshot.finality),
		observed_at: system_time(snapshot.observed_at_unix_ms),
		freshness,
	}
}

/// Whether a window still stands for what the Provider would say now. A
/// Provider that refused after the window was read is reported as
/// unreachable rather than as a current reading (ADR-0023).
fn freshness(
	reach: Option<&UsageProviderReachRecord>,
	observed_at_unix_ms: i64,
	now_unix_ms: i64,
) -> UsageFreshness {
	if let Some(record) = reach
		&& let ProviderReachRecord::Unreachable { reason } = &record.reach
		&& record.observed_at_unix_ms >= observed_at_unix_ms
	{
		return UsageFreshness::Unreachable {
			reason: reason.clone(),
		};
	}
	if now_unix_ms.saturating_sub(observed_at_unix_ms) > USAGE_FRESHNESS_MS {
		UsageFreshness::Stale
	} else {
		UsageFreshness::Fresh
	}
}

/// The per-Model rows folded into one total for the selection, with the
/// uncertainty carried rather than discarded.
fn consumption(totals: Vec<UsageTotalRecord>) -> ObservedConsumption {
	let mut consumption = ObservedConsumption::default();
	for total in totals {
		consumption.tokens = UsageTokens {
			input: consumption.tokens.input.saturating_add(total.tokens.input),
			cached_input: consumption
				.tokens
				.cached_input
				.saturating_add(total.tokens.cached_input),
			output: consumption
				.tokens
				.output
				.saturating_add(total.tokens.output),
			reasoning: consumption
				.tokens
				.reasoning
				.saturating_add(total.tokens.reasoning),
		};
		consumption.measurements =
			consumption.measurements.saturating_add(total.measurements);
		consumption.estimated =
			consumption.estimated.saturating_add(total.estimated);
		consumption.interim = consumption.interim.saturating_add(total.interim);
		consumption.last_observed_at = consumption
			.last_observed_at
			.max(Some(system_time(total.last_observed_at_unix_ms)));
		consumption.models.push(ModelConsumption {
			model: total.model.map(ModelId),
			tokens: UsageTokens {
				input: total.tokens.input,
				cached_input: total.tokens.cached_input,
				output: total.tokens.output,
				reasoning: total.tokens.reasoning,
			},
			measurements: total.measurements,
			estimated: total.estimated,
			interim: total.interim,
			last_observed_at: system_time(total.last_observed_at_unix_ms),
		});
	}
	consumption
}

fn estimation(estimation: UsageEstimationRecord) -> UsageEstimation {
	match estimation {
		UsageEstimationRecord::Measured => UsageEstimation::Measured,
		UsageEstimationRecord::Estimated => UsageEstimation::Estimated,
	}
}

fn finality(finality: UsageFinalityRecord) -> UsageFinality {
	match finality {
		UsageFinalityRecord::Interim => UsageFinality::Interim,
		UsageFinalityRecord::Final => UsageFinality::Final,
	}
}
