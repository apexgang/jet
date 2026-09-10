//! Durable firing advancement and coalescing into the Conversation Turn queue.
use crate::{
	Actor, CommandId, Core, CoreError, EventActor, EventKind, ScheduledTask,
	TurnSource, TurnState, turn_queue,
};
use jet_store::WriteTransaction;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
const CATCH_UP_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ScheduleInput {
	pub(crate) schedule_id: Uuid,
	pub(crate) firing: crate::ScheduleFiring,
}
impl ScheduleInput {
	pub(crate) fn expired(&self, now: i64) -> bool {
		self.firing.due_at_unix_ms < now.saturating_sub(CATCH_UP_MS)
	}
}
impl Core {
	/// Advance due schedules transactionally, retaining only the newest missed input.
	/// The daemon calls this on startup and its recovery tick, before Run dispatch.
	/// # Errors
	/// Returns a persistence error without advancing an uncommitted firing.
	pub async fn perform_schedules(&self) -> Result<(), CoreError> {
		let now = self.now_unix_ms();
		let mut after_schedule = String::new();
		let mut has_work = false;
		loop {
			let due = self
				.store
				.read(async |tx| tx.due_schedules(now, &after_schedule).await)
				.await?;
			if due.is_empty() {
				break;
			}
			has_work = true;
			for json in due {
				let task: ScheduledTask = crate::run_state::decode(&json)?;
				after_schedule = task.schedule_id.to_string();
				let result = self
					.store
					.write(async |tx| {
						let Some(json) =
							tx.scheduled_task(task.schedule_id).await?
						else {
							return Ok(());
						};
						let task: ScheduledTask =
							crate::run_state::decode(&json)?;
						advance(tx, task, now).await
					})
					.await;
				// A full user queue retains the unadvanced firing for the next tick.
				if let Err(error) = result
					&& error.code != "turn.queue_full"
				{
					return Err(error);
				}
			}
		}
		let mut after = String::new();
		loop {
			let ids = self
				.store
				.read(async |tx| tx.pending_turn_queues(&after).await)
				.await?;
			if ids.is_empty() {
				break;
			}
			has_work = true;
			for id in ids {
				after = id.to_string();
				self.store
					.write(async |tx| {
						expire(tx, crate::ConversationId(id), now).await
					})
					.await?;
			}
		}
		if has_work {
			self.run_work.notify_one();
		}
		Ok(())
	}
}
async fn advance(
	tx: &mut WriteTransaction,
	mut task: ScheduledTask,
	now: i64,
) -> Result<(), CoreError> {
	let actor = Actor::InteractiveClient {
		client_id: task.authorized_by,
	};
	// Bound transaction work after long offline periods. No intermediate firing
	// is admitted while another occurrence of this schedule is already due.
	for _ in 0..128 {
		if task.next.due_at_unix_ms > now {
			break;
		}
		let next = crate::schedule_clock::next(&task)?;
		let expired =
			task.next.due_at_unix_ms < now.saturating_sub(CATCH_UP_MS);
		let mut outcome = if expired {
			crate::ScheduleFiringOutcome::Expired
		} else {
			crate::ScheduleFiringOutcome::Superseded
		};
		if !expired && next.due_at_unix_ms > now {
			let queue = turn_queue::load(tx, task.conversation_id).await?;
			let newer = queue.entries.iter().any(|entry| {
				entry.turn.state == TurnState::Queued
					&& entry.schedule.as_ref().is_some_and(|input| {
						input.firing.due_at_unix_ms > task.next.due_at_unix_ms
					})
			});
			if !newer {
				let (queue, changes) = turn_queue::prepare(
					tx,
					&actor,
					CommandId(task.next.firing_id),
					task.conversation_id,
					TurnSource::Schedule,
					turn_queue::Admission::Scheduled {
						prompt: task.prompt.clone(),
						schedule: ScheduleInput {
							schedule_id: task.schedule_id,
							firing: task.next.clone(),
						},
					},
				)
				.await?;
				turn_queue::commit(
					tx,
					&actor,
					task.conversation_id,
					&queue,
					&changes,
					now,
				)
				.await?;
				outcome = crate::ScheduleFiringOutcome::Queued;
			}
		}
		tx.append_event(
			EventKind::ScheduleFired {
				schedule_id: task.schedule_id,
				firing: task.next.clone(),
				outcome,
			}
			.to_record_as(
				EventActor::ScheduledTask {
					schedule_id: task.schedule_id,
					authorized_by: task.authorized_by,
				},
				crate::event::EventSubject::Conversation(task.conversation_id),
				now,
			)?,
		)
		.await?;
		task.next = next;
	}
	crate::schedule::save(tx, &task).await
}
async fn expire(
	tx: &mut WriteTransaction,
	id: crate::ConversationId,
	now: i64,
) -> Result<(), CoreError> {
	let mut queue = turn_queue::load(tx, id).await?;
	let mut kept = Vec::new();
	let mut changed = false;
	for mut entry in queue.entries {
		if entry.turn.state == TurnState::Queued
			&& entry
				.schedule
				.as_ref()
				.is_some_and(|input| input.expired(now))
		{
			entry.turn.state = TurnState::Canceled;
			let actor = Actor::InteractiveClient {
				client_id: entry.turn.client_id,
			};
			turn_queue::changed(tx, &actor, id, &entry, now).await?;
			let input = entry.schedule.expect("expired scheduled input");
			tx.append_event(
				EventKind::ScheduleFired {
					schedule_id: input.schedule_id,
					firing: input.firing,
					outcome: crate::ScheduleFiringOutcome::Expired,
				}
				.to_record_as(
					EventActor::ScheduledTask {
						schedule_id: input.schedule_id,
						authorized_by: entry.turn.client_id,
					},
					crate::event::EventSubject::Conversation(id),
					now,
				)?,
			)
			.await?;
			changed = true;
		} else {
			kept.push(entry);
		}
	}
	queue.entries = kept;
	if changed {
		turn_queue::save(tx, id, &queue).await?;
	}
	Ok(())
}

/// Recheck at the claim transaction, so elapsed catch-up windows or still-due
/// schedules cannot dispatch stale input while the scheduler is catching up.
pub(crate) async fn can_dispatch(
	tx: &mut jet_store::ReadTransaction,
	id: crate::ConversationId,
	queue: &turn_queue::Queue,
	now: i64,
) -> Result<bool, CoreError> {
	if !queue.ready() || queue.quota_until.is_some_and(|due| due > now) {
		return Ok(false);
	}
	let Some(input) = queue
		.entries
		.first()
		.and_then(|entry| entry.schedule.as_ref())
	else {
		return Ok(true);
	};
	if input.expired(now) {
		return Ok(false);
	}
	for json in tx.scheduled_tasks(id.0).await? {
		let task: ScheduledTask = crate::run_state::decode(&json)?;
		if task.next.due_at_unix_ms <= now {
			return Ok(false);
		}
	}
	Ok(true)
}
