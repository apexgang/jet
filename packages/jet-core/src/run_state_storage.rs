//! Serialization and snapshot access for managed execution state.

use jet_store::{ReadTransaction, WriteTransaction};

use crate::event::EventSubject;
use crate::run_state::State;
use crate::{
	ConversationId, CoreError, EventKind, EventSequence, Run, RunActivity,
	RunExecution, RunId,
};

pub(crate) async fn snapshot(
	tx: &mut ReadTransaction,
	run_id: RunId,
) -> Result<RunExecution, CoreError> {
	let run = tx.run(run_id.0).await?.ok_or_else(missing)?;
	let record = tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
	let state: State = decode(&record.state)?;
	let plan: crate::LaunchPlan = decode(&record.plan)?;
	Ok(RunExecution {
		needs_attention: tx.orphaned_execution(run_id.0).await?.is_some(),
		visa: plan.visa.filter(|_| plan.no_visa.is_none()),
		no_visa: plan.no_visa,
		cursor: EventSequence(tx.event_cursor().await?),
		run: run.into(),
		activity: if state.disconnected {
			Some(RunActivity::Reconnecting)
		} else {
			state.activity
		},
		processes: state.processes,
		native_conversation: state.native_conversation,
		exit_code: state.exit_code,
		termination: state.termination,
	})
}

pub(crate) async fn save(
	tx: &mut WriteTransaction,
	run_id: RunId,
	state: &State,
) -> Result<(), CoreError> {
	tx.update_run_execution(
		run_id.0,
		&serde_json::to_string(state)
			.map_err(|e| CoreError::internal("run.encode", e.to_string()))?,
	)
	.await?;
	Ok(())
}

pub(crate) async fn append(
	tx: &mut WriteTransaction,
	actor: &crate::EventActor,
	run: &Run,
	event: EventKind,
	now: i64,
) -> Result<(), CoreError> {
	let record = event.to_record_as(
		actor.clone(),
		EventSubject::Run {
			conversation_id: ConversationId(run.conversation_id.0),
			run_id: run.run_id,
		},
		now,
	)?;
	tx.append_event(record).await?;
	Ok(())
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(
	json: &str,
) -> Result<T, CoreError> {
	serde_json::from_str(json)
		.map_err(|e| CoreError::internal("run.invalid_record", e.to_string()))
}

fn missing() -> CoreError {
	CoreError::not_found(
		"run.execution_not_found",
		"the managed Run does not exist",
	)
}
