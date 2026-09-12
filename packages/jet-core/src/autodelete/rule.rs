//! Reading a rule: its stored row joined with the Utility job's answer.

use super::{
	AutodeleteRule, AutodeleteRuleId, AutodeleteRuleState, AutodeleteScope,
};
use crate::{CoreError, UtilityOutcome, retention::DAY_MS};
use jet_store::{AutodeleteRuleRecord, ReadTransaction};

/// The interpretation an approved rule executes, if the row is approved.
pub(super) fn approved_inactive_days(
	record: &AutodeleteRuleRecord,
) -> Option<u32> {
	record.approved_at_unix_ms.and(record.inactive_days)
}

/// The instant a Conversation must have been idle since to match
/// `inactive_days` at `now`.
pub(super) fn cutoff(now: i64, inactive_days: u32) -> i64 {
	now.saturating_sub(i64::from(inactive_days).saturating_mul(DAY_MS))
}

/// When the next Conversation will first match an approved rule, if any
/// rule is approved and any Conversation is not yet idle long enough.
/// Conversations already past a rule's cutoff are the sweep's to judge
/// and set no deadline.
pub(crate) async fn next_deadline(
	tx: &mut ReadTransaction,
	now: i64,
) -> Result<Option<i64>, CoreError> {
	let mut soonest = None;
	for record in tx.autodelete_rules().await? {
		let Some(days) = approved_inactive_days(&record) else {
			continue;
		};
		if let Some(idle_since) =
			tx.next_inactivity_after(cutoff(now, days)).await?
		{
			let due = idle_since
				.saturating_add(i64::from(days).saturating_mul(DAY_MS));
			soonest = Some(soonest.map_or(due, |s: i64| s.min(due)));
		}
	}
	Ok(soonest)
}

/// The rule `record` describes. An interpretation the owner edited or
/// approved is the row's; otherwise the draft is whatever the compiling
/// Utility job produced, and a job that produced nothing usable leaves the
/// rule refused (ADR-0099).
pub(super) async fn load(
	tx: &mut ReadTransaction,
	record: AutodeleteRuleRecord,
) -> Result<AutodeleteRule, CoreError> {
	let state = match (record.approved_at_unix_ms, record.inactive_days) {
		(Some(at), Some(inactive_days)) => AutodeleteRuleState::Approved {
			inactive_days,
			approved_at: crate::system_time(at),
		},
		(None, Some(inactive_days)) => {
			AutodeleteRuleState::Draft { inactive_days }
		}
		(Some(_), None) => {
			return Err(CoreError::internal(
				"autodelete.invalid_state",
				"an approved rule has no interpretation",
			));
		}
		(None, None) => compiled(tx, record.utility_job_id).await?,
	};
	Ok(AutodeleteRule {
		rule_id: AutodeleteRuleId(record.rule_id),
		prompt: record.prompt,
		utility_job_id: record.utility_job_id,
		state,
		scope: if record.everywhere {
			AutodeleteScope::Everywhere
		} else {
			AutodeleteScope::Forget
		},
		created_at: crate::system_time(record.created_at_unix_ms),
		updated_at: crate::system_time(record.updated_at_unix_ms),
	})
}

/// What the Utility job made of the source. Only a validated draft becomes
/// one; a text answer is not this purpose's schema and is refused like any
/// other out-of-schema output.
async fn compiled(
	tx: &mut ReadTransaction,
	job_id: uuid::Uuid,
) -> Result<AutodeleteRuleState, CoreError> {
	let job = crate::utility::work::query(tx, job_id).await?;
	Ok(match job.outcome {
		UtilityOutcome::Pending => AutodeleteRuleState::Compiling,
		UtilityOutcome::Draft { inactive_days } => {
			AutodeleteRuleState::Draft { inactive_days }
		}
		UtilityOutcome::Refused { reason } => {
			AutodeleteRuleState::Refused { reason }
		}
		UtilityOutcome::Text { .. } => AutodeleteRuleState::Refused {
			reason: "utility.output_invalid".into(),
		},
	})
}

/// The interpretation a rule offers for approval or executes, if it has
/// one.
pub(super) fn inactive_days(state: &AutodeleteRuleState) -> Option<u32> {
	match state {
		AutodeleteRuleState::Draft { inactive_days }
		| AutodeleteRuleState::Approved { inactive_days, .. } => Some(*inactive_days),
		AutodeleteRuleState::Compiling
		| AutodeleteRuleState::Refused { .. } => None,
	}
}
