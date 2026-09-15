//! Recording what a Craft reports about Usage, without letting one
//! measurement be counted twice (ADR-0023, ADR-0045).
//!
//! Every report arrives inside the Run's own source batch, so a record is
//! committed with the native event it was derived from. What is written
//! depends on what the report can honestly claim: consumption always,
//! quota windows only where the Run named the Account binding they belong
//! to, and an unreachable Provider as itself rather than as an old window
//! presented as current.
//!
//! A report is checked before anything is written. A Usage record is
//! durable Plane state of its own rather than execution state, and the
//! complete native event stays in the Event journal whatever happens to
//! the normalized record, so a report the Plane cannot record is refused
//! on its own: its batch, and its Run, go on, and the refusal is left in
//! the Diagnostic log (ADR-0023, ADR-0061). Only a report already admitted
//! reaches the store, so what the store refuses is its own integrity
//! failing, never a Craft's content, and that still fails the batch.

use crate::{
	AccountBindingId, CoreError, EventActor, EventKind, ProviderId, Run, RunId,
	usage::{
		MAX_REASON_TEXT, MAX_USAGE_TEXT, ModelId, ObservedUsage, QuotaMeasure,
		QuotaReport, QuotaScope, QuotaUnit, USAGE_REFRESH_MS, UsageEstimation,
		UsageFinality, UsageMeasurement, UsageReport, UsageSource, UsageTokens,
	},
};
use jet_runtime::{Diagnostic, DiagnosticComponent};
use jet_store::{
	NewUsageObservation, ProviderReachRecord, QuotaScopeRecord,
	QuotaUnitRecord, UsageEstimationRecord, UsageFinalityRecord,
	UsageProviderReachRecord, UsageQuotaSnapshotRecord, UsageScopeRecord,
	UsageTokensRecord, WriteTransaction,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// What a share is measured against: hundredths of a percent. The wire
/// states the same bound beside its own unit, because a domain type never
/// borrows a wire one (ADR-0049).
const QUOTA_SHARE_LIMIT: u64 = 10_000;

/// A report the Plane can record: bounded metadata, amounts the store
/// holds, and a share within its fixed limit, with the parts a Craft may
/// spell differently already fixed. Only one of these reaches the store.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AdmittedReport(UsageReport);

impl AdmittedReport {
	pub(crate) fn report(&self) -> &UsageReport {
		&self.0
	}
}

/// Checks that a Craft's report is one the Plane can record, and fixes
/// the parts of it that are fixed: a share always has its limit, and a
/// window length no clock could mean is no length. Nothing is written.
///
/// # Errors
///
/// Returns an `invalid_input` [`CoreError`] that names what was refused
/// and repeats nothing of the report: metadata that is not bounded, an
/// amount above what the store holds, or a share above its fixed limit.
pub(crate) fn admit(report: UsageReport) -> Result<AdmittedReport, CoreError> {
	let report = match report {
		UsageReport::Observed(observed) => {
			if let Some(ModelId(model)) = &observed.model {
				require_text("usage.model_unsupported", "a Model name", model)?;
			}
			match &observed.measurement {
				UsageMeasurement::Turn {
					turn,
					native_usage_id,
				} => {
					require_text(
						"usage.turn_unsupported",
						"a turn identity",
						turn,
					)?;
					require_native(native_usage_id.as_deref())?;
				}
				UsageMeasurement::Run { native_usage_id } => {
					require_native(native_usage_id.as_deref())?;
				}
			}
			let UsageTokens {
				input,
				cached_input,
				output,
				reasoning,
			} = observed.tokens;
			require_amount("an input token count", input)?;
			require_amount("a cached input token count", cached_input)?;
			require_amount("an output token count", output)?;
			require_amount("a reasoning token count", reasoning)?;
			UsageReport::Observed(observed)
		}
		UsageReport::ProviderQuota(mut quota) => {
			quota.window_seconds = window_length(quota.window_seconds);
			require_text(
				"usage.window_unsupported",
				"a Provider window name",
				&quota.window,
			)?;
			if let QuotaScope::Model(ModelId(model)) = &quota.scope {
				require_text("usage.model_unsupported", "a Model name", model)?;
			}
			quota.measure = require_measure(quota.measure)?;
			require_amount("a used amount", quota.measure.used)?;
			if let Some(limit) = quota.measure.limit {
				require_amount("a stated limit", limit)?;
			}
			UsageReport::ProviderQuota(quota)
		}
		UsageReport::ProviderUnreachable { reason } => {
			require_bounded(
				"usage.reason_unsupported",
				"an unreachable Provider reason",
				MAX_REASON_TEXT,
				&reason,
			)?;
			UsageReport::ProviderUnreachable { reason }
		}
	};
	Ok(AdmittedReport(report))
}

/// The Diagnostic record of a refused report: the Run it came from, the
/// stable code of what was refused, and the refusal's own constant words.
/// Nothing of the report itself (ADR-0061).
pub(crate) fn refusal(run_id: RunId, refused: &CoreError) -> Diagnostic {
	Diagnostic::warn(DiagnosticComponent::Run, "Usage report refused")
		.code(&refused.code)
		.identity("run_id", &run_id.0)
		.failure(refused)
}

/// Records one admitted Craft Usage report against its Run.
///
/// # Errors
///
/// Returns a store category when the row cannot be written.
pub(crate) async fn record(
	tx: &mut WriteTransaction,
	actor: &EventActor,
	run: &Run,
	binding: Option<AccountBindingId>,
	report: AdmittedReport,
	now_unix_ms: i64,
) -> Result<(), CoreError> {
	let recorded = match report.0 {
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
		crate::run::state_storage::append(tx, actor, run, event, now_unix_ms)
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
	// Both spellings of a measurement identity are prefixed, so a native
	// identity that happens to read like a turn is still its own record.
	let (scope, key, native_usage_id) = match measurement {
		UsageMeasurement::Turn {
			turn,
			native_usage_id,
		} => {
			let key = native_usage_id.as_ref().map_or_else(
				|| format!("turn:{turn}"),
				|native| format!("native:{native}"),
			);
			(UsageScopeRecord::Turn, key, native_usage_id)
		}
		UsageMeasurement::Run { native_usage_id } => {
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
	report: QuotaReport,
	now_unix_ms: i64,
) -> Result<Option<UsageSource>, CoreError> {
	let measure = report.measure;
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
	let digest = content_digest(&provider, &report);
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
fn content_digest(provider: &ProviderId, report: &QuotaReport) -> [u8; 32] {
	let measure = report.measure;
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
/// allows it to send either.
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

/// Refuses an amount the store cannot hold. The wire carries it, so it is
/// refused here as the Craft's content rather than reaching the store,
/// whose refusal of the same number is an integrity failure of its own.
fn require_amount(described: &str, amount: u64) -> Result<(), CoreError> {
	if i64::try_from(amount).is_ok() {
		return Ok(());
	}
	Err(CoreError::invalid_input(
		"usage.amount_unsupported",
		format!("{described} is at most {}", i64::MAX),
	))
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
	require_bounded(code, described, MAX_USAGE_TEXT, value)
}

/// Refuses text that is empty, longer than `limit` characters, or carries
/// control characters. The refusal states the bound and never the text.
fn require_bounded(
	code: &'static str,
	described: &str,
	limit: usize,
	value: &str,
) -> Result<(), CoreError> {
	let supported = !value.trim().is_empty()
		&& value.chars().count() <= limit
		&& !value.chars().any(char::is_control);
	if supported {
		return Ok(());
	}
	Err(CoreError::invalid_input(
		code,
		format!(
			"{described} is between one and {limit} characters and holds no \
			 control characters"
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

#[cfg(test)]
mod tests {
	//! What a record refuses, it refuses before anything is written, so a
	//! refusal is always the Craft's content and never the store's
	//! integrity.
	use super::*;
	use pretty_assertions::assert_eq;

	/// A count the wire carries and the store does not.
	const OVER_STORE: u64 = i64::MAX as u64 + 1;

	fn observed(
		measurement: UsageMeasurement,
		tokens: UsageTokens,
	) -> UsageReport {
		UsageReport::Observed(ObservedUsage {
			measurement,
			model: Some(ModelId("claude-opus-5".into())),
			estimation: UsageEstimation::Measured,
			finality: UsageFinality::Final,
			tokens,
		})
	}

	fn turn(turn: &str) -> UsageMeasurement {
		UsageMeasurement::Turn {
			turn: turn.into(),
			native_usage_id: None,
		}
	}

	fn window(measure: QuotaMeasure) -> QuotaReport {
		QuotaReport {
			window: "five_hour".into(),
			scope: QuotaScope::ProviderAccount,
			measure,
			window_seconds: Some(18_000),
			resets_in_seconds: Some(3_600),
			estimation: UsageEstimation::Measured,
			finality: UsageFinality::Interim,
		}
	}

	fn measure(unit: QuotaUnit, used: u64, limit: Option<u64>) -> QuotaMeasure {
		QuotaMeasure { unit, used, limit }
	}

	fn refused(report: UsageReport) -> String {
		admit(report).expect_err("refused").code
	}

	#[test]
	fn an_amount_above_what_the_store_holds_is_refused() {
		let over = UsageTokens {
			reasoning: OVER_STORE,
			..UsageTokens::default()
		};
		assert_eq!(
			[
				refused(observed(turn("turn-1"), over)),
				refused(UsageReport::ProviderQuota(window(measure(
					QuotaUnit::Tokens,
					OVER_STORE,
					None
				)))),
				refused(UsageReport::ProviderQuota(window(measure(
					QuotaUnit::Tokens,
					1,
					Some(OVER_STORE)
				)))),
			],
			["usage.amount_unsupported"; 3]
		);
	}

	#[test]
	fn a_share_above_its_fixed_limit_is_refused() {
		assert_eq!(
			refused(UsageReport::ProviderQuota(window(measure(
				QuotaUnit::Share,
				QUOTA_SHARE_LIMIT + 1,
				None
			)))),
			"usage.share_unsupported"
		);
	}

	#[test]
	fn metadata_that_is_not_bounded_is_refused() {
		let counted = UsageTokens {
			input: 1,
			..UsageTokens::default()
		};
		let mut named = window(measure(QuotaUnit::Share, 1, None));
		named.window = String::new();
		let mut modelled = window(measure(QuotaUnit::Share, 1, None));
		modelled.scope = QuotaScope::Model(ModelId("claude\u{7}".into()));
		assert_eq!(
			[
				refused(observed(turn(&"t".repeat(129)), counted)),
				refused(observed(
					UsageMeasurement::Run {
						native_usage_id: Some(" ".into()),
					},
					counted
				)),
				refused(UsageReport::ProviderQuota(named)),
				refused(UsageReport::ProviderQuota(modelled)),
				refused(UsageReport::ProviderUnreachable {
					reason: "r".repeat(257),
				}),
			],
			[
				"usage.turn_unsupported",
				"usage.native_identity_unsupported",
				"usage.window_unsupported",
				"usage.model_unsupported",
				"usage.reason_unsupported",
			]
		);
	}

	/// A refusal is left in the Diagnostic log, so it says what was refused
	/// and never what was sent: the report's own text and numbers appear
	/// in neither the code nor the message (ADR-0061).
	#[test]
	fn a_refusal_repeats_nothing_of_the_report() {
		let sent = "PAYLOAD-4242\u{7}";
		let mut modelled =
			window(measure(QuotaUnit::Share, QUOTA_SHARE_LIMIT + 4242, None));
		modelled.scope = QuotaScope::Model(ModelId(sent.into()));
		let refusals = [
			observed(turn(sent), UsageTokens::default()),
			observed(
				turn("turn-1"),
				UsageTokens {
					output: OVER_STORE + 4242,
					..UsageTokens::default()
				},
			),
			UsageReport::ProviderQuota(modelled),
			UsageReport::ProviderUnreachable {
				reason: sent.into(),
			},
		]
		.map(|report| admit(report).expect_err("refused"))
		.map(|refused| format!("{refused} {:?}", refused.detail));
		let repeated = refusals
			.iter()
			.filter(|words| words.contains("PAYLOAD") || words.contains("4242"))
			.count();
		assert_eq!((repeated, refusals.len()), (0, 4));
	}

	/// The parts of a report a Craft may spell differently are fixed on
	/// the way in, so one measurement is stored the same way whatever it
	/// sent: a share always has its fixed limit, and a window length no
	/// clock could mean is no length.
	#[test]
	fn an_admitted_report_carries_its_fixed_parts() {
		let mut reported = window(measure(QuotaUnit::Share, 2_500, None));
		reported.window_seconds = Some(0);
		let mut fixed =
			window(measure(QuotaUnit::Share, 2_500, Some(QUOTA_SHARE_LIMIT)));
		fixed.window_seconds = None;
		assert_eq!(
			admit(UsageReport::ProviderQuota(reported))
				.unwrap()
				.report(),
			&UsageReport::ProviderQuota(fixed)
		);
	}
}
