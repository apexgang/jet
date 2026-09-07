//! Atomic queue admission and replacement, serialized by the Plane store.
use crate::{
	Actor, CommandOutcome, ConversationId, CoreError, EventKind, EventSequence,
	Turn, TurnQueue, TurnSource, TurnState,
};
use jet_store::{ReadTransaction, WriteTransaction};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Queue {
	sequence: u64,
	pub(crate) entries: Vec<Entry>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Entry {
	pub(crate) turn: Turn,
	pub(crate) prompt: String,
	pub(crate) command_id: Uuid,
}

pub(crate) async fn load(
	tx: &mut ReadTransaction,
	id: ConversationId,
) -> Result<Queue, CoreError> {
	if tx.conversation(id.0).await?.is_none() {
		return Err(CoreError::not_found(
			"conversation.not_found",
			"the Conversation does not exist",
		));
	}
	tx.turn_queue(id.0)
		.await?
		.map(|json| crate::run_state::decode(&json))
		.transpose()
		.map(Option::unwrap_or_default)
}
pub(crate) async fn save(
	tx: &mut WriteTransaction,
	id: ConversationId,
	queue: &Queue,
) -> Result<(), CoreError> {
	let json = serde_json::to_string(queue)
		.map_err(|e| CoreError::internal("turn.encode", e.to_string()))?;
	let pending = queue
		.entries
		.iter()
		.filter(|entry| entry.turn.state == TurnState::Queued)
		.count() as u32;
	tx.save_turn_queue(id.0, &json, pending).await?;
	Ok(())
}
pub(crate) async fn snapshot(
	tx: &mut ReadTransaction,
	id: ConversationId,
) -> Result<TurnQueue, CoreError> {
	let queue = load(tx, id).await?;
	Ok(TurnQueue {
		cursor: EventSequence(tx.event_cursor().await?),
		turns: queue.entries.into_iter().map(|e| e.turn).collect(),
	})
}
pub(crate) async fn changed(
	tx: &mut WriteTransaction,
	actor: &Actor,
	id: ConversationId,
	entry: &Entry,
	now: i64,
) -> Result<(), CoreError> {
	let origin = match entry.turn.state {
		TurnState::Completed => crate::EventActor::Harness {
			run_id: entry.turn.run_id.expect("completed execution"),
			authorized_by: entry.turn.client_id,
		},
		TurnState::Active | TurnState::Failed | TurnState::OutcomeUnknown => {
			crate::EventActor::RunSupervisor {
				run_id: entry.turn.run_id.expect("claimed execution"),
				authorized_by: entry.turn.client_id,
			}
		}
		TurnState::Queued
		| TurnState::Superseded
		| TurnState::Canceled
		| TurnState::Withdrawn => crate::EventActor::InteractiveClient {
			client_id: actor.client_id(),
		},
	};
	let subject = entry.turn.run_id.map_or(
		crate::event::EventSubject::Conversation(id),
		|run_id| crate::event::EventSubject::Run {
			conversation_id: id,
			run_id,
		},
	);
	tx.append_event(
		EventKind::TurnChanged {
			turn: entry.turn.clone(),
		}
		.to_record_as(origin, subject, now)?,
	)
	.await?;
	if entry.turn.state == TurnState::Queued {
		// Even all-control-character input must fit the 64 KiB Event payload.
		// Splitting at UTF-8 boundaries preserves the exact original text.
		let mut text = entry.prompt.as_str();
		while !text.is_empty() {
			let end = text.floor_char_boundary(text.len().min(8192));
			let (chunk, remaining) = text.split_at(end);
			tx.append_event(
				EventKind::TurnInput {
					turn_id: entry.turn.turn_id,
					text: chunk.into(),
				}
				.to_record(actor, subject, now)?,
			)
			.await?;
			text = remaining;
		}
	}
	Ok(())
}
pub(crate) async fn admit(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: crate::CommandId,
	id: ConversationId,
	source: TurnSource,
	prompt: String,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let (queue, changes) =
		prepare(tx, actor, command_id, id, source, prompt).await?;
	let turn = queue
		.entries
		.last()
		.expect("admission appended")
		.turn
		.clone();
	commit(tx, actor, id, &queue, &changes, now).await?;
	Ok(CommandOutcome::TurnAdmitted(turn))
}
// Validate everything before journal writes: authoritative refusals retain receipts.
pub(crate) async fn prepare(
	tx: &mut ReadTransaction,
	actor: &Actor,
	command_id: crate::CommandId,
	id: ConversationId,
	source: TurnSource,
	prompt: String,
) -> Result<(Queue, Vec<Entry>), CoreError> {
	// ASVS 2.2.1, 2.3.3, 2.3.4: bounds, sequences and replacements share
	// the receipt's write transaction, so a refusal cannot consume user work.
	if prompt.is_empty() || prompt.len() > 65_536 {
		return Err(CoreError::invalid_input(
			"turn.invalid_prompt",
			"input must contain 1 to 65536 bytes",
		));
	}
	let mut queue = load(tx, id).await?;
	let mut kept = Vec::new();
	let mut removed = Vec::new();
	for mut entry in queue.entries {
		let replaced = entry.turn.state == TurnState::Queued
			&& ((source != TurnSource::User && entry.turn.source == source)
				|| (source == TurnSource::User
					&& entry.turn.source == TurnSource::AutoContinue));
		if replaced {
			entry.turn.state = if source == TurnSource::User {
				TurnState::Canceled
			} else {
				TurnState::Superseded
			};
			removed.push(entry);
		} else {
			kept.push(entry);
		}
	}
	if kept.len() >= 128
		|| kept.iter().map(|e| e.prompt.len()).sum::<usize>() + prompt.len()
			> 1024 * 1024
	{
		return Err(CoreError::conflict(
			"turn.queue_full",
			"the Turn queue has reached its input limit",
		));
	}
	queue.sequence = queue.sequence.checked_add(1).ok_or_else(|| {
		CoreError::conflict(
			"turn.sequence_exhausted",
			"the Conversation sequence is exhausted",
		)
	})?;
	let entry = Entry {
		turn: Turn {
			turn_id: Uuid::now_v7(),
			sequence: queue.sequence,
			client_id: actor.client_id(),
			source,
			state: TurnState::Queued,
			run_id: None,
		},
		prompt,
		command_id: command_id.0,
	};
	removed.push(entry.clone());
	kept.push(entry);
	queue.entries = kept;
	Ok((queue, removed))
}

pub(crate) async fn withdraw(
	tx: &mut WriteTransaction,
	actor: &Actor,
	id: ConversationId,
	turn_id: Uuid,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let mut queue = load(tx, id).await?;
	let index = queue
		.entries
		.iter()
		.position(|entry| entry.turn.turn_id == turn_id)
		.ok_or_else(|| {
			CoreError::not_found(
				"turn.not_found",
				"the turn is no longer queued",
			)
		})?;
	let turn = &queue.entries[index].turn;
	// ASVS 8.2.2, 8.3.1: client identity comes from authentication, not input.
	if turn.client_id != actor.client_id()
		|| turn.source != TurnSource::User
		|| turn.state != TurnState::Queued
	{
		return Err(CoreError::conflict(
			"turn.withdraw_denied",
			"only the admitting client may withdraw its own queued user turn",
		));
	}
	let mut entry = queue.entries.remove(index);
	entry.turn.state = TurnState::Withdrawn;
	changed(tx, actor, id, &entry, now).await?;
	save(tx, id, &queue).await?;
	Ok(CommandOutcome::TurnWithdrawn(entry.turn))
}

pub(crate) async fn commit(
	tx: &mut WriteTransaction,
	actor: &Actor,
	id: ConversationId,
	queue: &Queue,
	changes: &[Entry],
	now: i64,
) -> Result<(), CoreError> {
	for entry in changes {
		changed(tx, actor, id, entry, now).await?;
	}
	save(tx, id, queue).await
}
