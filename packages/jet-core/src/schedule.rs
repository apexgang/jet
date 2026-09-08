//! Immutable daily Scheduled tasks attached to retained Conversations (ADR-0024).
use crate::{
	Actor, ClientId, CommandOutcome, ConversationId, CoreError, EventKind,
	EventSequence,
};
use jet_store::{ReadTransaction, WriteTransaction};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Durable admission outcome of one intended firing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleFiringOutcome {
	/// Admitted to the replaceable schedule slot.
	Queued,
	/// Replaced by a more recent missed occurrence.
	Superseded,
	/// Outside the seven-day catch-up window.
	Expired,
}
/// One intended local occurrence, resolved once and persisted before delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleFiring {
	/// Deterministic identity, also the admitted Turn identity.
	pub firing_id: Uuid,
	/// Original local date and time, including a nonexistent or repeated time.
	pub intended_local: String,
	/// Selected UTC instant in signed Unix milliseconds.
	pub due_at_unix_ms: i64,
}
/// An enabled daily Scheduled task. Cancel and create anew to change its rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledTask {
	/// Immutable schedule identity.
	pub schedule_id: Uuid,
	/// Durable owning Conversation.
	pub conversation_id: ConversationId,
	/// Client that authorized scheduled input.
	pub authorized_by: ClientId,
	/// Original IANA zone, independent of the Plane's current zone.
	pub time_zone: String,
	/// Daily local time in HH:MM:SS form.
	pub local_time: String,
	/// Turn input, bounded to 8192 UTF-8 bytes.
	pub prompt: String,
	/// Next intended occurrence, retained unchanged across restarts.
	pub next: ScheduleFiring,
}
/// Enabled schedules and their snapshot fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledTasks {
	/// Plane Event high-water cursor in this read transaction.
	pub cursor: EventSequence,
	/// At most 32 schedules in the addressed Conversation.
	pub tasks: Vec<ScheduledTask>,
}
pub(crate) async fn snapshot(
	tx: &mut ReadTransaction,
	id: ConversationId,
) -> Result<ScheduledTasks, CoreError> {
	crate::turn_queue::load(tx, id).await?;
	let tasks = tx
		.scheduled_tasks(id.0)
		.await?
		.iter()
		.map(|json| crate::run_state::decode(json))
		.collect::<Result<_, _>>()?;
	Ok(ScheduledTasks {
		cursor: EventSequence(tx.event_cursor().await?),
		tasks,
	})
}
pub(crate) async fn create(
	tx: &mut WriteTransaction,
	actor: &Actor,
	id: ConversationId,
	time_zone: String,
	local_time: String,
	prompt: String,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let existing = snapshot(tx, id).await?;
	if existing.tasks.len() >= 32 || prompt.is_empty() || prompt.len() > 8192 {
		return Err(CoreError::invalid_input(
			"schedule.limit",
			"a Conversation supports 32 schedules with 1 to 8192 bytes of input",
		));
	}
	if tx
		.conversation(id.0)
		.await?
		.expect("validated Conversation")
		.retention
		!= crate::RetentionPolicy::Retain
	{
		return Err(CoreError::invalid_input(
			"schedule.retention",
			"Scheduled tasks require a retained Conversation",
		));
	}
	let schedule_id = Uuid::now_v7();
	let next = crate::schedule_clock::first(
		schedule_id,
		&time_zone,
		&local_time,
		now,
	)?;
	let task = ScheduledTask {
		schedule_id,
		conversation_id: id,
		authorized_by: actor.client_id(),
		time_zone,
		local_time,
		prompt,
		next,
	};
	save(tx, &task).await?;
	tx.append_event(
		EventKind::ScheduleCreated { task: task.clone() }.to_record(
			actor,
			crate::event::EventSubject::Conversation(id),
			now,
		)?,
	)
	.await?;
	Ok(CommandOutcome::ScheduleCreated(task))
}
pub(crate) async fn cancel(
	tx: &mut WriteTransaction,
	actor: &Actor,
	schedule_id: Uuid,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let json = tx.scheduled_task(schedule_id).await?.ok_or_else(|| {
		CoreError::not_found(
			"schedule.not_found",
			"the Scheduled task does not exist",
		)
	})?;
	let task: ScheduledTask = crate::run_state::decode(&json)?;
	let mut queue = crate::turn_queue::load(tx, task.conversation_id).await?;
	let mut kept = Vec::new();
	for mut entry in queue.entries {
		if entry
			.schedule
			.as_ref()
			.is_some_and(|s| s.schedule_id == schedule_id)
			&& entry.turn.state == crate::TurnState::Queued
		{
			entry.turn.state = crate::TurnState::Canceled;
			crate::turn_queue::changed(
				tx,
				actor,
				task.conversation_id,
				&entry,
				now,
			)
			.await?;
		} else {
			kept.push(entry);
		}
	}
	queue.entries = kept;
	crate::turn_queue::save(tx, task.conversation_id, &queue).await?;
	tx.delete_schedule(schedule_id).await?;
	tx.append_event(EventKind::ScheduleCanceled { schedule_id }.to_record(
		actor,
		crate::event::EventSubject::Conversation(task.conversation_id),
		now,
	)?)
	.await?;
	Ok(CommandOutcome::ScheduleCanceled { schedule_id })
}
pub(crate) async fn save(
	tx: &mut WriteTransaction,
	task: &ScheduledTask,
) -> Result<(), CoreError> {
	let json = serde_json::to_string(task)
		.map_err(|e| CoreError::internal("schedule.encode", e.to_string()))?;
	tx.save_schedule(
		task.schedule_id,
		task.conversation_id.0,
		task.next.due_at_unix_ms,
		&json,
	)
	.await?;
	Ok(())
}
