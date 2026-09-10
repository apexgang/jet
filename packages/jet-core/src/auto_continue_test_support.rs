//! Driven Harness and clock for Auto-continue tests at Commands/Queries/Events.
use super::*;
use crate::test_support::{FixedProbe, ManualClock, equipped, start_core_with};
use crate::{
	CoreError, LaunchPlan, PinnedCraft, RunConnection, RunFuture, RunHost,
	RunId, RunStartError,
};
use std::{
	path::Path,
	sync::{Arc, Mutex},
	time::{Duration, UNIX_EPOCH},
};
use tokio::sync::mpsc;

pub(super) struct Harness {
	pub core: Arc<Core>,
	pub clock: Arc<ManualClock>,
	pub id: ConversationId,
	pub run_id: RunId,
	pub binding: crate::AccountBindingId,
	pub host: Arc<Host>,
	sender: mpsc::Sender<RunObservation>,
	offset: u64,
}
impl Harness {
	pub async fn start(dir: &Path, selected: AutoContinuePolicy) -> Self {
		let clock =
			ManualClock::at(UNIX_EPOCH + Duration::from_secs(1_700_000_000));
		let (sender, receiver) = mpsc::channel(32);
		let host = Arc::new(Host {
			receiver: Arc::new(tokio::sync::Mutex::new(receiver)),
			launches: Mutex::default(),
			inputs: Arc::default(),
		});
		let core = Arc::new(
			start_core_with(
				&dir.join("plane.sqlite3"),
				clock.clone(),
				FixedProbe::new(equipped()),
			)
			.await
			.with_run_host(host.clone()),
		);
		let binding =
			bind_native_account(&core, ProviderId("anthropic".into())).await;
		core.execute(
			&actor(),
			request(Command::SetAutoContinue {
				target: AutoContinueTarget::AccountBinding(binding),
				policy: selected,
			}),
		)
		.await
		.unwrap();
		let project_id = register_repository(&core, &dir.join("repo")).await;
		let CommandOutcome::ConversationCreated(c) = core
			.execute(
				&actor(),
				request(Command::CreateConversation {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRequest::LocalCheckout {
						project_id,
					},
				}),
			)
			.await
			.unwrap()
		else {
			panic!("Conversation")
		};
		let QueryResult::Status(status) =
			core.query(&actor(), Query::Status).await.unwrap()
		else {
			panic!("Status")
		};
		let CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::StartVisaRun(VisaRunRequest {
					conversation_id: c.conversation_id,
					destination_plane_id: status.plane_id,
					account_binding_id: binding,
					craft: "fake".into(),
					prompt: "Work".into(),
				})),
			)
			.await
			.unwrap()
		else {
			panic!("Run")
		};
		core.perform_runs().await.unwrap();
		let mut harness = Self {
			core,
			clock,
			id: c.conversation_id,
			run_id: run.run_id,
			binding,
			host,
			sender,
			offset: 0,
		};
		harness
			.send(vec![RunObservation::Model(crate::ModelId(
				"original-model".into(),
			))])
			.await;
		harness
	}
	pub async fn send(&mut self, observations: Vec<RunObservation>) {
		let marker = format!("batch-{}", self.offset);
		for observation in observations {
			self.sender.send(observation).await.unwrap();
		}
		self.sender
			.send(RunObservation::RunTitle(marker.clone()))
			.await
			.unwrap();
		self.offset += 1;
		self.sender
			.send(RunObservation::Progress {
				offset: self.offset,
				checkpoint: String::new(),
			})
			.await
			.unwrap();
		tokio::time::timeout(Duration::from_secs(5), async {
			loop {
				let QueryResult::RunExecution(value) = self
					.core
					.query(
						&actor(),
						Query::RunExecution {
							run_id: self.run_id,
						},
					)
					.await
					.unwrap()
				else {
					panic!("Run")
				};
				if value.run.name.value == marker {
					break;
				}
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap();
	}
	pub async fn limited(&mut self, reset: Option<u64>) {
		let mut usage = exhausted();
		usage.resets_in_seconds = reset;
		let turn_id = self
			.host
			.inputs
			.lock()
			.unwrap()
			.last()
			.map(|(id, _)| *id)
			.unwrap_or_else(|| {
				self.host
					.launches
					.lock()
					.unwrap()
					.last()
					.unwrap()
					.turn_id
					.unwrap()
			});
		self.send(vec![
			RunObservation::Usage(UsageReport::ProviderQuota(usage)),
			RunObservation::Activity(RunActivity::WaitingForQuota),
			RunObservation::TurnCompleted {
				turn_id,
				native_conversation: "native-1".into(),
			},
		])
		.await;
	}

	pub async fn finish(&mut self) {
		self.sender
			.send(RunObservation::Ended(Some(1)))
			.await
			.unwrap();
		self.offset += 1;
		self.sender
			.send(RunObservation::Progress {
				offset: self.offset,
				checkpoint: String::new(),
			})
			.await
			.unwrap();
		tokio::time::timeout(Duration::from_secs(5), async {
			loop {
				let QueryResult::RunExecution(value) = self
					.core
					.query(
						&actor(),
						Query::RunExecution {
							run_id: self.run_id,
						},
					)
					.await
					.unwrap()
				else {
					panic!("Run")
				};
				if value.run.lifecycle.is_terminal() {
					break;
				}
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap();
	}
	pub async fn restart(&mut self, dir: &Path) {
		self.core.close().await;
		self.core = Arc::new(
			start_core_with(
				&dir.join("plane.sqlite3"),
				self.clock.clone(),
				FixedProbe::new(equipped()),
			)
			.await
			.with_run_host(self.host.clone()),
		);
	}
	pub async fn tick(&self, seconds: u64) {
		self.clock.advance(Duration::from_secs(seconds));
		self.core.perform_runs().await.unwrap();
		// The monitor wakes on another task, and its response is observable input.
		tokio::time::sleep(Duration::from_millis(50)).await;
	}
}
#[derive(Debug)]
pub(super) struct Host {
	receiver: Arc<tokio::sync::Mutex<mpsc::Receiver<RunObservation>>>,
	pub launches: Mutex<Vec<LaunchPlan>>,
	pub inputs: Arc<Mutex<Vec<(uuid::Uuid, String)>>>,
}
impl RunHost for Host {
	fn native_provider(
		&self,
		_: &PinnedCraft,
	) -> Result<ProviderId, CoreError> {
		Ok(ProviderId("anthropic".into()))
	}
	fn pin(
		&self,
		_: std::path::PathBuf,
		_: String,
	) -> RunFuture<'_, Result<PinnedCraft, CoreError>> {
		Box::pin(async {
			use sha2::{Digest, Sha256};
			let executable = Path::new("/bin/cat").canonicalize().unwrap();
			Ok(PinnedCraft {
				id: "fake".into(),
				sha256: format!(
					"{:x}",
					Sha256::digest(std::fs::read(&executable).unwrap())
				),
				executable,
				adapter_state: "pinned-model".into(),
			})
		})
	}
	fn prepare_retry_run(
		&self,
		plan: LaunchPlan,
	) -> RunFuture<'_, Result<LaunchPlan, CoreError>> {
		Box::pin(async { Ok(plan) })
	}
	fn prepare_next_run(
		&self,
		mut plan: LaunchPlan,
	) -> RunFuture<'_, Result<LaunchPlan, CoreError>> {
		// A default changed after admission; automatic retries must preserve the pin.
		plan.craft.adapter_state = "different-model".into();
		Box::pin(async { Ok(plan) })
	}
	fn start(
		&self,
		_: std::path::PathBuf,
		_: RunId,
		plan: LaunchPlan,
	) -> RunFuture<'_, Result<Box<dyn RunConnection>, RunStartError>> {
		self.launches.lock().unwrap().push(plan);
		Box::pin(async {
			Ok(Box::new(Connection {
				receiver: self.receiver.clone(),
				inputs: self.inputs.clone(),
				started: std::sync::atomic::AtomicBool::new(false),
			}) as Box<dyn RunConnection>)
		})
	}
}
struct Connection {
	inputs: Arc<Mutex<Vec<(uuid::Uuid, String)>>>,
	receiver: Arc<tokio::sync::Mutex<mpsc::Receiver<RunObservation>>>,
	started: std::sync::atomic::AtomicBool,
}
impl RunConnection for Connection {
	fn submit_turn(
		&self,
		turn_id: uuid::Uuid,
		prompt: String,
		_: crate::ChildWork,
	) -> RunFuture<'_, Result<(), CoreError>> {
		self.inputs.lock().unwrap().push((turn_id, prompt));
		Box::pin(async { Ok(()) })
	}
	fn supports_native_cancellation(&self) -> bool {
		false
	}
	fn interrupt(&self, _: uuid::Uuid) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async { panic!("no interruption") })
	}
	fn decide_approval<'a>(
		&'a self,
		_: &'a str,
		_: crate::ReviewDecision,
	) -> RunFuture<'a, Result<(), CoreError>> {
		Box::pin(async { panic!("no approval") })
	}
	fn acknowledge(&self, _: u64) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async { Ok(()) })
	}
	fn finish(&self) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async { Ok(()) })
	}

	#[expect(
		clippy::await_holding_invalid_type,
		reason = "the test driver serializes reads from its shared observation stream"
	)]
	fn receive(&self) -> RunFuture<'_, Result<RunObservation, CoreError>> {
		Box::pin(async {
			if !self.started.swap(true, std::sync::atomic::Ordering::SeqCst) {
				return Ok(RunObservation::Started {
					helper_pid: 100,
					harness_pid: 101,
				});
			}
			self.receiver.lock().await.recv().await.ok_or_else(|| {
				CoreError::internal("fixture.closed", "fixture closed")
			})
		})
	}
}
