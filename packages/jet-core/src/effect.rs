//! Durable Effects and restart reconciliation (ADR-0064, ADR-0067).

use crate::{
	CommandId, Core, CoreError, PromotionId, RunId,
	promotion::effect as promotion_effect,
};
use jet_store::{
	EffectKindRecord, EffectRecord, EffectSafetyRecord, EffectStateRecord,
	WriteTransaction,
};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffectKind {
	/// Deferred native extension mutation.
	ChangeExtension,
	Utility,
	GitDelivery,
	StartTerminal {
		terminal_id: crate::TerminalId,
	},
	CloseTerminal {
		terminal_id: crate::TerminalId,
	},
	ResolveExecution,
	StartRun {
		run_id: RunId,
	},
	/// Carry out one admitted Interrupt turn or Stop Run request
	/// (ADR-0083).
	ControlRun {
		run_id: RunId,
	},
	/// Apply one recorded Workspace promotion to its destination
	/// (ADR-0025).
	PromoteWorkspace {
		promotion_id: PromotionId,
	},
	/// Publish a verified staged Craft Artifact and accepted manifest.
	InstallCraft,
}

pub(crate) type EffectSafety = EffectSafetyRecord;
pub(crate) type EffectState = EffectStateRecord;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Effect {
	pub(crate) effect_id: Uuid,
	pub(crate) command_id: CommandId,
	pub(crate) kind: EffectKind,
	pub(crate) safety: EffectSafety,
	pub(crate) state: EffectState,
	pub(crate) attempt_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectResult {
	Completed,
	Failed,
	Unknown,
}

/// Adapter at the seam between durable orchestration and external work.
///
/// Implementations must use the Effect's recorded stable external key whenever
/// the target supports idempotency. They must return
/// [`EffectResult::Unknown`] rather than guessing when an acknowledgement is
/// lost.
pub(crate) trait EffectAdapter {
	/// Performs a new or provably safe repeated attempt.
	fn execute(
		&mut self,
		effect: &Effect,
	) -> impl std::future::Future<Output = EffectResult> + Send;

	/// Observes an interrupted attempt without changing external state.
	fn reconcile(
		&mut self,
		effect: &Effect,
	) -> impl std::future::Future<Output = EffectResult> + Send;
}

impl Core {
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "the guard must span the Adapter call between the two store \
		          transactions; releasing it earlier would let a second \
		          worker claim the same in-flight Effect (ADR-0067)"
	)]
	/// Performs every pending Effect of one `kind` and reconciles every
	/// interrupted one through `adapter`, so a worker that performs one
	/// kind of work leaves the others to theirs.
	pub(crate) async fn reconcile_effects(
		&self,
		adapter: &mut impl EffectAdapter,
		kind: EffectKindRecord,
	) -> Result<Vec<Effect>, CoreError> {
		// ASVS 15.4.1: one core serializes all Effect decisions, so two
		// workers cannot execute the same durable request concurrently. The
		// guard spans store awaits, so it is the async mutex.
		let _guard = self.effect_reconciliation.lock().await;
		let records = self
			.store
			.read(async |tx| tx.unresolved_effects_of(kind).await)
			.await?;
		let mut effects = Vec::with_capacity(records.len());
		for record in records {
			effects.push(self.reconcile_effect(adapter, record).await?);
		}
		Ok(effects)
	}

	async fn reconcile_effect(
		&self,
		adapter: &mut impl EffectAdapter,
		record: EffectRecord,
	) -> Result<Effect, CoreError> {
		let effect = Effect::try_from(record)?;
		match effect.state {
			EffectState::Pending => self.execute_effect(adapter, effect).await,
			EffectState::InFlight => match adapter.reconcile(&effect).await {
				EffectResult::Completed => {
					self.finish_effect(effect, EffectStateRecord::Completed)
						.await
				}
				EffectResult::Failed => {
					self.finish_effect(effect, EffectStateRecord::Failed).await
				}
				EffectResult::Unknown
					if may_retry(effect.safety, effect.attempt_count) =>
				{
					self.execute_effect(adapter, effect).await
				}
				EffectResult::Unknown => {
					self.finish_effect(
						effect,
						EffectStateRecord::OutcomeUnknown,
					)
					.await
				}
			},
			EffectState::Completed
			| EffectState::Failed
			| EffectState::OutcomeUnknown => Ok(effect),
		}
	}

	async fn execute_effect(
		&self,
		adapter: &mut impl EffectAdapter,
		effect: Effect,
	) -> Result<Effect, CoreError> {
		if matches!(
			effect.kind,
			EffectKind::StartRun { .. }
				| EffectKind::StartTerminal { .. }
				| EffectKind::ChangeExtension
				| EffectKind::PromoteWorkspace { .. }
				| EffectKind::InstallCraft
				| EffectKind::GitDelivery
		) {
			match self.check_effect_disk(&effect.kind).await {
				Ok(()) => {}
				// Preserve the exact pending/in-flight decision; recovery and controls
				// still reconcile, without spending an attempt on a refused launch.
				Err(error) if error.code == "storage.disk_pressure" => {
					return Ok(effect);
				}
				Err(error) => return Err(error),
			}
		}
		let record = self
			.store
			.write(async |tx| tx.begin_effect_attempt(effect.effect_id).await)
			.await?;
		let in_flight = Effect::try_from(record)?;
		match adapter.execute(&in_flight).await {
			EffectResult::Completed => {
				self.finish_effect(in_flight, EffectStateRecord::Completed)
					.await
			}
			EffectResult::Failed => {
				self.finish_effect(in_flight, EffectStateRecord::Failed)
					.await
			}
			EffectResult::Unknown => Ok(in_flight),
		}
	}

	/// Records an Effect's terminal state together with what it settles:
	/// the outcome of the work is durable in the same transaction as the
	/// Effect that did it (ADR-0064).
	async fn finish_effect(
		&self,
		effect: Effect,
		state: EffectStateRecord,
	) -> Result<Effect, CoreError> {
		let now_unix_ms = self.now_unix_ms();
		let record = self
			.store
			.write(async |tx| {
				let record = tx.finish_effect(effect.effect_id, state).await?;
				settle(tx, &effect, state, now_unix_ms).await?;
				Ok::<_, CoreError>(record)
			})
			.await?;
		Effect::try_from(record)
	}
}

/// Settles what the Effect was for, in the transaction that finishes it.
async fn settle(
	tx: &mut WriteTransaction,
	effect: &Effect,
	state: EffectStateRecord,
	now_unix_ms: i64,
) -> Result<(), CoreError> {
	match effect.kind {
		EffectKind::Utility | EffectKind::GitDelivery => Ok(()),
		EffectKind::StartTerminal { terminal_id }
		| EffectKind::CloseTerminal { terminal_id } => {
			crate::terminal::effect::settle(
				tx,
				terminal_id,
				&effect.kind,
				state,
				now_unix_ms,
			)
			.await
		}
		EffectKind::ResolveExecution => {
			crate::run::orphan::settle(tx, effect, state, now_unix_ms).await
		}
		EffectKind::StartRun { run_id } => {
			crate::run::state::settle_start(tx, run_id, state, now_unix_ms)
				.await
		}
		EffectKind::ControlRun { .. } => Ok(()),
		EffectKind::PromoteWorkspace { promotion_id } => {
			promotion_effect::settle(tx, promotion_id, state, now_unix_ms).await
		}
		EffectKind::ChangeExtension | EffectKind::InstallCraft => Ok(()),
	}
}

impl TryFrom<EffectRecord> for Effect {
	type Error = CoreError;

	fn try_from(record: EffectRecord) -> Result<Self, CoreError> {
		let kind = match record.kind {
			EffectKindRecord::Utility => EffectKind::Utility,
			EffectKindRecord::GitDelivery => EffectKind::GitDelivery,
			EffectKindRecord::StartTerminal => EffectKind::StartTerminal {
				terminal_id: crate::TerminalId(
					record.terminal_id.ok_or_else(crate::terminal::missing)?,
				),
			},
			EffectKindRecord::CloseTerminal => EffectKind::CloseTerminal {
				terminal_id: crate::TerminalId(
					record.terminal_id.ok_or_else(crate::terminal::missing)?,
				),
			},
			EffectKindRecord::ResolveExecution => EffectKind::ResolveExecution,
			EffectKindRecord::StartRun => EffectKind::StartRun {
				run_id: RunId(record.run_id.ok_or_else(|| {
					CoreError::internal(
						"effect.invalid",
						"a run.start Effect has no Run identity",
					)
				})?),
			},
			EffectKindRecord::ControlRun => EffectKind::ControlRun {
				run_id: RunId(record.run_id.ok_or_else(|| {
					CoreError::internal(
						"effect.invalid",
						"a run.control Effect has no Run identity",
					)
				})?),
			},
			EffectKindRecord::PromoteWorkspace => {
				EffectKind::PromoteWorkspace {
					promotion_id: PromotionId(record.promotion_id.ok_or_else(
						|| {
							CoreError::internal(
								"effect.invalid",
								"a workspace.promote Effect has no promotion identity",
							)
						},
					)?),
				}
			}
			EffectKindRecord::ChangeExtension => EffectKind::ChangeExtension,
			EffectKindRecord::InstallCraft => EffectKind::InstallCraft,
		};
		Ok(Self {
			effect_id: record.effect_id,
			command_id: CommandId(record.command_id),
			kind,
			safety: record.safety,
			state: record.state,
			attempt_count: record.attempt_count,
		})
	}
}

fn may_retry(safety: EffectSafety, attempt_count: u32) -> bool {
	// ASVS 13.2.6: retry eligibility and its bound are explicit policy.
	match safety {
		EffectSafety::ReadOnly { max_attempts }
		| EffectSafety::Idempotent { max_attempts, .. } => {
			attempt_count < max_attempts
		}
		EffectSafety::Ambiguous => false,
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use jet_store::{EffectKindRecord, EffectSafetyRecord, NewEffect};

	use crate::effect::{
		Effect, EffectAdapter, EffectKind, EffectResult, EffectSafety,
		EffectState,
	};
	use crate::test_support::{actor, start_core};
	use crate::{
		Command, CommandEnvelope, CommandId, CommandOutcome, ConversationId,
		Core, Query, QueryResult, RetentionPolicy, Run, RunLifecycle,
		WorkingTreeRequest,
	};

	async fn execute(
		core: &Core,
		command_id: CommandId,
		command: Command,
	) -> CommandOutcome {
		let bytes = serde_json::to_vec(&command).unwrap();
		core.execute(
			&actor(),
			CommandEnvelope::new(command_id, command, &bytes).unwrap(),
		)
		.await
		.unwrap()
	}

	async fn create_run(core: &Core) -> (ConversationId, Run) {
		let CommandOutcome::ConversationCreated(conversation) = execute(
			core,
			CommandId(Uuid::now_v7()),
			Command::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRequest::NoProject,
			},
		)
		.await
		else {
			panic!("expected a Conversation");
		};
		let CommandOutcome::RunCreated(run) = execute(
			core,
			CommandId(Uuid::now_v7()),
			Command::CreateRun {
				conversation_id: conversation.conversation_id,
			},
		)
		.await
		else {
			panic!("expected a Run");
		};
		(conversation.conversation_id, run)
	}

	async fn queue_start(core: &Core, run: &Run) -> CommandId {
		let command_id = CommandId(Uuid::now_v7());
		execute(
			core,
			command_id,
			Command::TransitionRun {
				run_id: run.run_id,
				expected_revision: run.revision,
				lifecycle: RunLifecycle::Starting,
			},
		)
		.await;
		command_id
	}

	async fn lifecycle(
		core: &Core,
		conversation_id: ConversationId,
	) -> RunLifecycle {
		let QueryResult::Conversation(snapshot) = core
			.query(&actor(), Query::Conversation { conversation_id })
			.await
			.unwrap()
		else {
			panic!("expected a Conversation snapshot");
		};
		snapshot.runs[0].lifecycle
	}

	struct RecordingAdapter {
		execution: EffectResult,
		executed: Vec<Effect>,
		reconciled: Vec<Effect>,
	}

	impl RecordingAdapter {
		fn new(execution: EffectResult) -> Self {
			Self {
				execution,
				executed: vec![],
				reconciled: vec![],
			}
		}
	}

	impl EffectAdapter for RecordingAdapter {
		async fn execute(&mut self, effect: &Effect) -> EffectResult {
			self.executed.push(effect.clone());
			self.execution
		}

		async fn reconcile(&mut self, effect: &Effect) -> EffectResult {
			self.reconciled.push(effect.clone());
			EffectResult::Unknown
		}
	}

	async fn assert_no_work_remains(path: &std::path::Path) {
		let core = start_core(path).await;
		let mut adapter = RecordingAdapter::new(EffectResult::Unknown);
		assert_eq!(
			core.reconcile_effects(&mut adapter, EffectKindRecord::StartRun)
				.await
				.unwrap(),
			vec![]
		);
		assert_eq!((adapter.executed, adapter.reconciled), (vec![], vec![]));
	}

	#[tokio::test]
	async fn a_starting_run_and_its_effect_commit_before_external_work_begins()
	{
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let first = start_core(&path).await;
		let (conversation_id, run) = create_run(&first).await;
		let command_id = queue_start(&first, &run).await;
		assert_eq!(
			lifecycle(&first, conversation_id).await,
			RunLifecycle::Starting
		);
		drop(first);

		let restarted = start_core(&path).await;
		assert_eq!(
			lifecycle(&restarted, conversation_id).await,
			RunLifecycle::Starting
		);
		let mut adapter = RecordingAdapter::new(EffectResult::Completed);
		let reconciled = restarted
			.reconcile_effects(&mut adapter, EffectKindRecord::StartRun)
			.await
			.unwrap();
		let attempted = adapter.executed[0].clone();
		let expected_attempt = Effect {
			effect_id: attempted.effect_id,
			command_id,
			kind: EffectKind::StartRun { run_id: run.run_id },
			safety: EffectSafety::Idempotent {
				external_key: attempted.effect_id,
				max_attempts: 3,
			},
			state: EffectState::InFlight,
			attempt_count: 1,
		};

		assert_eq!(attempted, expected_attempt);
		assert_eq!(
			reconciled,
			vec![Effect {
				state: EffectState::Completed,
				..expected_attempt
			}]
		);
		drop(restarted);
		assert_no_work_remains(&path).await;
	}

	#[tokio::test]
	async fn an_idempotent_effect_resumes_under_the_same_identity_after_interruption()
	 {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let first = start_core(&path).await;
		let (_, run) = create_run(&first).await;
		let command_id = queue_start(&first, &run).await;
		let mut interrupted_adapter =
			RecordingAdapter::new(EffectResult::Unknown);
		let first_pass = first
			.reconcile_effects(
				&mut interrupted_adapter,
				EffectKindRecord::StartRun,
			)
			.await
			.unwrap();
		let interrupted = interrupted_adapter.executed[0].clone();
		let expected_interrupted = Effect {
			effect_id: interrupted.effect_id,
			command_id,
			kind: EffectKind::StartRun { run_id: run.run_id },
			safety: EffectSafety::Idempotent {
				external_key: interrupted.effect_id,
				max_attempts: 3,
			},
			state: EffectState::InFlight,
			attempt_count: 1,
		};

		assert_eq!(interrupted, expected_interrupted);
		assert_eq!(first_pass, vec![expected_interrupted.clone()]);
		drop(first);

		let second = start_core(&path).await;
		let mut retry_adapter = RecordingAdapter::new(EffectResult::Failed);
		let second_pass = second
			.reconcile_effects(&mut retry_adapter, EffectKindRecord::StartRun)
			.await
			.unwrap();
		let retried = retry_adapter.executed[0].clone();
		let expected_retried = Effect {
			attempt_count: 2,
			..expected_interrupted.clone()
		};

		assert_eq!(retry_adapter.reconciled, vec![expected_interrupted]);
		assert_eq!(retried, expected_retried);
		assert_eq!(
			second_pass,
			vec![Effect {
				state: EffectState::Failed,
				..expected_retried
			}]
		);
		drop(second);
		assert_no_work_remains(&path).await;
	}

	#[tokio::test]
	async fn an_ambiguous_interrupted_effect_becomes_outcome_unknown_without_retry()
	 {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let first = start_core(&path).await;
		let (_, run) = create_run(&first).await;
		first
			.store
			.write(async |tx| {
				tx.insert_effect(&NewEffect {
					effect_id: Uuid::now_v7(),
					command_id: Uuid::now_v7(),
					run_id: Some(run.run_id.0),
					promotion_id: None,
					terminal_id: None,
					kind: EffectKindRecord::StartRun,
					safety: EffectSafetyRecord::Ambiguous,
				})
				.await
			})
			.await
			.unwrap();
		let mut interrupted_adapter =
			RecordingAdapter::new(EffectResult::Unknown);
		first
			.reconcile_effects(
				&mut interrupted_adapter,
				EffectKindRecord::StartRun,
			)
			.await
			.unwrap();
		let interrupted = interrupted_adapter.executed[0].clone();
		drop(first);

		let second = start_core(&path).await;
		let mut unknown_adapter =
			RecordingAdapter::new(EffectResult::Completed);
		let second_pass = second
			.reconcile_effects(&mut unknown_adapter, EffectKindRecord::StartRun)
			.await
			.unwrap();

		assert_eq!(unknown_adapter.reconciled, vec![interrupted.clone()]);
		assert_eq!(unknown_adapter.executed, vec![]);
		assert_eq!(
			second_pass,
			vec![Effect {
				state: EffectState::OutcomeUnknown,
				..interrupted
			}]
		);
		drop(second);
		assert_no_work_remains(&path).await;
	}
}
