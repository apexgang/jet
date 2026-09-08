//! Pending input continues the Conversation's last accepted managed execution.
use crate::{
	Actor, CommandId, CommandOutcome, ConversationId, Core, CoreError,
	LaunchPlan, turn_queue,
};
use jet_store::{ReadTransaction, WriteTransaction};

#[derive(PartialEq, Eq)]
struct Continuation {
	previous_run_id: uuid::Uuid,
	plan: LaunchPlan,
}

impl Core {
	/// Waits for committed input or a released Run to wake execution workers.
	pub async fn wait_for_run_work(&self) {
		self.run_work.notified().await;
	}
	pub(crate) async fn prepare_queued_runs(&self) -> Result<(), CoreError> {
		let mut after = String::new();
		loop {
			let ids = self
				.store
				.read(async |tx| tx.pending_turn_queues(&after).await)
				.await?;
			if ids.is_empty() {
				return Ok(());
			}
			for id in ids {
				after = id.to_string();
				let outcome = self.prepare_queued_run(ConversationId(id)).await;
				// Busy checkouts and unavailable pins retain their exact pending input.
				if let Err(error) = outcome
					&& !error.is_authoritative_result()
				{
					return Err(error);
				}
			}
		}
	}
	async fn prepare_queued_run(
		&self,
		id: ConversationId,
	) -> Result<(), CoreError> {
		let Some(host) = &self.run_host else {
			return Ok(());
		};
		let Some(accepted) = self
			.store
			.read(async |tx| continuation(tx, id, self.now_unix_ms()).await)
			.await?
		else {
			return Ok(());
		};
		// Read-only external validation precedes the write transaction. The
		// transaction rechecks the selection before admitting another execution.
		let prepared = async {
			let plan = host.prepare_next_run(accepted.plan.clone()).await?;
			plan.revalidate().await?;
			Ok::<_, CoreError>(plan)
		}
		.await;
		let plan = match prepared {
			Ok(plan) => plan,
			// External preparation is local to this Conversation. In particular,
			// an unavailable artifact must not block unrelated launch Effects.
			Err(error)
				if error.category == crate::ErrorCategory::Unavailable =>
			{
				return Ok(());
			}
			Err(error) => return Err(error),
		};
		self.store
			.write(async |tx| {
				if continuation(tx, id, self.now_unix_ms()).await?.as_ref()
					!= Some(&accepted)
				{
					return Ok(());
				}
				prepare(tx, id, plan, self.now_unix_ms()).await
			})
			.await
	}
}

async fn continuation(
	tx: &mut ReadTransaction,
	id: ConversationId,
	now: i64,
) -> Result<Option<Continuation>, CoreError> {
	let queue = turn_queue::load(tx, id).await?;
	if !crate::schedule_work::can_dispatch(tx, id, &queue, now).await? {
		return Ok(None);
	}
	let runs = tx.runs(id.0).await?;
	if runs.iter().any(|run| !run.lifecycle.is_terminal()) {
		return Ok(None);
	}
	for run in runs.iter().rev() {
		if let Some(execution) = tx.run_execution(run.run_id).await? {
			let state: crate::run_state::State =
				crate::run_state::decode(&execution.state)?;
			if state.partial_source.count != 0 {
				return Ok(None);
			}
			let mut plan: LaunchPlan =
				crate::run_state::decode(&execution.plan)?;
			// Fork delivery creates only the new native Conversation. Later queued
			// Runs continue the destination's own durable native identity.
			plan.fork = None;
			plan.native_conversation =
				state.native_conversation.or(plan.native_conversation);
			return Ok(Some(Continuation {
				previous_run_id: run.run_id,
				plan,
			}));
		}
	}
	Ok(None)
}

async fn prepare(
	tx: &mut WriteTransaction,
	id: ConversationId,
	mut plan: LaunchPlan,
	now: i64,
) -> Result<(), CoreError> {
	let mut queue = turn_queue::load(tx, id).await?;
	let actor = Actor::InteractiveClient {
		client_id: queue.entries[0].turn.client_id,
	};
	let CommandOutcome::RunCreated(run) =
		crate::command::create_run(tx, &actor, id, now).await?
	else {
		unreachable!("Run created")
	};
	let entry = queue.claim(run.run_id).expect("pending input checked");
	plan.prompt = entry.prompt.clone();
	plan.turn_id = Some(entry.turn.turn_id);
	plan.client_id = entry.turn.client_id;
	turn_queue::changed(tx, &actor, id, &entry, now).await?;
	turn_queue::save(tx, id, &queue).await?;
	crate::run_command::install(
		tx,
		&actor,
		CommandId(entry.command_id),
		run,
		plan,
		now,
	)
	.await?;
	Ok(())
}
