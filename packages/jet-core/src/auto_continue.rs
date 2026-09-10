//! Bounded Auto-continue after structured quota exhaustion (ADR-0033).
use crate::{
	AccountBindingId, Actor, CommandOutcome, ConversationId, CoreError,
	EventKind, EventSequence, RunId,
};
use jet_store::{ReadTransaction, WriteTransaction};
use serde::{Deserialize, Serialize};

/// Where the user configures Auto-continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoContinueTarget {
	/// Default for this Home Plane's Account binding.
	AccountBinding(AccountBindingId),
	/// One quota-exhaustion episode, consumed when selected.
	Conversation(ConversationId),
}
/// Explicit retry limits; absence of a configured policy resolves to off.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AutoContinuePolicy {
	/// Do not admit automatic input.
	#[default]
	Off,
	/// Retry the same native Conversation and execution selection.
	Retry {
		/// Initial fallback delay, in milliseconds.
		delay_ms: u32,
		/// Maximum exponential fallback delay, in milliseconds.
		max_delay_ms: u32,
		/// Maximum admitted retries in one exhaustion episode.
		max_retries: u32,
		/// Exact continuation input, 1 to 8192 bytes.
		message: String,
	},
}
/// Durable outcome of the most recent retry decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoContinueStatus {
	/// The selected policy disabled this exhaustion episode.
	Disabled,
	/// Queue capacity prevented admission; the retry count has not advanced.
	Deferred,
	/// The Turn queue holds the retry until its due time.
	Pending,
	/// The retry was claimed for native delivery.
	Dispatched,
	/// New user input canceled the retry.
	Canceled,
	/// The configured retry count was reached.
	Exhausted,
}
/// The evidence and policy retained for the latest retry decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoContinueRetry {
	/// Run whose structured quota condition triggered the decision.
	pub run_id: RunId,
	/// Input that encountered this quota condition.
	pub triggering_turn: uuid::Uuid,
	/// Turn admitted by this decision, absent until queue admission succeeds.
	pub retry_turn: Option<uuid::Uuid>,

	/// Provider-reported Usage condition, retained without guessing from text.
	pub usage: crate::QuotaReport,
	/// Plane clock at the observation.
	pub observed_at_unix_ms: i64,
	/// Earliest permitted delivery, respecting the Provider reset time.
	pub due_at_unix_ms: i64,
	/// Number of automatic retries admitted in this episode.
	pub retry_count: u32,
	/// Exact policy selected for this episode, including its message.
	pub policy: AutoContinuePolicy,
	/// Where that policy was selected.
	pub selected_from: AutoContinueTarget,
	/// Latest decision outcome.
	pub status: AutoContinueStatus,
}
/// A fenced policy and retry snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoContinueSnapshot {
	/// Plane Event cursor in the same read transaction.
	pub cursor: EventSequence,
	/// Configured policy, or off when unset.
	pub policy: AutoContinuePolicy,
	/// Latest retry decision for a Conversation.
	pub retry: Option<AutoContinueRetry>,
}

impl AutoContinuePolicy {
	fn validate(&self) -> Result<(), CoreError> {
		// ASVS 2.2.1, 2.3.2: user input cannot authorize unbounded retries.
		if let Self::Retry {
			delay_ms,
			max_delay_ms,
			max_retries,
			message,
		} = self && (*delay_ms == 0
			|| *max_delay_ms < *delay_ms
			|| *max_delay_ms > 86_400_000
			|| !(1..=100).contains(max_retries)
			|| message.trim().is_empty()
			|| message.len() > 8192)
		{
			return Err(CoreError::invalid_input(
				"auto_continue.invalid_policy",
				"retry delays must be 1 to 86400000 milliseconds, the count 1 to 100, and the message 1 to 8192 bytes",
			));
		}
		Ok(())
	}
}

pub(crate) async fn snapshot(
	tx: &mut ReadTransaction,
	target: AutoContinueTarget,
) -> Result<AutoContinueSnapshot, CoreError> {
	let (policy, retry) = match target {
		AutoContinueTarget::AccountBinding(id) => {
			require_binding(tx, id).await?;
			(binding_policy(tx, id).await?, None)
		}
		AutoContinueTarget::Conversation(id) => {
			let queue = crate::turn_queue::load(tx, id).await?;
			(
				queue.auto_continue_override.unwrap_or_default(),
				queue.auto_continue,
			)
		}
	};
	Ok(AutoContinueSnapshot {
		cursor: EventSequence(tx.event_cursor().await?),
		policy,
		retry,
	})
}
pub(crate) async fn binding_policy(
	tx: &mut ReadTransaction,
	id: AccountBindingId,
) -> Result<AutoContinuePolicy, CoreError> {
	tx.auto_continue_policy(id.0)
		.await?
		.map(|json| crate::run_state::decode(&json))
		.transpose()
		.map(Option::unwrap_or_default)
}
async fn require_binding(
	tx: &mut ReadTransaction,
	id: AccountBindingId,
) -> Result<(), CoreError> {
	if tx.account_binding(id.0).await?.is_none() {
		return Err(CoreError::not_found(
			"account.not_found",
			"the Account binding does not exist",
		));
	}
	Ok(())
}
pub(crate) async fn configure(
	tx: &mut WriteTransaction,
	actor: &Actor,
	target: AutoContinueTarget,
	policy: AutoContinuePolicy,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	policy.validate()?;
	let subject = match target {
		AutoContinueTarget::AccountBinding(id) => {
			require_binding(tx, id).await?;
			let json = serde_json::to_string(&policy).map_err(|e| {
				CoreError::internal("auto_continue.encode", e.to_string())
			})?;
			tx.save_auto_continue_policy(id.0, &json).await?;
			crate::event::EventSubject::Plane
		}
		AutoContinueTarget::Conversation(id) => {
			let mut queue = crate::turn_queue::load(tx, id).await?;
			queue.auto_continue_override = Some(policy.clone());
			if policy == AutoContinuePolicy::Off
				&& let Some(mut retry) = queue.auto_continue.clone()
				&& matches!(
					retry.status,
					AutoContinueStatus::Pending | AutoContinueStatus::Deferred
				) {
				let mut kept = Vec::new();
				for mut entry in queue.entries {
					if entry.turn.source == crate::TurnSource::AutoContinue
						&& entry.turn.state == crate::TurnState::Queued
					{
						entry.turn.state = crate::TurnState::Canceled;
						crate::turn_queue::changed(tx, actor, id, &entry, now)
							.await?;
					} else {
						kept.push(entry);
					}
				}
				queue.entries = kept;
				retry.status = AutoContinueStatus::Disabled;
				retry.policy = AutoContinuePolicy::Off;
				retry.selected_from = target;
				queue.auto_continue = Some(retry.clone());
				queue.auto_continue_override = None;
				crate::auto_continue_work::changed(tx, actor, id, retry, now)
					.await?;
			}
			crate::turn_queue::save(tx, id, &queue).await?;
			crate::event::EventSubject::Conversation(id)
		}
	};
	tx.append_event(
		EventKind::AutoContinueConfigured { target, policy }
			.to_record(actor, subject, now)?,
	)
	.await?;
	if let AutoContinueTarget::Conversation(id) = target {
		crate::auto_continue_work::reconsider(tx, id, now).await?;
	}
	crate::audit::record(
		tx,
		actor,
		crate::audit::Decision::succeeded(
			crate::AuditDecision::AutoContinuePolicyChanged,
			crate::audit::auto_continue_subject(target),
		),
		now,
	)
	.await?;
	Ok(CommandOutcome::AutoContinueConfigured)
}
