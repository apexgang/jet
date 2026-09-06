//! Durable start Effects dispatch once; process observations drive Run state.
use crate::effect::{Effect, EffectAdapter, EffectKind, EffectResult};
use crate::{Core, CoreError, RunId, RunLifecycle};
use crate::{
	run_command::LaunchPlan,
	run_state::{self, Observation},
};
use jet_store::EffectKindRecord;
use std::{sync::Arc, time::Duration};

struct Runs<'a>(&'a Arc<Core>);
impl Core {
	/// Dispatches pending managed Run Effects after commit. Started executions
	/// continue asynchronously; a lost acknowledgement is never guessed or retried.
	///
	/// # Errors
	/// Returns a store error if an Effect or its observation cannot be recorded.
	pub async fn perform_runs(self: &Arc<Self>) -> Result<(), CoreError> {
		self.reconcile_effects(&mut Runs(self), EffectKindRecord::StartRun)
			.await?;
		Ok(())
	}
}
impl EffectAdapter for Runs<'_> {
	async fn execute(&mut self, effect: &Effect) -> EffectResult {
		let EffectKind::StartRun { run_id } = effect.kind else {
			return EffectResult::Unknown;
		};
		let record = match self
			.0
			.store
			.read(async |tx| tx.run_execution(run_id.0).await)
			.await
		{
			Ok(Some(record)) => record,
			Ok(None) | Err(_) => return EffectResult::Unknown,
		};
		let plan: LaunchPlan = match run_state::decode(&record.plan) {
			Ok(plan) => plan,
			Err(_) => return EffectResult::Unknown,
		};
		if plan.revalidate().await.is_err() {
			return EffectResult::Failed;
		}
		let Some(host) = &self.0.run_host else {
			return EffectResult::Failed;
		};
		let mut connection = match host
			.start(self.0.run_home(), run_id, plan)
			.await
		{
			Ok(connection) => connection,
			Err(crate::RunStartError::NotStarted) => {
				return EffectResult::Failed;
			}
			Err(crate::RunStartError::Unknown) => return EffectResult::Unknown,
		};
		let first =
			tokio::time::timeout(Duration::from_secs(10), connection.receive())
				.await;
		let observation = match first {
			Ok(Ok(
				observation @ (Observation::Started { .. }
				| Observation::LaunchFailed),
			)) => observation,
			_ => return EffectResult::Unknown,
		};
		let failed = matches!(observation, Observation::LaunchFailed);
		if self.0.observe_run(run_id, observation).await.is_err() {
			return EffectResult::Unknown;
		}
		let core = Arc::clone(self.0);
		tokio::spawn(async move {
			if monitor(&core, run_id, connection, failed).await.is_err()
				&& let Err(error) =
					core.observe_run(run_id, Observation::Disconnected).await
			{
				eprintln!("jetd: cannot record Run disconnection: {error}");
			}
		});
		EffectResult::Completed
	}
	async fn reconcile(&mut self, effect: &Effect) -> EffectResult {
		let EffectKind::StartRun { run_id } = effect.kind else {
			return EffectResult::Unknown;
		};
		match self.0.store.read(async |tx| tx.run(run_id.0).await).await {
			Ok(Some(run))
				if matches!(
					run.lifecycle,
					RunLifecycle::Active | RunLifecycle::Stopping
				) || run.lifecycle.is_terminal() =>
			{
				EffectResult::Completed
			}
			Ok(Some(_)) | Ok(None) | Err(_) => EffectResult::Unknown,
		}
	}
}

async fn monitor(
	core: &Core,
	run_id: RunId,
	mut connection: Box<dyn crate::RunConnection>,
	mut ended: bool,
) -> Result<(), CoreError> {
	loop {
		let observation = connection.receive().await?;
		match observation {
			Observation::Progress(offset) => {
				core.observe_run(run_id, Observation::Progress(offset))
					.await?;
				connection.acknowledge(offset).await?;
				if ended {
					connection.finish().await?;
					return Ok(());
				}
			}
			observation => {
				ended |= matches!(observation, Observation::Ended(_));
				core.observe_run(run_id, observation).await?;
			}
		}
	}
}
