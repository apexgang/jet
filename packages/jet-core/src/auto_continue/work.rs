//! Quota evidence, retry admission, and dispatch share the queue transaction.
use crate::{
	Actor, AutoContinuePolicy, AutoContinueRetry, AutoContinueStatus,
	AutoContinueTarget, CommandId, ConversationId, CoreError, EventActor,
	EventKind, LaunchPlan, RunId, TurnSource, run::state as run_state,
	turn::queue as turn_queue,
};
use jet_store::WriteTransaction;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Condition {
	pub(crate) usage: crate::QuotaReport,
	pub(crate) observed_at: i64,
	pub(crate) turn_id: uuid::Uuid,
}

pub(crate) fn exhausted(usage: &crate::QuotaReport) -> bool {
	let limit = if usage.measure.unit == crate::QuotaUnit::Share {
		Some(10_000)
	} else {
		usage.measure.limit
	};
	usage.estimation == crate::UsageEstimation::Measured
		&& usage.finality == crate::UsageFinality::Interim
		&& limit.is_some_and(|limit| limit > 0 && usage.measure.used >= limit)
}

pub(crate) async fn consider(
	tx: &mut WriteTransaction,
	run_id: RunId,
	now: i64,
) -> Result<(), CoreError> {
	let execution = tx
		.run_execution(run_id.0)
		.await?
		.expect("observed execution");
	let state: run_state::State = run_state::decode(&execution.state)?;
	if state.partial_source.count != 0
		|| state.disconnected
		|| state.control.is_some()
	{
		return Ok(());
	}
	let plan: LaunchPlan = run_state::decode(&execution.plan)?;
	let run = tx.run(run_id.0).await?.expect("observed Run");
	let id = ConversationId(run.conversation_id);
	let mut queue = turn_queue::load(tx, id).await?;
	if !state.quota_wait {
		if queue
			.auto_continue
			.as_ref()
			.is_some_and(|retry| retry.status == AutoContinueStatus::Pending)
		{
			return Ok(());
		}
		if queue.quota_until.take().is_some() {
			turn_queue::save(tx, id, &queue).await?;
		}
		return Ok(());
	}
	let Some(condition) = state
		.quota
		.iter()
		.filter(|c| exhausted(&c.usage))
		.max_by_key(|c| reset_at(c).unwrap_or(c.observed_at))
		.cloned()
	else {
		return Ok(());
	};
	if queue.auto_continue.as_ref().is_some_and(|previous| {
		previous.triggering_turn == condition.turn_id
			&& previous.status != AutoContinueStatus::Deferred
	}) && queue.auto_continue_override.is_none()
	{
		return refresh_pending(
			tx,
			id,
			&mut queue,
			&state.quota,
			plan.client_id,
			now,
		)
		.await;
	}
	let Some(binding) = plan.visa else {
		return Ok(());
	};
	// ASVS 2.3.1, 15.4.2: authority and policy are re-read inside admission.
	if tx
		.account_binding(binding.account_binding_id.0)
		.await?
		.is_none()
	{
		return Ok(());
	}
	let (policy, selected_from, count) =
		if let Some(policy) = queue.auto_continue_override.clone() {
			(policy, AutoContinueTarget::Conversation(id), 0)
		} else if let Some(previous) = &queue.auto_continue
			&& (previous.retry_turn == Some(condition.turn_id)
				|| (previous.triggering_turn == condition.turn_id
					&& previous.status == AutoContinueStatus::Deferred))
			&& !matches!(
				previous.status,
				AutoContinueStatus::Canceled | AutoContinueStatus::Disabled
			) {
			(
				previous.policy.clone(),
				previous.selected_from,
				previous.retry_count,
			)
		} else {
			(
				crate::auto_continue::binding_policy(
					tx,
					binding.account_binding_id,
				)
				.await?,
				AutoContinueTarget::AccountBinding(binding.account_binding_id),
				0,
			)
		};
	let reset = reset_at(&condition);
	let AutoContinuePolicy::Retry {
		delay_ms,
		max_delay_ms,
		max_retries,
		ref message,
	} = policy
	else {
		queue.quota_until = Some(reset.unwrap_or(i64::MAX));
		queue.auto_continue_override = None;
		let actor = Actor::InteractiveClient {
			client_id: plan.client_id,
		};
		let mut kept = Vec::new();
		for mut entry in queue.entries {
			if entry.turn.source == TurnSource::AutoContinue
				&& entry.turn.state == crate::TurnState::Queued
			{
				entry.turn.state = crate::TurnState::Canceled;
				turn_queue::changed(tx, &actor, id, &entry, now).await?;
			} else {
				kept.push(entry);
			}
		}
		queue.entries = kept;
		let decision = AutoContinueRetry {
			run_id,
			triggering_turn: condition.turn_id,
			retry_turn: None,
			usage: condition.usage,
			observed_at_unix_ms: condition.observed_at,
			due_at_unix_ms: queue.quota_until.expect("quota wait"),
			retry_count: 0,
			policy,
			selected_from,
			status: AutoContinueStatus::Disabled,
		};
		queue.auto_continue = Some(decision.clone());
		turn_queue::save(tx, id, &queue).await?;
		return changed(tx, &actor, id, decision, now).await;
	};
	// A native identity is required: automatic input must never create a new
	// native Conversation when the failed turn has not supplied its identity.
	if state
		.native_conversation
		.as_ref()
		.or(plan.native_conversation.as_ref())
		.is_none()
	{
		return Ok(());
	}
	let delay = u64::from(delay_ms)
		.saturating_mul(1_u64.checked_shl(count).unwrap_or(u64::MAX))
		.min(u64::from(max_delay_ms));
	let condition = state
		.quota
		.iter()
		.filter(|c| exhausted(&c.usage))
		.max_by_key(|c| due_at(c, delay))
		.expect("exhausted condition")
		.clone();
	let due = due_at(&condition, delay);
	queue.quota_until = Some(due);
	let mut decision = AutoContinueRetry {
		run_id,
		triggering_turn: condition.turn_id,
		retry_turn: None,
		usage: condition.usage,
		observed_at_unix_ms: condition.observed_at,
		due_at_unix_ms: due,
		retry_count: if count < max_retries {
			count + 1
		} else {
			count
		},
		policy: policy.clone(),
		selected_from,
		status: if count < max_retries {
			AutoContinueStatus::Pending
		} else {
			AutoContinueStatus::Exhausted
		},
	};
	let actor = Actor::InteractiveClient {
		client_id: plan.client_id,
	};
	if decision.status == AutoContinueStatus::Pending {
		// Prepare validates capacity before any state changes. A full queue may
		// be retried on a later tick without losing its user or schedule inputs.
		let prepared = turn_queue::prepare(
			tx,
			&actor,
			CommandId(uuid::Uuid::now_v7()),
			id,
			TurnSource::AutoContinue,
			turn_queue::Admission::Prompt(message.clone()),
		)
		.await;
		let (mut prepared, mut changes) = match prepared {
			Ok(value) => value,
			Err(error) if error.code == "turn.queue_full" => {
				decision.status = AutoContinueStatus::Deferred;
				decision.retry_count = count;
				queue.auto_continue_override = None;
				let unchanged = queue.auto_continue.as_ref() == Some(&decision);
				queue.auto_continue = Some(decision.clone());
				turn_queue::save(tx, id, &queue).await?;
				if !unchanged {
					changed(tx, &actor, id, decision, now).await?;
				}
				return Ok(());
			}
			Err(error) => return Err(error),
		};
		let admitted = prepared.entries.last_mut().expect("admitted retry");
		admitted.auto_continue_run = Some(run_id);
		admitted.auto_continue_model = state.model.clone().or(plan.model);
		decision.retry_turn = Some(admitted.turn.turn_id);
		changes
			.last_mut()
			.expect("admission event")
			.auto_continue_run = Some(run_id);
		prepared.quota_until = queue.quota_until;
		prepared.auto_continue_override = None;
		prepared.auto_continue = Some(decision.clone());
		turn_queue::commit(tx, &actor, id, &prepared, &changes, now).await?;
	} else {
		queue.auto_continue_override = None;
		queue.auto_continue = Some(decision.clone());
		turn_queue::save(tx, id, &queue).await?;
	}
	changed(tx, &actor, id, decision, now).await
}

pub(crate) async fn changed(
	tx: &mut WriteTransaction,
	actor: &Actor,
	id: ConversationId,
	retry: AutoContinueRetry,
	now: i64,
) -> Result<(), CoreError> {
	tx.append_event(
		EventKind::AutoContinueChanged {
			retry: Box::new(retry),
		}
		.to_record_as(
			EventActor::AutoContinue {
				authorized_by: actor.client_id(),
			},
			crate::event::EventSubject::Conversation(id),
			now,
		)?,
	)
	.await?;
	Ok(())
}

pub(crate) async fn claimed(
	tx: &mut WriteTransaction,
	id: ConversationId,
	queue: &mut turn_queue::Queue,
	entry: &turn_queue::Entry,
	now: i64,
) -> Result<(), CoreError> {
	if entry.auto_continue_run.is_some()
		&& let Some(retry) = &mut queue.auto_continue
	{
		retry.status = AutoContinueStatus::Dispatched;
		changed(
			tx,
			&Actor::InteractiveClient {
				client_id: entry.turn.client_id,
			},
			id,
			retry.clone(),
			now,
		)
		.await?;
	}
	let run_id = entry.turn.run_id.expect("claimed Run");
	if let Some(execution) = tx.run_execution(run_id.0).await? {
		let mut state: run_state::State = run_state::decode(&execution.state)?;
		state.current_turn = Some(entry.turn.turn_id);
		state.quota.clear();
		state.quota_wait = false;
		run_state::save(tx, run_id, &state).await?;
	}
	queue.quota_until = None;
	Ok(())
}

/// A tick retries capacity-blocked admissions and wakes live Runs whose delay ended.
impl crate::Core {
	pub(crate) async fn wake_auto_continue(&self) -> Result<(), CoreError> {
		let mut after = String::new();
		loop {
			let ids = self
				.store
				.read(async |tx| tx.pending_turn_queues(&after).await)
				.await?;
			if ids.is_empty() {
				break;
			}
			for id in ids {
				after = id.to_string();
				self.store
					.write(async |tx| {
						let queue =
							turn_queue::load(tx, ConversationId(id)).await?;
						if queue.auto_continue.as_ref().is_some_and(|r| {
							r.status == AutoContinueStatus::Deferred
						}) {
							reconsider(
								tx,
								ConversationId(id),
								self.now_unix_ms(),
							)
							.await?;
						}
						Ok::<_, CoreError>(())
					})
					.await?;
			}
		}
		self.turn_wake.send_replace(());
		Ok(())
	}
}

/// Re-evaluate only the most recent managed Run; older waits have no authority.
pub(crate) async fn reconsider(
	tx: &mut WriteTransaction,
	id: ConversationId,
	now: i64,
) -> Result<(), CoreError> {
	for run in tx.runs(id.0).await?.iter().rev() {
		if tx.run_execution(run.run_id).await?.is_some() {
			return consider(tx, RunId(run.run_id), now).await;
		}
	}
	Ok(())
}

/// A preceding scheduled turn may have started a Run with new defaults.
/// Never send an older retry to that connection without matching its selection.
pub(crate) async fn matches_execution(
	tx: &mut jet_store::ReadTransaction,
	entry: &turn_queue::Entry,
	plan: &LaunchPlan,
	state: &run_state::State,
) -> Result<bool, CoreError> {
	let Some(origin) = entry.auto_continue_run else {
		return Ok(true);
	};
	let Some(record) = tx.run_execution(origin.0).await? else {
		return Ok(false);
	};
	let original: LaunchPlan = run_state::decode(&record.plan)?;
	let original_state: run_state::State = run_state::decode(&record.state)?;
	let binding_present = if let Some(binding) = original.visa {
		tx.account_binding(binding.account_binding_id.0)
			.await?
			.is_some()
	} else {
		false
	};
	Ok(binding_present
		&& original.craft == plan.craft
		&& original.visa == plan.visa
		&& original.no_visa == plan.no_visa
		&& original.root == plan.root
		&& original.project_root == plan.project_root
		&& entry.auto_continue_model.as_ref()
			== state.model.as_ref().or(plan.model.as_ref())
		&& original_state
			.native_conversation
			.as_ref()
			.or(original.native_conversation.as_ref())
			== state
				.native_conversation
				.as_ref()
				.or(plan.native_conversation.as_ref()))
}

#[path = "timing.rs"]
mod timing;
use timing::{due_at, refresh_pending, reset_at};
