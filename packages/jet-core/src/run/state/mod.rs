//! Atomic projection and semantic Event updates for managed executions.
mod projection;
use crate::{
	ConversationId, Core, CoreError, EventKind, ManagedProcess, RunActivity,
	RunId, turn::dispatch::Settlement,
};
use jet_store::WriteTransaction;
use projection::{apply, executing};
use serde::{Deserialize, Serialize};

pub(crate) use crate::run::observation::{
	Observation, SourceBoundary, SourcePrefix,
};
pub(crate) use crate::run::state_storage::{append, decode, save, snapshot};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct State {
	#[serde(default)]
	pub(crate) model: Option<crate::ModelId>,
	#[serde(default)]
	pub(crate) current_turn: Option<uuid::Uuid>,
	#[serde(default)]
	pub(crate) quota: Vec<crate::auto_continue::work::Condition>,
	#[serde(default)]
	pub(crate) quota_wait: bool,
	/// Durable limits and reservations for Automatic review.
	#[serde(default)]
	pub(crate) review: crate::review::guard::Guard,
	#[serde(default)]
	pub(crate) changes: Option<crate::checkpoint::state::Tracking>,
	pub(crate) activity: Option<RunActivity>,
	#[serde(default)]
	pub(crate) disconnected: bool,
	pub(crate) processes: Vec<ManagedProcess>,
	pub(crate) native_conversation: Option<String>,
	pub(crate) exit_code: Option<i32>,
	#[serde(default)]
	pub(crate) source_offset: u64,
	#[serde(default)]
	pub(crate) checkpoint: String,
	#[serde(default)]
	pub(crate) partial_source: SourcePrefix,
	/// The admitted interactive control request, kept until the execution
	/// settles it (ADR-0083).
	#[serde(default)]
	pub(crate) control: Option<crate::run::execution_control::ControlRequest>,
	/// The recorded terminal outcome of that request.
	#[serde(default)]
	pub(crate) termination: Option<crate::RunTermination>,
}

impl Core {
	pub(crate) async fn source_prefix(
		&self,
		run_id: RunId,
	) -> Result<SourcePrefix, CoreError> {
		self.store
			.read(async |tx| {
				let record =
					tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
				Ok(decode::<State>(&record.state)?.partial_source)
			})
			.await
	}
	pub(crate) async fn commit_run_source(
		&self,
		run_id: RunId,
		observations: Vec<Observation>,
		boundary: SourceBoundary,
	) -> Result<(), CoreError> {
		let now = self.now_unix_ms();
		let terminal = self.store.write(async |tx| {
            let execution = tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
            let state: State = decode(&execution.state)?;
            if matches!(&boundary, SourceBoundary::Complete { offset, .. } if *offset <= state.source_offset) { return Err(invalid()); }
            let mut prefix = state.partial_source;
            for observation in observations {
                prefix.include(&observation)?;
                crate::checkpoint::state::observe(self, tx, run_id, &observation).await?;
                record(tx, run_id, observation, now).await?;
            }
            // ADR-0071: bounded groups commit with their replay prefix. Source
            // remains retained until its final parser checkpoint commits.
            if let SourceBoundary::Complete { offset, checkpoint } = boundary {
                record(tx, run_id, Observation::Progress { offset, checkpoint }, now).await?;
                prefix = SourcePrefix::default();
            }
            let execution = tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
            let mut state: State = decode(&execution.state)?;
            state.partial_source = prefix;
            tx.update_run_execution(run_id.0, &serde_json::to_string(&state).map_err(|_| invalid())?).await?;
            if state.partial_source.count == 0 { crate::auto_continue::work::consider(tx, run_id, now).await?; }
            let terminal = tx.run(run_id.0).await?.ok_or_else(missing)?.lifecycle.is_terminal();
            Ok::<_, CoreError>(terminal && state.partial_source.count == 0)
        }).await?;
		// A delivery admitted in a partial source batch waits for the parser's
		// complete boundary before touching the live checkout.
		self.utility_wake.notify_one();
		if terminal {
			self.turn_wake.send_replace(());
			self.maintenance_work.notify_one();
			self.run_work.notify_one();
		}
		Ok(())
	}
	pub(crate) async fn observe_run(
		&self,
		run_id: RunId,
		observation: Observation,
	) -> Result<(), CoreError> {
		let now = self.now_unix_ms();
		let terminal = matches!(
			observation,
			Observation::Lost
				| Observation::LaunchFailed
				| Observation::Ended(_)
		);
		self.store
			.write(async |tx| {
				crate::checkpoint::state::observe(
					self,
					tx,
					run_id,
					&observation,
				)
				.await?;
				record(tx, run_id, observation, now).await
			})
			.await?;
		if terminal {
			self.turn_wake.send_replace(());
			self.maintenance_work.notify_one();
			self.run_work.notify_one();
		}
		Ok(())
	}
}

async fn record(
	tx: &mut WriteTransaction,
	run_id: RunId,
	observation: Observation,
	now: i64,
) -> Result<(), CoreError> {
	let record = tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
	let plan: crate::LaunchPlan = decode(&record.plan)?;
	let authorized_by = plan.client_id;
	let mut state: State = decode(&record.state)?;
	let run = tx.run(run_id.0).await?.ok_or_else(missing)?;
	if matches!(
		&observation,
		Observation::ConversationTitle(_)
			| Observation::RunTitle(_)
			| Observation::ProcessTitle { .. }
	) && !executing(run.lifecycle)
	{
		return Err(invalid());
	}
	let actor = match &observation {
		Observation::FileChanged(_)
		| Observation::TurnStarted
		| Observation::TurnEnded(_)
		| Observation::Completed(_)
		| Observation::Activity(_)
		| Observation::TurnCompleted { .. }
		| Observation::Output { .. }
		| Observation::Model(_)
		| Observation::Usage(_)
		| Observation::NativeConversation(_)
		| Observation::ConversationTitle(_)
		| Observation::RunTitle(_)
		| Observation::ApprovalRequested(_)
		| Observation::ProcessTitle { .. } => crate::EventActor::Harness {
			run_id,
			authorized_by,
		},
		Observation::Started { .. }
		| Observation::Progress { .. }
		| Observation::Ended(_)
		| Observation::LaunchFailed
		| Observation::Lost
		| Observation::Reconnected
		| Observation::Disconnected => crate::EventActor::RunSupervisor {
			run_id,
			authorized_by,
		},
	};
	let observation = match observation {
		Observation::ConversationTitle(title) => {
			crate::conversation::name::apply_harness_conversation(
				tx,
				&actor,
				ConversationId(run.conversation_id),
				run_id,
				title,
				now,
			)
			.await?;
			return Ok(());
		}
		Observation::RunTitle(title) => {
			crate::conversation::name::apply_harness_run(
				tx, &actor, run_id, title, now,
			)
			.await?;
			return Ok(());
		}
		Observation::Model(model) => {
			if !executing(run.lifecycle)
				|| model.0.is_empty()
				|| model.0.len() > 128
				|| model.0.chars().any(char::is_control)
			{
				return Err(invalid());
			}
			state.model = Some(model);
			save(tx, run_id, &state).await?;
			return Ok(());
		}

		// A Usage record is durable Plane state of its own rather than
		// execution state, so it is written beside the Run instead of
		// changing its lifecycle (ADR-0023).
		Observation::Usage(report) => {
			if let crate::UsageReport::ProviderQuota(usage) = &report {
				state.quota.retain(|previous| {
					previous.usage.window != usage.window
						|| previous.usage.scope != usage.scope
				});
				if state.quota.len() < 128 {
					state.quota.push(crate::auto_continue::work::Condition {
						usage: usage.clone(),
						observed_at: now,
						turn_id: state
							.current_turn
							.or(plan.turn_id)
							.unwrap_or(run_id.0),
					});
				}
				save(tx, run_id, &state).await?;
			}
			crate::usage::record::record(
				tx,
				&actor,
				&run.clone().into(),
				plan.visa.map(|selection| selection.account_binding_id),
				report,
				now,
			)
			.await?;
			return Ok(());
		}
		other => other,
	};
	let settlement = match &observation {
		Observation::TurnCompleted { turn_id, .. } => {
			Some(Settlement::Completed { turn_id: *turn_id })
		}
		Observation::NativeConversation(_) | Observation::Completed(_) => {
			Some(Settlement::Completed {
				turn_id: plan.turn_id.unwrap_or(run_id.0),
			})
		}
		Observation::Ended(_)
		| Observation::LaunchFailed
		| Observation::Lost => Some(Settlement::Failed),
		Observation::Disconnected | Observation::Reconnected => {
			Some(Settlement::OutcomeUnknown)
		}
		// A cancelled turn releases the queue: the Run itself is still
		// alive and takes the next admitted input (ADR-0083).
		Observation::TurnEnded(crate::TurnOutcome::Interrupted)
			if state.control.is_some_and(|request| {
				request.control == crate::RunControl::InterruptTurn
			}) =>
		{
			Some(Settlement::Canceled)
		}
		Observation::Started { .. }
		| Observation::FileChanged(_)
		| Observation::TurnStarted
		| Observation::TurnEnded(_)
		| Observation::Activity(_)
		| Observation::Output { .. }
		| Observation::Model(_)
		| Observation::Usage(_)
		| Observation::ProcessTitle { .. }
		| Observation::ConversationTitle(_)
		| Observation::RunTitle(_)
		| Observation::ApprovalRequested(_)
		| Observation::Progress { .. } => None,
	};
	let (lifecycle, events) = apply(run.lifecycle, &mut state, observation)?;
	if let Some(outcome) = settlement {
		crate::turn::dispatch::settle(tx, run_id, outcome, now).await?;
	}
	let event_run: crate::Run = run.clone().into();
	if lifecycle != run.lifecycle {
		let from = run.lifecycle;
		tx.update_run_lifecycle(run_id.0, lifecycle, now).await?;
		append(
			tx,
			&actor,
			&event_run,
			EventKind::RunLifecycleChanged {
				from,
				to: lifecycle,
			},
			now,
		)
		.await?;
	} else if !events.is_empty() {
		tx.update_run_lifecycle(run_id.0, lifecycle, now).await?;
	}
	for event in events {
		append(tx, &actor, &event_run, event, now).await?;
	}
	save(tx, run_id, &state).await?;
	Ok::<_, CoreError>(())
}

pub(crate) async fn settle_start(
	tx: &mut WriteTransaction,
	run_id: RunId,
	state: jet_store::EffectStateRecord,
	now: i64,
) -> Result<(), CoreError> {
	if tx.run_execution(run_id.0).await?.is_some()
		&& state == jet_store::EffectStateRecord::Failed
	{
		record(tx, run_id, Observation::LaunchFailed, now).await?;
	}
	Ok(())
}

fn missing() -> CoreError {
	CoreError::not_found(
		"run.execution_not_found",
		"the managed Run does not exist",
	)
}
fn invalid() -> CoreError {
	CoreError::conflict(
		"run.invalid_observation",
		"the Craft observation conflicts with the Run lifecycle",
	)
}
