//! Power changes arrive at the machine probe, without a Command waking the Run.
use super::{AdmissionHost, install_craft};
use crate::test_support::{
	FixedProbe, actor, equipped, request, start_core_with,
};
use crate::*;
use pretty_assertions::assert_eq;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

#[derive(Debug, PartialEq, Eq)]
enum Delivery {
	Constraint(ChildWork),
	Turn(Uuid),
}

#[tokio::test]
async fn a_transient_failed_power_probe_cannot_strand_a_quiet_runs_turn() {
	let dir = tempfile::tempdir().unwrap();
	let (core, probe, observations, mut deliveries, id) =
		start(dir.path(), 0).await;
	let pending = queue(&core, id).await;
	core.constrain_child_work().await.unwrap();
	assert_eq!(
		deliveries.recv().await,
		Some(Delivery::Constraint(ChildWork::Native))
	);
	let mut power = equipped();
	power.power = jet_runtime::PowerState::Unavailable;
	probe.answer_with(power);
	complete(&observations).await;
	tokio::time::timeout(Duration::from_secs(5), async {
		loop {
			let QueryResult::TurnQueue(queue) = core
				.query(
					&actor(),
					Query::TurnQueue {
						conversation_id: id,
					},
				)
				.await
				.unwrap()
			else {
				panic!("queue")
			};
			if queue.turns.len() == 1 {
				assert_eq!(queue.turns[0].turn_id, pending);
				assert_eq!(queue.turns[0].state, TurnState::Queued);
				break;
			}
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	})
	.await
	.unwrap();
	// Keep the Run quiet after its completion boundary. Only admission sees the
	// transient failure; both surrounding maintenance samples see normal power.
	tokio::time::sleep(Duration::from_millis(50)).await;
	probe.answer_with(equipped());
	core.constrain_child_work().await.unwrap();
	tokio::time::timeout(Duration::from_secs(2), async {
		loop {
			if deliveries.recv().await == Some(Delivery::Turn(pending)) {
				break;
			}
		}
	})
	.await
	.expect("power recovery must wake the silent Run without another Command");
}

#[tokio::test]
async fn a_turn_receives_the_new_child_constraint_before_native_delivery() {
	let dir = tempfile::tempdir().unwrap();
	let (core, probe, observations, mut deliveries, id) =
		start(dir.path(), 1).await;
	let pending = queue(&core, id).await;
	let mut power = equipped();
	power.power = jet_runtime::PowerState::Constrained;
	probe.answer_with(power);
	complete(&observations).await;
	let first = tokio::time::timeout(Duration::from_secs(5), deliveries.recv())
		.await
		.unwrap();
	assert_eq!(first, Some(Delivery::Constraint(ChildWork::Paused)));
	assert_eq!(deliveries.recv().await, Some(Delivery::Turn(pending)));
}

async fn queue(core: &Core, id: ConversationId) -> Uuid {
	let CommandOutcome::TurnAdmitted(turn) = core
		.execute(
			&actor(),
			request(Command::SubmitTurn {
				conversation_id: id,
				source: TurnSource::Schedule,
				prompt: "Continuation".into(),
			}),
		)
		.await
		.unwrap()
	else {
		panic!("queued turn")
	};
	turn.turn_id
}
async fn complete(observations: &mpsc::Sender<RunObservation>) {
	observations
		.send(RunObservation::Completed("native".into()))
		.await
		.unwrap();
	observations
		.send(RunObservation::Progress {
			offset: 1,
			checkpoint: String::new(),
		})
		.await
		.unwrap();
}

type Fixture = (
	Arc<Core>,
	Arc<FixedProbe>,
	mpsc::Sender<RunObservation>,
	mpsc::UnboundedReceiver<Delivery>,
	ConversationId,
);
async fn start(home: &std::path::Path, reduced: u32) -> Fixture {
	let probe = FixedProbe::new(equipped());
	let (observations, receiver) = mpsc::channel(16);
	let (deliveries, delivered) = mpsc::unbounded_channel();
	let core = Arc::new(
		start_core_with(
			&home.join("plane.sqlite3"),
			Arc::new(SystemClock),
			probe.clone(),
		)
		.await
		.with_run_host(Arc::new(Host(Mutex::new(Some(receiver)), deliveries))),
	);
	install_craft(home);
	core.execute(
		&actor(),
		request(Command::SetSetting {
			scope: SettingScope::Plane,
			key: SettingKey::EnergyLowPowerConcurrency,
			value: SettingValue::Count(reduced),
		}),
	)
	.await
	.unwrap();
	let project_id =
		crate::test_support::register_repository(&core, &home.join("repo"))
			.await;
	let CommandOutcome::ConversationCreated(conversation) = core
		.execute(
			&actor(),
			request(Command::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRequest::LocalCheckout { project_id },
			}),
		)
		.await
		.unwrap()
	else {
		panic!("Conversation")
	};
	let id = conversation.conversation_id;
	core.execute(
		&actor(),
		request(Command::StartRun {
			conversation_id: id,
			craft: "fake".into(),
			prompt: "Start".into(),
		}),
	)
	.await
	.unwrap();
	core.perform_runs().await.unwrap();
	(core, probe, observations, delivered, id)
}
#[derive(Debug)]
struct Host(
	Mutex<Option<mpsc::Receiver<RunObservation>>>,
	mpsc::UnboundedSender<Delivery>,
);
impl RunHost for Host {
	fn pin(
		&self,
		home: PathBuf,
		id: String,
	) -> RunFuture<'_, Result<PinnedCraft, CoreError>> {
		AdmissionHost.pin(home, id)
	}
	fn prepare_next_run(
		&self,
		plan: LaunchPlan,
	) -> RunFuture<'_, Result<LaunchPlan, CoreError>> {
		Box::pin(async { Ok(plan) })
	}
	fn start(
		&self,
		_home: PathBuf,
		_id: RunId,
		_plan: LaunchPlan,
	) -> RunFuture<'_, Result<Box<dyn RunConnection>, RunStartError>> {
		Box::pin(async {
			Ok(Box::new(Connection {
				observations: Mutex::new(self.0.lock().await.take().unwrap()),
				deliveries: self.1.clone(),
				started: std::sync::atomic::AtomicBool::new(false),
			}) as Box<dyn RunConnection>)
		})
	}
}
struct Connection {
	observations: Mutex<mpsc::Receiver<RunObservation>>,
	deliveries: mpsc::UnboundedSender<Delivery>,
	started: std::sync::atomic::AtomicBool,
}
impl RunConnection for Connection {
	fn constrain_children(
		&self,
		work: ChildWork,
	) -> RunFuture<'_, Result<(), CoreError>> {
		self.deliveries.send(Delivery::Constraint(work)).unwrap();
		Box::pin(async { Ok(()) })
	}
	fn submit_turn(
		&self,
		id: Uuid,
		_prompt: String,
		child_work: ChildWork,
	) -> RunFuture<'_, Result<(), CoreError>> {
		self.deliveries
			.send(Delivery::Constraint(child_work))
			.unwrap();
		self.deliveries.send(Delivery::Turn(id)).unwrap();
		Box::pin(async { Ok(()) })
	}
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "one external observation stream is serialized by its receiver"
	)]
	fn receive(&self) -> RunFuture<'_, Result<RunObservation, CoreError>> {
		Box::pin(async {
			if !self.started.swap(true, std::sync::atomic::Ordering::SeqCst) {
				return Ok(RunObservation::Started {
					helper_pid: 100,
					harness_pid: 101,
				});
			}
			Ok(self.observations.lock().await.recv().await.unwrap())
		})
	}
	fn supports_native_cancellation(&self) -> bool {
		false
	}
	fn interrupt(
		&self,
		_turn_id: Uuid,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async { panic!("no cancellation requested") })
	}
	fn decide_approval<'a>(
		&'a self,
		_id: &'a str,
		_decision: ReviewDecision,
	) -> RunFuture<'a, Result<(), CoreError>> {
		Box::pin(async { panic!("no approval requested") })
	}
	fn acknowledge(
		&self,
		_offset: u64,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async { Ok(()) })
	}
	fn finish(&self) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async { Ok(()) })
	}
}
