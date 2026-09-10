//! Bounded Auto-continue after structured quota exhaustion (ADR-0033).
pub(crate) mod work;

use crate::{
	AccountBindingId, Actor, CommandOutcome, ConversationId, CoreError,
	EventKind, EventSequence, RunId,
};
use jet_store::{ReadTransaction, WriteTransaction};
use serde::{Deserialize, Serialize};

/// Where the user configures Auto-continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoContinueTarget {
	/// Default for this Home Plane's Account binding.
	AccountBinding(AccountBindingId),
	/// One quota-exhaustion episode, consumed when selected.
	Conversation(ConversationId),
}
/// Explicit retry limits; absence of a configured policy resolves to off.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AutoContinuePolicy {
	/// Do not admit automatic input.
	#[default]
	Off,
	/// Retry the same native Conversation and execution selection.
	Retry {
		/// Initial fallback delay, in milliseconds.
		delay_ms: u32,
		/// Maximum exponential fallback delay, in milliseconds.
		max_delay_ms: u32,
		/// Maximum admitted retries in one exhaustion episode.
		max_retries: u32,
		/// Exact continuation input, 1 to 8192 bytes.
		message: String,
	},
}
/// Durable outcome of the most recent retry decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoContinueStatus {
	/// The selected policy disabled this exhaustion episode.
	Disabled,
	/// Queue capacity prevented admission; the retry count has not advanced.
	Deferred,
	/// The Turn queue holds the retry until its due time.
	Pending,
	/// The retry was claimed for native delivery.
	Dispatched,
	/// New user input canceled the retry.
	Canceled,
	/// The configured retry count was reached.
	Exhausted,
}
/// The evidence and policy retained for the latest retry decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoContinueRetry {
	/// Run whose structured quota condition triggered the decision.
	pub run_id: RunId,
	/// Input that encountered this quota condition.
	pub triggering_turn: uuid::Uuid,
	/// Turn admitted by this decision, absent until queue admission succeeds.
	pub retry_turn: Option<uuid::Uuid>,

	/// Provider-reported Usage condition, retained without guessing from text.
	pub usage: crate::QuotaReport,
	/// Plane clock at the observation.
	pub observed_at_unix_ms: i64,
	/// Earliest permitted delivery, respecting the Provider reset time.
	pub due_at_unix_ms: i64,
	/// Number of automatic retries admitted in this episode.
	pub retry_count: u32,
	/// Exact policy selected for this episode, including its message.
	pub policy: AutoContinuePolicy,
	/// Where that policy was selected.
	pub selected_from: AutoContinueTarget,
	/// Latest decision outcome.
	pub status: AutoContinueStatus,
}
/// A fenced policy and retry snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoContinueSnapshot {
	/// Plane Event cursor in the same read transaction.
	pub cursor: EventSequence,
	/// Configured policy, or off when unset.
	pub policy: AutoContinuePolicy,
	/// Latest retry decision for a Conversation.
	pub retry: Option<AutoContinueRetry>,
}

impl AutoContinuePolicy {
	fn validate(&self) -> Result<(), CoreError> {
		// ASVS 2.2.1, 2.3.2: user input cannot authorize unbounded retries.
		if let Self::Retry {
			delay_ms,
			max_delay_ms,
			max_retries,
			message,
		} = self && (*delay_ms == 0
			|| *max_delay_ms < *delay_ms
			|| *max_delay_ms > 86_400_000
			|| !(1..=100).contains(max_retries)
			|| message.trim().is_empty()
			|| message.len() > 8192)
		{
			return Err(CoreError::invalid_input(
				"auto_continue.invalid_policy",
				"retry delays must be 1 to 86400000 milliseconds, the count 1 to 100, and the message 1 to 8192 bytes",
			));
		}
		Ok(())
	}
}

pub(crate) async fn snapshot(
	tx: &mut ReadTransaction,
	target: AutoContinueTarget,
) -> Result<AutoContinueSnapshot, CoreError> {
	let (policy, retry) = match target {
		AutoContinueTarget::AccountBinding(id) => {
			require_binding(tx, id).await?;
			(binding_policy(tx, id).await?, None)
		}
		AutoContinueTarget::Conversation(id) => {
			let queue = crate::turn::queue::load(tx, id).await?;
			(
				queue.auto_continue_override.unwrap_or_default(),
				queue.auto_continue,
			)
		}
	};
	Ok(AutoContinueSnapshot {
		cursor: EventSequence(tx.event_cursor().await?),
		policy,
		retry,
	})
}
pub(crate) async fn binding_policy(
	tx: &mut ReadTransaction,
	id: AccountBindingId,
) -> Result<AutoContinuePolicy, CoreError> {
	tx.auto_continue_policy(id.0)
		.await?
		.map(|json| crate::run::state::decode(&json))
		.transpose()
		.map(Option::unwrap_or_default)
}
async fn require_binding(
	tx: &mut ReadTransaction,
	id: AccountBindingId,
) -> Result<(), CoreError> {
	if tx.account_binding(id.0).await?.is_none() {
		return Err(CoreError::not_found(
			"account.not_found",
			"the Account binding does not exist",
		));
	}
	Ok(())
}
pub(crate) async fn configure(
	tx: &mut WriteTransaction,
	actor: &Actor,
	target: AutoContinueTarget,
	policy: AutoContinuePolicy,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	policy.validate()?;
	let subject = match target {
		AutoContinueTarget::AccountBinding(id) => {
			require_binding(tx, id).await?;
			let json = serde_json::to_string(&policy).map_err(|e| {
				CoreError::internal("auto_continue.encode", e.to_string())
			})?;
			tx.save_auto_continue_policy(id.0, &json).await?;
			crate::event::EventSubject::Plane
		}
		AutoContinueTarget::Conversation(id) => {
			let mut queue = crate::turn::queue::load(tx, id).await?;
			queue.auto_continue_override = Some(policy.clone());
			if policy == AutoContinuePolicy::Off
				&& let Some(mut retry) = queue.auto_continue.clone()
				&& matches!(
					retry.status,
					AutoContinueStatus::Pending | AutoContinueStatus::Deferred
				) {
				let mut kept = Vec::new();
				for mut entry in queue.entries {
					if entry.turn.source == crate::TurnSource::AutoContinue
						&& entry.turn.state == crate::TurnState::Queued
					{
						entry.turn.state = crate::TurnState::Canceled;
						crate::turn::queue::changed(tx, actor, id, &entry, now)
							.await?;
					} else {
						kept.push(entry);
					}
				}
				queue.entries = kept;
				retry.status = AutoContinueStatus::Disabled;
				retry.policy = AutoContinuePolicy::Off;
				retry.selected_from = target;
				queue.auto_continue = Some(retry.clone());
				queue.auto_continue_override = None;
				crate::auto_continue::work::changed(tx, actor, id, retry, now)
					.await?;
			}
			crate::turn::queue::save(tx, id, &queue).await?;
			crate::event::EventSubject::Conversation(id)
		}
	};
	tx.append_event(
		EventKind::AutoContinueConfigured { target, policy }
			.to_record(actor, subject, now)?,
	)
	.await?;
	if let AutoContinueTarget::Conversation(id) = target {
		crate::auto_continue::work::reconsider(tx, id, now).await?;
	}
	crate::audit::record(
		tx,
		actor,
		crate::audit::Decision::succeeded(
			crate::AuditDecision::AutoContinuePolicyChanged,
			crate::audit::auto_continue_subject(target),
		),
		now,
	)
	.await?;
	Ok(CommandOutcome::AutoContinueConfigured)
}

#[cfg(test)]
pub(crate) mod tests {
	use crate::test_support::{
		actor, bind_native_account, request, start_core,
	};
	use crate::{
		AutoContinuePolicy, AutoContinueTarget, Command, CommandOutcome,
		ProviderId, Query, QueryResult,
	};
	use pretty_assertions::assert_eq;

	#[tokio::test]
	async fn binding_policy_is_off_by_default_and_survives_restart() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let core = start_core(&path).await;
		let binding =
			bind_native_account(&core, ProviderId("anthropic".into())).await;
		let target = AutoContinueTarget::AccountBinding(binding);
		let query = Query::AutoContinue { target };
		let QueryResult::AutoContinue(before) =
			core.query(&actor(), query.clone()).await.unwrap()
		else {
			panic!("Auto-continue")
		};
		assert_eq!(before.policy, AutoContinuePolicy::Off);
		let policy = AutoContinuePolicy::Retry {
			delay_ms: 60_000,
			max_delay_ms: 600_000,
			max_retries: 3,
			message: "Continue".into(),
		};
		let command = request(Command::SetAutoContinue {
			target,
			policy: policy.clone(),
		});
		let outcome = core.execute(&actor(), command.clone()).await.unwrap();
		core.close().await;
		let core = start_core(&path).await;
		assert_eq!(core.execute(&actor(), command).await.unwrap(), outcome);
		let QueryResult::AutoContinue(after) =
			core.query(&actor(), query).await.unwrap()
		else {
			panic!("Auto-continue")
		};
		assert_eq!(after.policy, policy);
		assert_eq!(after.retry, None);
	}

	use crate::checkpoint::tests::start_answering;
	use crate::test_support::register_repository;
	use crate::{
		AutoContinueStatus, ConversationId, Core, QuotaMeasure, QuotaReport,
		QuotaScope, QuotaUnit, RetentionPolicy, RunActivity, RunObservation,
		TurnSource, UsageEstimation, UsageFinality, UsageReport,
		VisaRunRequest, WorkingTreeRequest,
	};

	fn policy() -> AutoContinuePolicy {
		AutoContinuePolicy::Retry {
			delay_ms: 60_000,
			max_delay_ms: 600_000,
			max_retries: 2,
			message: "Continue the existing work".into(),
		}
	}
	fn exhausted() -> QuotaReport {
		QuotaReport {
			window: "five_hour".into(),
			scope: QuotaScope::ProviderAccount,
			measure: QuotaMeasure {
				unit: QuotaUnit::Share,
				used: 10_000,
				limit: Some(10_000),
			},
			window_seconds: Some(18_000),
			resets_in_seconds: Some(3600),
			estimation: UsageEstimation::Measured,
			finality: UsageFinality::Interim,
		}
	}
	async fn retry(
		core: &Core,
		id: ConversationId,
	) -> crate::AutoContinueSnapshot {
		let QueryResult::AutoContinue(value) = core
			.query(
				&actor(),
				Query::AutoContinue {
					target: AutoContinueTarget::Conversation(id),
				},
			)
			.await
			.unwrap()
		else {
			panic!("Auto-continue")
		};
		*value
	}

	#[tokio::test]
	async fn structured_exhaustion_admits_one_retry_and_user_input_cancels_it()
	{
		let dir = tempfile::tempdir().unwrap();
		let (core, sender, _) = start_answering(dir.path()).await;
		let binding =
			bind_native_account(&core, ProviderId("anthropic".into())).await;
		core.execute(
			&actor(),
			request(Command::SetAutoContinue {
				target: AutoContinueTarget::AccountBinding(binding),
				policy: policy(),
			}),
		)
		.await
		.unwrap();
		let project_id =
			register_repository(&core, &dir.path().join("repo")).await;
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
		let QueryResult::Status(status) =
			core.query(&actor(), Query::Status).await.unwrap()
		else {
			panic!("Status")
		};
		let CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::StartVisaRun(VisaRunRequest {
					conversation_id: id,
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
		for observation in [
			RunObservation::Usage(UsageReport::ProviderQuota(exhausted())),
			RunObservation::Activity(RunActivity::WaitingForQuota),
			RunObservation::Completed("native-1".into()),
			RunObservation::Progress {
				offset: 2,
				checkpoint: String::new(),
			},
		] {
			sender.send(observation).await.unwrap();
		}
		let decision =
			tokio::time::timeout(std::time::Duration::from_secs(5), async {
				loop {
					if let Some(value) = retry(&core, id).await.retry {
						break value;
					}
					tokio::time::sleep(std::time::Duration::from_millis(10))
						.await;
				}
			})
			.await
			.expect("structured exhaustion admits a retry");
		assert_eq!(
			(
				&decision.policy,
				decision.selected_from,
				decision.retry_count,
				decision.status,
				&decision.usage
			),
			(
				&policy(),
				AutoContinueTarget::AccountBinding(binding),
				1,
				AutoContinueStatus::Pending,
				&exhausted()
			)
		);
		assert_eq!(
			decision.due_at_unix_ms - decision.observed_at_unix_ms,
			3_600_000
		);
		assert_eq!(decision.run_id, run.run_id);
		core.execute(
			&actor(),
			request(Command::SubmitTurn {
				conversation_id: id,
				source: TurnSource::User,
				prompt: "Do this instead".into(),
			}),
		)
		.await
		.unwrap();
		assert_eq!(
			retry(&core, id).await.retry.unwrap().status,
			AutoContinueStatus::Canceled
		);
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
			panic!("Queue")
		};
		assert_eq!(
			queue
				.turns
				.iter()
				.map(|turn| turn.source)
				.collect::<Vec<_>>(),
			vec![TurnSource::User]
		);
		let QueryResult::RunExecution(execution) = core
			.query(&actor(), Query::RunExecution { run_id: run.run_id })
			.await
			.unwrap()
		else {
			panic!("Run")
		};
		assert_eq!(execution.activity, Some(RunActivity::WaitingForQuota));
	}

	mod driver {
		//! Driven Harness and clock for Auto-continue tests at Commands/Queries/Events.
		use super::*;
		use crate::test_support::{
			FixedProbe, ManualClock, equipped, start_core_with,
		};
		use crate::{
			CoreError, LaunchPlan, PinnedCraft, RunConnection, RunFuture,
			RunHost, RunId, RunStartError,
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
			pub async fn start(
				dir: &Path,
				selected: AutoContinuePolicy,
			) -> Self {
				let clock = ManualClock::at(
					UNIX_EPOCH + Duration::from_secs(1_700_000_000),
				);
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
					bind_native_account(&core, ProviderId("anthropic".into()))
						.await;
				core.execute(
					&actor(),
					request(Command::SetAutoContinue {
						target: AutoContinueTarget::AccountBinding(binding),
						policy: selected,
					}),
				)
				.await
				.unwrap();
				let project_id =
					register_repository(&core, &dir.join("repo")).await;
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
					let executable =
						Path::new("/bin/cat").canonicalize().unwrap();
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
			) -> RunFuture<'_, Result<Box<dyn RunConnection>, RunStartError>>
			{
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
			fn interrupt(
				&self,
				_: uuid::Uuid,
			) -> RunFuture<'_, Result<(), CoreError>> {
				Box::pin(async { panic!("no interruption") })
			}
			fn decide_approval<'a>(
				&'a self,
				_: &'a str,
				_: crate::ReviewDecision,
			) -> RunFuture<'a, Result<(), CoreError>> {
				Box::pin(async { panic!("no approval") })
			}
			fn acknowledge(
				&self,
				_: u64,
			) -> RunFuture<'_, Result<(), CoreError>> {
				Box::pin(async { Ok(()) })
			}
			fn finish(&self) -> RunFuture<'_, Result<(), CoreError>> {
				Box::pin(async { Ok(()) })
			}

			#[expect(
				clippy::await_holding_invalid_type,
				reason = "the test driver serializes reads from its shared observation stream"
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
					self.receiver.lock().await.recv().await.ok_or_else(|| {
						CoreError::internal("fixture.closed", "fixture closed")
					})
				})
			}
		}
	}

	#[tokio::test]
	async fn retry_backoff_is_capped_and_stops_at_the_configured_bound() {
		let dir = tempfile::tempdir().unwrap();
		let selected = AutoContinuePolicy::Retry {
			delay_ms: 2000,
			max_delay_ms: 3000,
			max_retries: 2,
			message: "Continue".into(),
		};
		let mut h = driver::Harness::start(dir.path(), selected).await;
		h.limited(None).await;
		assert_eq!(retry(&h.core, h.id).await.retry.unwrap().retry_count, 1);
		h.tick(1).await;
		assert_eq!(h.host.inputs.lock().unwrap().len(), 0);
		h.tick(1).await;
		assert_eq!(h.host.inputs.lock().unwrap().len(), 1);
		h.limited(None).await;
		let second = retry(&h.core, h.id).await.retry.unwrap();
		assert_eq!(
			(
				second.retry_count,
				second.due_at_unix_ms - second.observed_at_unix_ms
			),
			(2, 3000)
		);
		h.tick(3).await;
		assert_eq!(h.host.inputs.lock().unwrap().len(), 2);
		h.limited(None).await;
		h.tick(100).await;
		assert_eq!(h.host.inputs.lock().unwrap().len(), 2);
		let stopped = retry(&h.core, h.id).await.retry.unwrap();
		assert_eq!(
			(stopped.retry_count, stopped.status),
			(2, AutoContinueStatus::Exhausted)
		);
	}

	#[tokio::test]
	async fn one_shot_override_can_enable_an_existing_quota_wait() {
		let dir = tempfile::tempdir().unwrap();
		let mut h =
			driver::Harness::start(dir.path(), AutoContinuePolicy::Off).await;
		h.limited(None).await;
		assert!(h.host.inputs.lock().unwrap().is_empty());
		h.core
			.execute(
				&actor(),
				request(Command::SetAutoContinue {
					target: AutoContinueTarget::Conversation(h.id),
					policy: policy(),
				}),
			)
			.await
			.unwrap();
		let selected = retry(&h.core, h.id).await;
		assert_eq!(selected.policy, AutoContinuePolicy::Off);
		let selected = selected
			.retry
			.expect("the one-shot override admits the existing wait");
		assert_eq!(
			(selected.selected_from, selected.retry_count),
			(AutoContinueTarget::Conversation(h.id), 1)
		);
		let QueryResult::AutoContinue(binding) = h
			.core
			.query(
				&actor(),
				Query::AutoContinue {
					target: AutoContinueTarget::AccountBinding(h.binding),
				},
			)
			.await
			.unwrap()
		else {
			panic!("Policy")
		};
		assert_eq!(binding.policy, AutoContinuePolicy::Off);
	}

	#[tokio::test]
	async fn one_shot_off_survives_repeated_reports_for_the_same_quota_wait() {
		let dir = tempfile::tempdir().unwrap();
		let mut h = driver::Harness::start(dir.path(), policy()).await;
		h.core
			.execute(
				&actor(),
				request(Command::SetAutoContinue {
					target: AutoContinueTarget::Conversation(h.id),
					policy: AutoContinuePolicy::Off,
				}),
			)
			.await
			.unwrap();
		h.limited(None).await;
		h.send(vec![
			RunObservation::Usage(UsageReport::ProviderQuota(exhausted())),
			RunObservation::Activity(RunActivity::WaitingForQuota),
		])
		.await;
		let QueryResult::TurnQueue(queue) = h
			.core
			.query(
				&actor(),
				Query::TurnQueue {
					conversation_id: h.id,
				},
			)
			.await
			.unwrap()
		else {
			panic!("Queue")
		};
		assert_eq!(queue.turns, vec![]);
	}

	#[tokio::test]
	async fn restart_preserves_due_time_retry_count_and_native_execution_selection()
	 {
		let dir = tempfile::tempdir().unwrap();
		let mut h = driver::Harness::start(dir.path(), policy()).await;
		h.limited(Some(20)).await;
		let before = retry(&h.core, h.id).await.retry;
		h.finish().await;
		h.restart(dir.path()).await;
		assert_eq!(retry(&h.core, h.id).await.retry, before);
		h.tick(19).await;
		assert_eq!(h.host.launches.lock().unwrap().len(), 1);
		h.tick(1).await;
		let launches = h.host.launches.lock().unwrap().clone();
		assert_eq!(launches.len(), 2);
		assert_eq!(
			(
				&launches[1].craft,
				launches[1].visa,
				&launches[1].no_visa,
				&launches[1].native_conversation,
				&launches[1].model,
				launches[1].prompt.as_str()
			),
			(
				&launches[0].craft,
				launches[0].visa,
				&launches[0].no_visa,
				&Some("native-1".into()),
				&Some(crate::ModelId("original-model".into())),
				"Continue the existing work"
			)
		);
		let decision = retry(&h.core, h.id).await.retry.unwrap();
		assert_eq!(
			(decision.retry_count, decision.status),
			(1, AutoContinueStatus::Dispatched)
		);
	}

	mod queue_tests {
		use super::*;
		use pretty_assertions::assert_eq;

		async fn sources(h: &driver::Harness) -> Vec<TurnSource> {
			let QueryResult::TurnQueue(queue) = h
				.core
				.query(
					&actor(),
					Query::TurnQueue {
						conversation_id: h.id,
					},
				)
				.await
				.unwrap()
			else {
				panic!("queue")
			};
			queue.turns.into_iter().map(|t| t.source).collect()
		}
		async fn submit(h: &driver::Harness, source: TurnSource) {
			h.core
				.execute(
					&actor(),
					request(Command::SubmitTurn {
						conversation_id: h.id,
						source,
						prompt: "queued input".into(),
					}),
				)
				.await
				.unwrap();
		}
		#[tokio::test]
		async fn a_nonexhausted_window_does_not_hide_an_exhausted_window() {
			let dir = tempfile::tempdir().unwrap();
			let mut h = driver::Harness::start(dir.path(), policy()).await;
			let mut secondary = exhausted();
			secondary.window = "weekly".into();
			secondary.measure.used = 1;
			h.send(vec![
				RunObservation::Usage(UsageReport::ProviderQuota(exhausted())),
				RunObservation::Usage(UsageReport::ProviderQuota(secondary)),
				RunObservation::Activity(RunActivity::WaitingForQuota),
				RunObservation::Completed("native-1".into()),
			])
			.await;
			let selected = retry(&h.core, h.id).await.retry.unwrap();
			assert_eq!(selected.usage, exhausted());
			assert_eq!(sources(&h).await, vec![TurnSource::AutoContinue]);
		}
		#[tokio::test]
		async fn replacement_and_off_keep_user_and_scheduled_input() {
			let dir = tempfile::tempdir().unwrap();
			let mut h = driver::Harness::start(dir.path(), policy()).await;
			submit(&h, TurnSource::User).await;
			submit(&h, TurnSource::Schedule).await;
			h.limited(Some(20)).await;
			let mut replacement = policy();
			if let AutoContinuePolicy::Retry { message, .. } = &mut replacement
			{
				*message = "Updated continuation".into();
			}
			h.core
				.execute(
					&actor(),
					request(Command::SetAutoContinue {
						target: AutoContinueTarget::Conversation(h.id),
						policy: replacement.clone(),
					}),
				)
				.await
				.unwrap();
			assert_eq!(
				sources(&h).await,
				vec![
					TurnSource::User,
					TurnSource::Schedule,
					TurnSource::AutoContinue
				]
			);
			assert_eq!(
				retry(&h.core, h.id).await.retry.unwrap().policy,
				replacement
			);
			h.core
				.execute(
					&actor(),
					request(Command::SetAutoContinue {
						target: AutoContinueTarget::Conversation(h.id),
						policy: AutoContinuePolicy::Off,
					}),
				)
				.await
				.unwrap();
			assert_eq!(
				sources(&h).await,
				vec![TurnSource::User, TurnSource::Schedule]
			);
			assert_eq!(
				retry(&h.core, h.id).await.retry.unwrap().status,
				AutoContinueStatus::Disabled
			);
			h.tick(19).await;
			assert!(h.host.inputs.lock().unwrap().is_empty());
		}
		#[tokio::test]
		async fn quota_evidence_without_structured_wait_cannot_authorize_retry()
		{
			let dir = tempfile::tempdir().unwrap();
			let mut h = driver::Harness::start(dir.path(), policy()).await;
			h.send(vec![
				RunObservation::Usage(UsageReport::ProviderQuota(exhausted())),
				RunObservation::Completed("native-1".into()),
			])
			.await;
			assert_eq!(retry(&h.core, h.id).await.retry, None);
			assert_eq!(sources(&h).await, vec![]);
		}

		#[tokio::test]
		async fn later_reset_evidence_postpones_the_same_pending_turn() {
			let dir = tempfile::tempdir().unwrap();
			let mut h = driver::Harness::start(dir.path(), policy()).await;
			h.limited(Some(60)).await;
			let before = retry(&h.core, h.id).await.retry.unwrap();
			h.tick(10).await;
			let mut second = exhausted();
			second.window = "weekly".into();
			h.send(vec![RunObservation::Usage(UsageReport::ProviderQuota(
				second,
			))])
			.await;
			let after = retry(&h.core, h.id).await.retry.unwrap();
			assert_eq!(after.retry_count, before.retry_count);
			assert_eq!(
				after.due_at_unix_ms,
				before.observed_at_unix_ms + 3_610_000
			);
			assert_eq!(sources(&h).await, vec![TurnSource::AutoContinue]);
			h.tick(50).await;
			assert!(h.host.inputs.lock().unwrap().is_empty());
		}
		#[tokio::test]
		async fn scheduled_work_does_not_reuse_a_consumed_one_shot_policy() {
			let dir = tempfile::tempdir().unwrap();
			let mut h =
				driver::Harness::start(dir.path(), AutoContinuePolicy::Off)
					.await;
			h.core
				.execute(
					&actor(),
					request(Command::SetAutoContinue {
						target: AutoContinueTarget::Conversation(h.id),
						policy: policy(),
					}),
				)
				.await
				.unwrap();
			h.limited(Some(1)).await;
			h.tick(1).await;
			let turn_id = h.host.inputs.lock().unwrap().last().unwrap().0;
			h.send(vec![
				RunObservation::Activity(RunActivity::Working),
				RunObservation::TurnCompleted {
					turn_id,
					native_conversation: "native-1".into(),
				},
			])
			.await;
			submit(&h, TurnSource::Schedule).await;
			h.tick(1).await;
			h.limited(Some(1)).await;
			assert_eq!(
				retry(&h.core, h.id).await.retry.unwrap().status,
				AutoContinueStatus::Disabled
			);
		}
		#[tokio::test]
		async fn consumption_breakdown_does_not_change_the_native_model_selection()
		 {
			let dir = tempfile::tempdir().unwrap();
			let mut h = driver::Harness::start(dir.path(), policy()).await;
			h.send(vec![RunObservation::Usage(UsageReport::Observed(
				crate::ObservedUsage {
					model: Some(crate::ModelId("child-model".into())),
					measurement: crate::UsageMeasurement::Run {
						native_usage_id: None,
					},
					estimation: UsageEstimation::Measured,
					finality: UsageFinality::Interim,
					tokens: crate::UsageTokens::default(),
				},
			))])
			.await;
			h.limited(Some(1)).await;
			h.finish().await;
			h.tick(1).await;
			assert_eq!(
				h.host.launches.lock().unwrap().last().unwrap().model,
				Some(crate::ModelId("original-model".into()))
			);
		}

		#[tokio::test]
		async fn live_model_changes_leave_admitted_retry_pending() {
			let dir = tempfile::tempdir().unwrap();
			let mut h = driver::Harness::start(dir.path(), policy()).await;
			h.limited(Some(1)).await;
			h.send(vec![RunObservation::Model(crate::ModelId(
				"different-model".into(),
			))])
			.await;
			h.tick(1).await;
			assert!(h.host.inputs.lock().unwrap().is_empty());
			assert_eq!(
				retry(&h.core, h.id).await.retry.unwrap().status,
				AutoContinueStatus::Pending
			);
		}
		#[tokio::test]
		async fn full_queue_defers_admission_without_consuming_retry_budget() {
			let dir = tempfile::tempdir().unwrap();
			let mut h =
				driver::Harness::start(dir.path(), AutoContinuePolicy::Off)
					.await;
			h.limited(Some(3600)).await;
			for _ in 0..128 {
				submit(&h, TurnSource::User).await;
			}
			h.core
				.execute(
					&actor(),
					request(Command::SetAutoContinue {
						target: AutoContinueTarget::Conversation(h.id),
						policy: policy(),
					}),
				)
				.await
				.unwrap();
			let deferred = retry(&h.core, h.id).await.retry.unwrap();
			assert_eq!(
				(deferred.status, deferred.retry_count),
				(AutoContinueStatus::Deferred, 0)
			);
			let QueryResult::TurnQueue(queue) = h
				.core
				.query(
					&actor(),
					Query::TurnQueue {
						conversation_id: h.id,
					},
				)
				.await
				.unwrap()
			else {
				panic!("queue")
			};
			h.core
				.execute(
					&actor(),
					request(Command::WithdrawTurn {
						conversation_id: h.id,
						turn_id: queue.turns[0].turn_id,
					}),
				)
				.await
				.unwrap();
			h.tick(0).await;
			let admitted = retry(&h.core, h.id).await.retry.unwrap();
			assert_eq!(
				(admitted.status, admitted.retry_count),
				(AutoContinueStatus::Pending, 1)
			);
			let sources = sources(&h).await;
			assert_eq!(
				sources.iter().filter(|s| **s == TurnSource::User).count(),
				127
			);
			assert_eq!(sources.last(), Some(&TurnSource::AutoContinue));
		}

		#[tokio::test]
		async fn repeated_reports_without_reset_keep_the_original_fallback_deadline()
		 {
			let dir = tempfile::tempdir().unwrap();
			let mut h = driver::Harness::start(dir.path(), policy()).await;
			h.limited(None).await;
			let before = retry(&h.core, h.id).await.retry;
			h.tick(10).await;
			let mut usage = exhausted();
			usage.resets_in_seconds = None;
			h.send(vec![RunObservation::Usage(UsageReport::ProviderQuota(
				usage,
			))])
			.await;
			assert_eq!(retry(&h.core, h.id).await.retry, before);
		}

		#[tokio::test]
		async fn cancellation_retains_later_provider_reset_extensions() {
			let dir = tempfile::tempdir().unwrap();
			let mut h = driver::Harness::start(dir.path(), policy()).await;
			h.limited(Some(60)).await;
			submit(&h, TurnSource::User).await;
			h.tick(10).await;
			h.send(vec![RunObservation::Usage(UsageReport::ProviderQuota(
				exhausted(),
			))])
			.await;
			let updated = retry(&h.core, h.id).await.retry.unwrap();
			assert_eq!(updated.status, AutoContinueStatus::Canceled);
			assert_eq!(updated.due_at_unix_ms, 1_700_003_610_000);
			h.tick(50).await;
			assert!(h.host.inputs.lock().unwrap().is_empty());
			assert_eq!(sources(&h).await, vec![TurnSource::User]);
		}
	}
}
