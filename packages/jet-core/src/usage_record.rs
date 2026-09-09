//! Recording what a Craft reports about Usage, without letting one
//! measurement be counted twice (ADR-0023, ADR-0045).
//!
//! Every report arrives inside the Run's own source batch, so a record is
//! committed with the native event it was derived from. What is written
//! depends on what the report can honestly claim: consumption always,
//! quota windows only where the Run named the Account binding they belong
//! to, and an unreachable Provider as itself rather than as an old window
//! presented as current.

use jet_store::{
	NewUsageObservation, ProviderReachRecord, QuotaScopeRecord,
	QuotaUnitRecord, UsageEstimationRecord, UsageFinalityRecord,
	UsageProviderReachRecord, UsageQuotaSnapshotRecord, UsageScopeRecord,
	UsageTokensRecord, WriteTransaction,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::usage::{
	MAX_REASON_TEXT, MAX_USAGE_TEXT, ModelId, ObservedUsage, QuotaMeasure,
	QuotaReport, QuotaScope, QuotaUnit, USAGE_REFRESH_MS, UsageEstimation,
	UsageFinality, UsageMeasurement, UsageReport, UsageSource, UsageTokens,
};
use crate::{
	AccountBindingId, CoreError, EventActor, EventKind, ProviderId, Run,
};

/// What a share is measured against: hundredths of a percent. The wire
/// states the same bound beside its own unit, because a domain type never
/// borrows a wire one (ADR-0049).
const QUOTA_SHARE_LIMIT: u64 = 10_000;

/// Records one Craft Usage report against its Run.
///
/// # Errors
///
/// Returns an `invalid_input` [`CoreError`] when the report is not the
/// bounded metadata a record carries, or a store category when the row
/// cannot be written.
pub(crate) async fn record(
	tx: &mut WriteTransaction,
	actor: &EventActor,
	run: &Run,
	binding: Option<AccountBindingId>,
	report: UsageReport,
	now_unix_ms: i64,
) -> Result<(), CoreError> {
	let recorded = match report {
		UsageReport::Observed(observed) => {
			observe(tx, run, binding, observed, now_unix_ms).await?
		}
		UsageReport::ProviderQuota(quota) => {
			window(tx, run, binding, quota, now_unix_ms).await?
		}
		UsageReport::ProviderUnreachable { reason } => {
			unreachable(tx, binding, reason, now_unix_ms).await?
		}
	};
	if let Some(source) = recorded {
		let event = EventKind::UsageRecorded {
			source,
			binding_id: binding,
		};
		crate::run_state_storage::append(tx, actor, run, event, now_unix_ms)
			.await?;
	}
	Ok(())
}

/// Stores what the Harness said it consumed, under the identity that makes
/// it one measurement of its Run.
async fn observe(
	tx: &mut WriteTransaction,
	run: &Run,
	binding: Option<AccountBindingId>,
	observed: ObservedUsage,
	now_unix_ms: i64,
) -> Result<Option<UsageSource>, CoreError> {
	let ObservedUsage {
		measurement,
		model,
		estimation,
		finality,
		tokens,
	} = observed;
	if let Some(ModelId(model)) = &model {
		require_text("usage.model_unsupported", "a Model name", model)?;
	}
	// Both spellings of a measurement identity are prefixed, so a native
	// identity that happens to read like a turn is still its own record.
	let (scope, key, native_usage_id) = match measurement {
		UsageMeasurement::Turn {
			turn,
			native_usage_id,
		} => {
			require_text("usage.turn_unsupported", "a turn identity", &turn)?;
			require_native(native_usage_id.as_deref())?;
			let key = native_usage_id.as_ref().map_or_else(
				|| format!("turn:{turn}"),
				|native| format!("native:{native}"),
			);
			(UsageScopeRecord::Turn, key, native_usage_id)
		}
		UsageMeasurement::Run { native_usage_id } => {
			require_native(native_usage_id.as_deref())?;
			let key = native_usage_id.as_ref().map_or_else(
				|| "run".to_owned(),
				|native| format!("native:{native}"),
			);
			(UsageScopeRecord::Run, key, native_usage_id)
		}
	};
	let provider = provider_of(tx, binding).await?;
	let stored = tx
		.record_usage_observation(&NewUsageObservation {
			observation_id: Uuid::now_v7(),
			conversation_id: run.conversation_id.0,
			run_id: run.run_id.0,
			measurement: key,
			native_usage_id,
			binding_id: binding.map(|binding| binding.0),
			provider: provider.map(|ProviderId(provider)| provider),
			model: model.map(|ModelId(model)| model),
			scope,
			estimation: estimation_record(estimation),
			finality: finality_record(finality),
			tokens: tokens.into(),
			observed_at_unix_ms: now_unix_ms,
		})
		.await?;
	Ok(stored.then_some(UsageSource::JetObserved))
}

/// Stores one Provider-reported quota window, unless the Provider already
/// said the same thing recently enough for this response to be a
/// heartbeat.
async fn window(
	tx: &mut WriteTransaction,
	run: &Run,
	binding: Option<AccountBindingId>,
	mut report: QuotaReport,
	now_unix_ms: i64,
) -> Result<Option<UsageSource>, CoreError> {
	report.window_seconds = window_length(report.window_seconds);
	require_text(
		"usage.window_unsupported",
		"a Provider window name",
		&report.window,
	)?;
	if let QuotaScope::Model(ModelId(model)) = &report.scope {
		require_text("usage.model_unsupported", "a Model name", model)?;
	}
	let measure = require_measure(report.measure)?;
	// A quota window is an account-level fact. A Run that selected no
	// Account binding has no account on this Plane to attach it to, and
	// attaching it to a guess is the manufactured total ADR-0023 forbids,
	// so the window is left unrecorded rather than misattributed.
	let (Some(binding_id), Some(provider)) =
		(binding, provider_of(tx, binding).await?)
	else {
		return Ok(None);
	};
	// The Provider answered, whatever it said, so the binding is reachable
	// again even when the answer repeats one already stored.
	tx.record_usage_provider_reach(&UsageProviderReachRecord {
		binding_id: binding_id.0,
		reach: ProviderReachRecord::Reachable,
		observed_at_unix_ms: now_unix_ms,
	})
	.await?;
	let digest = content_digest(&provider, &report, measure);
	let model = match &report.scope {
		QuotaScope::ProviderAccount => None,
		QuotaScope::Model(ModelId(model)) => Some(model.clone()),
	};
	if let Some(previous) = tx
		.usage_quota_heartbeat(binding_id.0, &report.window, model.as_deref())
		.await?
		&& previous.digest == digest
		&& now_unix_ms.saturating_sub(previous.observed_at_unix_ms)
			< USAGE_REFRESH_MS
	{
		// The Provider said the same thing again. That is not a new
		// snapshot, but it is an answer, and it answers about this window
		// alone: the other windows of this binding were not confirmed by
		// it and go stale on their own.
		tx.record_usage_quota_answer(previous.snapshot_id, now_unix_ms)
			.await?;
		return Ok(None);
	}
	let scope = match report.scope {
		QuotaScope::ProviderAccount => QuotaScopeRecord::ProviderAccount,
		QuotaScope::Model(_) => QuotaScopeRecord::Model,
	};
	tx.record_usage_quota_snapshot(&UsageQuotaSnapshotRecord {
		snapshot_id: Uuid::now_v7(),
		binding_id: binding_id.0,
		provider: provider.0,
		window_id: report.window,
		scope,
		model,
		conversation_id: Some(run.conversation_id.0),
		run_id: Some(run.run_id.0),
		unit: unit(measure.unit),
		used: measure.used,
		limit_amount: measure.limit,
		window_seconds: report.window_seconds,
		resets_at_unix_ms: report
			.resets_in_seconds
			.and_then(|seconds| seconds.checked_mul(1_000))
			.and_then(|elapsed| i64::try_from(elapsed).ok())
			.map(|elapsed| now_unix_ms.saturating_add(elapsed)),
		estimation: estimation_record(report.estimation),
		finality: finality_record(report.finality),
		observed_at_unix_ms: now_unix_ms,
		answered_at_unix_ms: now_unix_ms,
		digest,
	})
	.await?;
	Ok(Some(UsageSource::ProviderQuota))
}

/// Records that the Provider would not answer, so the windows it last
/// reported are shown as unreachable rather than as current.
async fn unreachable(
	tx: &mut WriteTransaction,
	binding: Option<AccountBindingId>,
	reason: String,
	now_unix_ms: i64,
) -> Result<Option<UsageSource>, CoreError> {
	let supported = !reason.trim().is_empty()
		&& reason.chars().count() <= MAX_REASON_TEXT
		&& !reason.chars().any(char::is_control);
	if !supported {
		return Err(CoreError::invalid_input(
			"usage.reason_unsupported",
			format!(
				"an unreachable Provider reason is between one and \
				 {MAX_REASON_TEXT} characters and holds no control characters"
			),
		));
	}
	let Some(binding_id) = binding else {
		return Ok(None);
	};
	tx.record_usage_provider_reach(&UsageProviderReachRecord {
		binding_id: binding_id.0,
		reach: ProviderReachRecord::Unreachable { reason },
		observed_at_unix_ms: now_unix_ms,
	})
	.await?;
	Ok(Some(UsageSource::ProviderUnreachable))
}

/// The Provider one binding authenticates to, absent when the binding was
/// forgotten while its Run was still reporting.
async fn provider_of(
	tx: &mut WriteTransaction,
	binding: Option<AccountBindingId>,
) -> Result<Option<ProviderId>, CoreError> {
	let Some(binding) = binding else {
		return Ok(None);
	};
	Ok(tx
		.account_binding(binding.0)
		.await?
		.map(|record| ProviderId(record.provider)))
}

/// Digest of what the Provider said about consumption. The countdown to
/// the window's reset moves on its own, so it is deliberately left out: a
/// snapshot is a new one when the reported fill changes (ADR-0045).
fn content_digest(
	provider: &ProviderId,
	report: &QuotaReport,
	measure: QuotaMeasure,
) -> [u8; 32] {
	let mut hash = Sha256::new();
	hash.update(provider.0.as_bytes());
	hash.update([0]);
	hash.update(report.window.as_bytes());
	hash.update([0]);
	match &report.scope {
		QuotaScope::ProviderAccount => hash.update([0]),
		QuotaScope::Model(ModelId(model)) => {
			hash.update([1]);
			hash.update(model.as_bytes());
		}
	}
	hash.update([unit_byte(measure.unit)]);
	hash.update(measure.used.to_be_bytes());
	hash.update(measure.limit.unwrap_or_default().to_be_bytes());
	hash.update(report.window_seconds.unwrap_or_default().to_be_bytes());
	hash.update([u8::from(report.estimation == UsageEstimation::Estimated)]);
	hash.update([u8::from(report.finality == UsageFinality::Final)]);
	hash.finalize().into()
}

/// How long the window lasts, as a length a record can carry. A Provider
/// that states zero seconds, or a number no clock could mean, has stated
/// no length, and is recorded as having stated none. The Craft protocol
/// allows it to send either, so refusing one here would fail the whole
/// source batch the report arrived in and take its Run down with it.
fn window_length(seconds: Option<u64>) -> Option<u64> {
	seconds.filter(|seconds| *seconds > 0 && i64::try_from(*seconds).is_ok())
}

/// Refuses a measure a Provider cannot have reported, and fixes the fixed
/// part of a share so one is stored the same way whatever a Craft sends.
fn require_measure(measure: QuotaMeasure) -> Result<QuotaMeasure, CoreError> {
	if measure.unit != QuotaUnit::Share {
		return Ok(measure);
	}
	if measure.used > QUOTA_SHARE_LIMIT {
		return Err(CoreError::invalid_input(
			"usage.share_unsupported",
			format!(
				"a reported share is between 0 and {QUOTA_SHARE_LIMIT} hundredths \
				 of a percent"
			),
		));
	}
	Ok(QuotaMeasure {
		limit: Some(QUOTA_SHARE_LIMIT),
		..measure
	})
}

fn require_native(native_usage_id: Option<&str>) -> Result<(), CoreError> {
	match native_usage_id {
		Some(native) => require_text(
			"usage.native_identity_unsupported",
			"a native usage identity",
			native,
		),
		None => Ok(()),
	}
}

/// Refuses record metadata that is empty, too long, or carries control
/// characters, which no Provider or Harness identity does.
fn require_text(
	code: &'static str,
	described: &str,
	value: &str,
) -> Result<(), CoreError> {
	let supported = !value.trim().is_empty()
		&& value.chars().count() <= MAX_USAGE_TEXT
		&& !value.chars().any(char::is_control);
	if supported {
		return Ok(());
	}
	Err(CoreError::invalid_input(
		code,
		format!(
			"{described} is between one and {MAX_USAGE_TEXT} characters and \
			 holds no control characters"
		),
	))
}

fn estimation_record(estimation: UsageEstimation) -> UsageEstimationRecord {
	match estimation {
		UsageEstimation::Measured => UsageEstimationRecord::Measured,
		UsageEstimation::Estimated => UsageEstimationRecord::Estimated,
	}
}

fn finality_record(finality: UsageFinality) -> UsageFinalityRecord {
	match finality {
		UsageFinality::Interim => UsageFinalityRecord::Interim,
		UsageFinality::Final => UsageFinalityRecord::Final,
	}
}

/// The unit's place in the digest, so one response hashes the same way
/// every time it is reported.
fn unit_byte(unit: QuotaUnit) -> u8 {
	match unit {
		QuotaUnit::Tokens => 0,
		QuotaUnit::Requests => 1,
		QuotaUnit::Credits => 2,
		QuotaUnit::Share => 3,
	}
}

fn unit(unit: QuotaUnit) -> QuotaUnitRecord {
	match unit {
		QuotaUnit::Tokens => QuotaUnitRecord::Tokens,
		QuotaUnit::Requests => QuotaUnitRecord::Requests,
		QuotaUnit::Credits => QuotaUnitRecord::Credits,
		QuotaUnit::Share => QuotaUnitRecord::Share,
	}
}

/// The token counts as the store keeps them.
impl From<UsageTokens> for UsageTokensRecord {
	fn from(tokens: UsageTokens) -> Self {
		Self {
			input: tokens.input,
			cached_input: tokens.cached_input,
			output: tokens.output,
			reasoning: tokens.reasoning,
		}
	}
}
