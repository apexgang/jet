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
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub(crate) review: Option<Vec<crate::ReviewComment>>,
	pub(crate) command_id: Uuid,
}

pub(crate) enum Admission {
	Prompt(String),
	Review {
		prompt: String,
		comments: Vec<crate::ReviewComment>,
	},
}

impl Admission {
	fn into_parts(self) -> (String, Option<Vec<crate::ReviewComment>>) {
		match self {
			Self::Prompt(prompt) => (prompt, None),
			Self::Review { prompt, comments } => (prompt, Some(comments)),
		}
	}
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
		if let Some(comments) = &entry.review {
			tx.append_event(
				EventKind::ReviewSubmitted {
					turn_id: entry.turn.turn_id,
					comments: comments.clone(),
				}
				.to_record(actor, subject, now)?,
			)
			.await?;
		}
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
		prepare(tx, actor, command_id, id, source, Admission::Prompt(prompt))
			.await?;
	let turn = queue
		.entries
		.last()
		.expect("admission appended")
		.turn
		.clone();
	commit(tx, actor, id, &queue, &changes, now).await?;
	Ok(CommandOutcome::TurnAdmitted(turn))
}

pub(crate) async fn admit_review(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: crate::CommandId,
	id: ConversationId,
	comments: Vec<crate::ReviewComment>,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	// ASVS 5.1.1, 5.1.3: preserve structure only after every nested value
	// has passed explicit count, line, path, and text bounds.
	if comments.is_empty() || comments.len() > 128 {
		return Err(CoreError::invalid_input(
			"review.invalid_comments",
			"a review contains 1 to 128 comments",
		));
	}
	if comments.iter().any(|comment| {
		comment.line == 0
			|| comment.comment.is_empty()
			|| comment.comment.len() > 8192
	}) {
		return Err(CoreError::invalid_input(
			"review.invalid_comment",
			"each review comment has a one-based line and 1 to 8192 bytes of text",
		));
	}
	let prompt = review_prompt(&comments);
	if prompt.len() > 65_536 {
		return Err(CoreError::invalid_input(
			"review.too_large",
			"the submitted review must fit in one Turn",
		));
	}
	let event_payload = serde_json::to_vec(&serde_json::json!({
		"turn_id": Uuid::nil(),
		"comments": &comments,
	}))
	.map_err(|error| {
		CoreError::internal("review.unencodable", error.to_string())
	})?;
	if event_payload.len() > 65_536 {
		return Err(CoreError::invalid_input(
			"review.too_large",
			"the submitted review must fit in one durable Event",
		));
	}
	let (queue, changes) = prepare(
		tx,
		actor,
		command_id,
		id,
		TurnSource::User,
		Admission::Review { prompt, comments },
	)
	.await?;
	let turn = queue
		.entries
		.last()
		.expect("admission appended")
		.turn
		.clone();
	commit(tx, actor, id, &queue, &changes, now).await?;
	Ok(CommandOutcome::TurnAdmitted(turn))
}

fn review_prompt(comments: &[crate::ReviewComment]) -> String {
	use std::fmt::Write as _;
	let mut prompt = String::from("Review comments:\n");
	for comment in comments {
		let _ = write!(
			prompt,
			"\n{}:{}\n{}\n",
			comment.path.as_str(),
			comment.line,
			comment.comment
		);
	}
	prompt
}
// Validate everything before journal writes: authoritative refusals retain receipts.
pub(crate) async fn prepare(
	tx: &mut ReadTransaction,
	actor: &Actor,
	command_id: crate::CommandId,
	id: ConversationId,
	source: TurnSource,
	admission: Admission,
) -> Result<(Queue, Vec<Entry>), CoreError> {
	let (prompt, review) = admission.into_parts();
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
		review,
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
impl Queue {
	pub(crate) fn ready(&self) -> bool {
		!self.entries.is_empty()
			&& self
				.entries
				.iter()
				.all(|entry| entry.turn.state == TurnState::Queued)
	}
	pub(crate) fn claim(&mut self, run_id: crate::RunId) -> Option<Entry> {
		if !self.ready() {
			return None;
		}
		let entry = self.entries.first_mut()?;
		entry.turn.state = TurnState::Active;
		entry.turn.run_id = Some(run_id);
		Some(entry.clone())
	}
}
