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
		self.perform_execution_resolutions().await?;
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
		let core = Arc::clone(self.0);
		spawn_monitor(core, run_id, connection, vec![observation]);
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

#[expect(
	clippy::await_holding_invalid_type,
	reason = "failure settlement and monitor removal serialize with adoption and launch Effects"
)]
pub(crate) fn spawn_monitor(
	core: Arc<Core>,
	run_id: RunId,
	connection: Box<dyn crate::RunConnection>,
	pending: Vec<Observation>,
) {
	core.run_recovery
		.monitors
		.lock()
		.expect("monitor lock")
		.insert(run_id);
	tokio::spawn(async move {
		if monitor(&core, run_id, connection, pending).await.is_err() {
			let _gate = core.effect_reconciliation.lock().await;
			let _ = core.observe_run(run_id, Observation::Disconnected).await;
			if core.run_recovery.failed(run_id) {
				let _ = core.mark_orphan(run_id).await;
			}
			core.run_recovery
				.monitors
				.lock()
				.expect("monitor lock")
				.remove(&run_id);
			return;
		}
		core.run_recovery
			.monitors
			.lock()
			.expect("monitor lock")
			.remove(&run_id);
	});
}

async fn monitor(
	core: &Core,
	run_id: RunId,
	mut connection: Box<dyn crate::RunConnection>,
	initial: Vec<Observation>,
) -> Result<(), CoreError> {
	let mut expected = core.source_prefix(run_id).await?;
	let mut replayed = run_state::SourcePrefix::default();
	let mut initial = initial.into_iter();
	let mut pending = Vec::new();
	let mut bytes = 0;
	let mut events = 0;
	let mut ended = false;
	let mut deadline = None;
	loop {
		let observation = if let Some(observation) = initial.next() {
			observation
		} else {
			let receive = connection.receive();
			tokio::pin!(receive);
			loop {
				let Some(at) = deadline else {
					break receive.await?;
				};
				tokio::select! {
					result = &mut receive => break result?,
					() = tokio::time::sleep_until(at) => {
						core.commit_run_source(run_id, std::mem::take(&mut pending), run_state::SourceBoundary::Pending).await?;
						bytes = 0; events = 0; deadline = None;
					}
				}
			}
		};
		match observation {
			Observation::Progress { offset, checkpoint } => {
				if replayed.count < expected.count {
					return Err(batch_mismatch());
				}
				core.commit_run_source(
					run_id,
					std::mem::take(&mut pending),
					run_state::SourceBoundary::Complete { offset, checkpoint },
				)
				.await?;
				connection.acknowledge(offset).await?;
				core.run_recovery.progressed(run_id);
				expected = run_state::SourcePrefix::default();
				replayed = run_state::SourcePrefix::default();
				bytes = 0;
				events = 0;
				deadline = None;
				if ended {
					connection.finish().await?;
					return Ok(());
				}
			}
			observation => {
				ended |= matches!(
					observation,
					Observation::Ended(_) | Observation::LaunchFailed
				);
				if replayed.count < expected.count {
					replayed.include(&observation)?;
					if replayed.count == expected.count
						&& replayed.digest != expected.digest
					{
						return Err(batch_mismatch());
					}
					continue;
				}
				let size = match &observation {
					Observation::Output {
						native_json,
						presentation_json,
					} => {
						native_json.len()
							+ presentation_json
								.iter()
								.map(String::len)
								.sum::<usize>()
					}
					Observation::NativeConversation(value) => value.len(),
					_ => 0,
				};
				let count = event_count(&observation);
				if !pending.is_empty()
					&& (bytes + size > 256 * 1024 || events + count > 64)
				{
					core.commit_run_source(
						run_id,
						std::mem::take(&mut pending),
						run_state::SourceBoundary::Pending,
					)
					.await?;
					bytes = 0;
					events = 0;
					deadline = None;
				}
				bytes += size;
				events += count;
				pending.push(observation);
				deadline.get_or_insert_with(|| {
					tokio::time::Instant::now() + Duration::from_millis(50)
				});
			}
		}
	}
}
fn batch_mismatch() -> CoreError {
	CoreError::invalid_input(
		"run.source_changed",
		"replayed source did not match its committed semantic prefix",
	)
}

fn event_count(observation: &Observation) -> usize {
	match observation {
		Observation::Started { .. }
		| Observation::Ended(_)
		| Observation::Lost => 3,
		Observation::Progress { .. } => 0,
		Observation::Activity(_)
		| Observation::Output { .. }
		| Observation::NativeConversation(_)
		| Observation::LaunchFailed
		| Observation::Disconnected
		| Observation::Reconnected => 1,
	}
}
