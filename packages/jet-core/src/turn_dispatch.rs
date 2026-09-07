//! A durable claim precedes each native turn; uncertain delivery is never retried.
use crate::{
	Actor, ConversationId, Core, CoreError, RunConnection, RunId, RunLifecycle,
	TurnState, turn_queue,
};
use jet_store::WriteTransaction;
use uuid::Uuid;

impl Core {
	pub(crate) async fn dispatch_turn(
		&self,
		run_id: RunId,
		connection: &dyn RunConnection,
	) -> Result<(), CoreError> {
		let next = self
			.store
			.write(async |tx| {
				let Some(run) = tx.run(run_id.0).await? else {
					return Ok(None);
				};
				let Some(record) = tx.run_execution(run_id.0).await? else {
					return Ok(None);
				};
				let state: crate::run_state::State =
					crate::run_state::decode(&record.state)?;
				let plan: crate::LaunchPlan =
					crate::run_state::decode(&record.plan)?;
				// A pre-queue Run still owns its initial turn until its completion.
				if plan.turn_id.is_none() && state.native_conversation.is_none()
				{
					return Ok(None);
				}
				// ASVS 2.3.1, 2.3.4: waiting activity cannot release the active turn.
				// Only a fully committed source boundary permits the next delivery.
				if run.lifecycle != RunLifecycle::Active
					|| state.disconnected
					|| state.source_offset == 0
					|| state.partial_source.count != 0
				{
					return Ok(None);
				}
				let id = ConversationId(run.conversation_id);
				let mut queue = turn_queue::load(tx, id).await?;
				let next = queue.claim(run_id);
				if let Some(entry) = &next {
					let actor = Actor::InteractiveClient {
						client_id: entry.turn.client_id,
					};
					turn_queue::changed(
						tx,
						&actor,
						id,
						entry,
						self.now_unix_ms(),
					)
					.await?;
					turn_queue::save(tx, id, &queue).await?;
				}
				Ok::<_, CoreError>(next)
			})
			.await?;
		if let Some(entry) = next {
			connection
				.submit_turn(entry.turn.turn_id, entry.prompt)
				.await?;
		}
		Ok(())
	}
}

pub(crate) enum Settlement {
	Completed { turn_id: Uuid },
	Failed,
	OutcomeUnknown,
}

pub(crate) async fn settle(
	tx: &mut WriteTransaction,
	run_id: RunId,
	settlement: Settlement,
	now: i64,
) -> Result<(), CoreError> {
	let run = tx.run(run_id.0).await?.expect("observed Run exists");
	let id = ConversationId(run.conversation_id);
	let mut queue = turn_queue::load(tx, id).await?;
	let Some(index) = queue
		.entries
		.iter()
		.position(|entry| entry.turn.run_id == Some(run_id))
	else {
		// Pre-queue executions have no initial admission to settle.
		if matches!(settlement, Settlement::Completed { turn_id } if turn_id != run_id.0)
		{
			return Err(invalid_completion());
		}
		return Ok(());
	};
	let mut entry = queue.entries[index].clone();
	if matches!(settlement, Settlement::Completed { turn_id } if turn_id != entry.turn.turn_id)
	{
		return Err(invalid_completion());
	}
	let state = match settlement {
		Settlement::Completed { .. } => TurnState::Completed,
		Settlement::Failed => TurnState::Failed,
		Settlement::OutcomeUnknown => TurnState::OutcomeUnknown,
	};
	if entry.turn.state == state {
		return Ok(());
	}
	entry.turn.state = state;
	if state == TurnState::OutcomeUnknown {
		queue.entries[index] = entry.clone();
	} else {
		queue.entries.remove(index);
	}
	let actor = Actor::InteractiveClient {
		client_id: entry.turn.client_id,
	};
	turn_queue::changed(tx, &actor, id, &entry, now).await?;
	turn_queue::save(tx, id, &queue).await
}
fn invalid_completion() -> CoreError {
	CoreError::conflict(
		"turn.invalid_completion",
		"the completion does not identify the active turn",
	)
}
