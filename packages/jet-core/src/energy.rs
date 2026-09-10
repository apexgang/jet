//! Admission budgets never revoke an existing execution or remove pending input.
use crate::{
	CoreError, RunId, SettingKey, SettingScope, SettingValue, TurnSource,
};
use jet_store::ReadTransaction;

pub(crate) struct Policy {
	pub(crate) limit: u32,
	pub(crate) foreground_override: bool,
	pub(crate) child_work: ChildWork,
}

/// Admission-only native child policy. Active children always keep running.
#[derive(
	Debug,
	Clone,
	Copy,
	Default,
	PartialEq,
	Eq,
	serde::Serialize,
	serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ChildWork {
	/// Preserve the Harness's normal child policy.
	#[default]
	Native,
	/// Reserve the constrained Plane budget for Runs; admit no new children.
	Paused,
}

pub(crate) async fn policy(
	core: &crate::Core,
	tx: &mut ReadTransaction,
) -> Result<Policy, CoreError> {
	let stored = tx.settings_for_scope(SettingScope::Plane.record()).await?;
	let values = crate::setting::resolve(
		&[
			SettingKey::EnergyConcurrency,
			SettingKey::EnergyLowPowerConcurrency,
			SettingKey::EnergyConstrained,
			SettingKey::EnergyForegroundOverride,
		],
		&stored,
	);
	let [normal, reduced, constrained, foreground] = values.as_slice() else {
		unreachable!("four requested Settings")
	};
	let (
		SettingValue::Count(normal),
		SettingValue::Count(reduced),
		SettingValue::Flag(constrained),
		SettingValue::Flag(foreground_override),
	) = (
		&normal.value,
		&reduced.value,
		&constrained.value,
		&foreground.value,
	)
	else {
		return Err(CoreError::conflict(
			"energy.policy_unreadable",
			"the Plane Energy policy cannot be read",
		));
	};
	let constrained = *constrained
		|| core.probe.power().await != jet_runtime::PowerState::Normal;
	Ok(Policy {
		limit: if constrained {
			(*normal).min(*reduced)
		} else {
			*normal
		},
		foreground_override: *foreground_override,
		child_work: if constrained {
			ChildWork::Paused
		} else {
			ChildWork::Native
		},
	})
}

pub(crate) enum Admission {
	NewRun,
	ExistingRun(RunId),
}

pub(crate) async fn admit(
	core: &crate::Core,
	tx: &mut ReadTransaction,
	source: TurnSource,
	admission: Admission,
) -> Result<ChildWork, CoreError> {
	if matches!(admission, Admission::NewRun) {
		core.check_disk(0).await?;
	}
	let policy = policy(core, tx).await?;
	if source == TurnSource::User && policy.foreground_override {
		return Ok(policy.child_work);
	}
	// ASVS 2.3.4, 15.4.2: count reservations and claim new work in the same
	// store transaction. Starting and stopping executions still consume capacity.
	let mut count = 0;
	let mut after = String::new();
	while count < policy.limit {
		let ids = tx.active_execution_ids(&after).await?;
		let Some(last) = ids.last() else {
			return Ok(policy.child_work);
		};
		after = last.to_string();
		count += ids
			.into_iter()
			.filter(
				|id| !matches!(admission, Admission::ExistingRun(run) if run.0 == *id),
			)
			.count() as u32;
	}
	Err(CoreError::conflict(
		"energy.budget_exhausted",
		"the Plane Energy budget is full; queued input is retained. Foreground work requires an explicit energy.foreground_override Setting",
	))
}

impl crate::Core {
	/// Apply the current energy constraint to capable active Crafts. Unsupported
	/// Crafts remain monitor-only; no control terminates an existing child.
	/// # Errors
	/// Returns a policy or transport error without altering Runs or their queues.
	pub async fn constrain_child_work(&self) -> Result<(), CoreError> {
		let connections: Vec<_> = self
			.run_recovery
			.connections
			.lock()
			.expect("connection lock")
			.values()
			.cloned()
			.collect();
		if connections.is_empty() {
			return Ok(());
		}
		let (policy, pending) = self
			.store
			.read(async |tx| {
				Ok::<_, CoreError>((
					policy(self, tx).await?,
					!tx.pending_turn_queues("").await?.is_empty(),
				))
			})
			.await?;
		if pending {
			// Admission may have seen a transient power constraint between two
			// normal maintenance samples. Retry retained input on every bounded
			// active-work deadline, even when the policy appears unchanged.
			self.turn_wake.send_replace(());
			self.run_work.notify_one();
		}
		for connection in connections {
			connection.constrain_children(policy.child_work).await?;
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	mod energy_tests {
		use crate::run::tests::{install_craft, start_core};
		use crate::test_support::{
			actor, conversation_snapshot, register_repository, request,
		};
		use crate::{
			Command, CommandOutcome, Core, RetentionPolicy, SettingKey,
			SettingScope, SettingValue, WorkingTreeRequest,
		};
		use pretty_assertions::assert_eq;

		async fn set(core: &Core, key: SettingKey, value: SettingValue) {
			core.execute(
				&actor(),
				request(Command::SetSetting {
					key,
					scope: SettingScope::Plane,
					value,
				}),
			)
			.await
			.unwrap();
		}

		async fn conversation(
			core: &Core,
			root: &std::path::Path,
		) -> crate::ConversationId {
			let project_id = register_repository(core, root).await;
			let CommandOutcome::ConversationCreated(conversation) = core
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
				panic!("Conversation expected")
			};
			conversation.conversation_id
		}

		fn start(id: crate::ConversationId) -> Command {
			Command::StartRun {
				conversation_id: id,
				craft: "fake".into(),
				prompt: "Continue".into(),
			}
		}

		#[tokio::test]
		async fn energy_budget_changes_preserve_admitted_runs_and_require_explicit_foreground_override()
		 {
			let dir = tempfile::tempdir().unwrap();
			let core = start_core(&dir.path().join("plane.sqlite3")).await;
			install_craft(dir.path());
			let first = conversation(&core, &dir.path().join("first")).await;
			let second = conversation(&core, &dir.path().join("second")).await;
			set(
				&core,
				SettingKey::EnergyConstrained,
				SettingValue::Flag(true),
			)
			.await;
			set(
				&core,
				SettingKey::EnergyLowPowerConcurrency,
				SettingValue::Count(1),
			)
			.await;
			core.execute(&actor(), request(start(first))).await.unwrap();
			let admitted = conversation_snapshot(&core, first).await;
			let refused = core
				.execute(&actor(), request(start(second)))
				.await
				.unwrap_err();
			assert_eq!(refused.code, "energy.budget_exhausted");
			set(
				&core,
				SettingKey::EnergyLowPowerConcurrency,
				SettingValue::Count(0),
			)
			.await;
			assert_eq!(
				conversation_snapshot(&core, first).await.runs,
				admitted.runs
			);
			assert_eq!(conversation_snapshot(&core, second).await.runs, vec![]);
			set(
				&core,
				SettingKey::EnergyForegroundOverride,
				SettingValue::Flag(true),
			)
			.await;
			core.execute(&actor(), request(start(second)))
				.await
				.unwrap();
		}

		#[tokio::test]
		async fn concurrent_commands_cannot_double_book_the_last_energy_slot() {
			let dir = tempfile::tempdir().unwrap();
			let core = start_core(&dir.path().join("plane.sqlite3")).await;
			install_craft(dir.path());
			let first = conversation(&core, &dir.path().join("first")).await;
			let second = conversation(&core, &dir.path().join("second")).await;
			set(
				&core,
				SettingKey::EnergyConstrained,
				SettingValue::Flag(true),
			)
			.await;
			let actor = actor();
			let (first, second) = tokio::join!(
				core.execute(&actor, request(start(first))),
				core.execute(&actor, request(start(second))),
			);
			let results = [first, second];
			assert_eq!(
				results.iter().filter(|result| result.is_ok()).count(),
				1
			);
			assert_eq!(
				results
					.into_iter()
					.filter_map(Result::err)
					.map(|error| error.code)
					.collect::<Vec<_>>(),
				vec!["energy.budget_exhausted"]
			);
		}
	}

	mod energy_transition_tests {
		//! Power changes arrive at the machine probe, without a Command waking the Run.
		use crate::clock::SystemClock;
		use crate::run::tests::{AdmissionHost, install_craft};
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
		async fn a_transient_failed_power_probe_cannot_strand_a_quiet_runs_turn()
		 {
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
					if deliveries.recv().await == Some(Delivery::Turn(pending))
					{
						break;
					}
				}
			})
			.await
			.expect(
				"power recovery must wake the silent Run without another Command",
			);
		}

		#[tokio::test]
		async fn a_turn_receives_the_new_child_constraint_before_native_delivery()
		 {
			let dir = tempfile::tempdir().unwrap();
			let (core, probe, observations, mut deliveries, id) =
				start(dir.path(), 1).await;
			let pending = queue(&core, id).await;
			let mut power = equipped();
			power.power = jet_runtime::PowerState::Constrained;
			probe.answer_with(power);
			complete(&observations).await;
			let first =
				tokio::time::timeout(Duration::from_secs(5), deliveries.recv())
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
				.with_run_host(Arc::new(Host(
					Mutex::new(Some(receiver)),
					deliveries,
				))),
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
			let project_id = crate::test_support::register_repository(
				&core,
				&home.join("repo"),
			)
			.await;
			let CommandOutcome::ConversationCreated(conversation) = core
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
			) -> RunFuture<'_, Result<Box<dyn RunConnection>, RunStartError>>
			{
				Box::pin(async {
					Ok(Box::new(Connection {
						observations: Mutex::new(
							self.0.lock().await.take().unwrap(),
						),
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
			fn receive(
				&self,
			) -> RunFuture<'_, Result<RunObservation, CoreError>> {
				Box::pin(async {
					if !self
						.started
						.swap(true, std::sync::atomic::Ordering::SeqCst)
					{
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
	}
}
