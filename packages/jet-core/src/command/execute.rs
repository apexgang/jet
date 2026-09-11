//! Authorization, receipt replay, and durable Command execution.

use super::{
	Command, CommandEnvelope, CommandOutcome, TransactionContext, execute_new,
};
use crate::{
	Actor, Core,
	command::receipt::{
		COMMAND_RETENTION_MS, encode_result, outcome_version, replay,
	},
	error::CoreError,
	pairing::PairingDisclosure,
	security::SecurityState,
};
use jet_store::NewCommandReceipt;

impl Core {
	/// Executes `command` on behalf of `actor`. The outcome is durable when
	/// this returns `Ok`.
	///
	/// # Errors
	///
	/// Returns a `not_found` [`CoreError`] for unknown identities, a
	/// `conflict` one when the Command violates a lifecycle invariant, and
	/// a store category when the transaction cannot commit.
	pub async fn execute(
		&self,
		actor: &Actor,
		envelope: CommandEnvelope,
	) -> Result<CommandOutcome, CoreError> {
		// ASVS 8.3.2: no admission can slip between revocation's commit and
		// invalidating live authority, including a replayed Command receipt.
		let _access = self
			.remote_access
			.acquire_many(crate::remote::AUTHORITY_READERS)
			.await
			.expect("authority gate never closes");
		actor.authorize(&self.remote_sessions)?;
		let CommandEnvelope {
			command_id,
			command,
			request_digest,
		} = envelope;
		// Restoring a snapshot replaces the store a receipt would go to, so
		// it runs beside the pipeline; everything else waits while the
		// store is in doubt (ADR-0077).
		let command = match command {
			Command::RestoreRecoverySnapshot { snapshot } => {
				return self.restore_recovery_snapshot(actor, snapshot).await;
			}
			other => other,
		};
		if let crate::RecoveryMode::ReadOnly(reason) = self.recovery_mode() {
			return Err(crate::store_recovery::read_only(reason));
		}
		let actor_record = actor.record();
		let security = *self.security.read().await;
		let recorded_at_unix_ms = self.now_unix_ms();
		let prepared = self
			.admit(
				actor,
				command_id,
				&command,
				request_digest,
				security,
				recorded_at_unix_ms,
			)
			.await?;
		let mut invalidated_client = None;
		let outcome = self
			.store
			.write(async |tx| {
				tx.prune_command_receipts_before(
					recorded_at_unix_ms.saturating_sub(COMMAND_RETENTION_MS),
				)
				.await?;
				if let Some(receipt) =
					tx.command_receipt(actor_record, command_id.0).await?
				{
					return replay(
						receipt,
						request_digest,
						recorded_at_unix_ms,
					);
				}
				// ASVS 16.2.1: a Plane that cannot vouch for its Security
				// audit changes nothing worth recording until an owner has
				// dealt with it (ADR-0105). The check sits behind the
				// receipt above, because a retry of a Command that already
				// committed is not a new mutation (ADR-0093), and it takes
				// the transaction down with it rather than recording an
				// outcome the audit could not be trusted to hold.
				security.admit(command.security_class())?;

				let result = execute_new(
					tx,
					actor,
					command_id,
					command,
					prepared,
					TransactionContext {
						core: self,
						security,
						workspace_home: &self.workspace_home,
					},
					recorded_at_unix_ms,
				)
				.await;
				invalidated_client = result
					.as_ref()
					.ok()
					.and_then(crate::remote::invalidated_client);
				if let Some(client_id) = invalidated_client {
					tx.invalidate_remote_operations(&client_id.0.to_string())
						.await?;
				}
				if let Err(error) = &result
					&& !error.is_authoritative_result()
				{
					return Err(error.clone());
				}
				// An authoritative error is a durable answer, so its receipt
				// commits together with whatever the Command wrote before
				// raising it. Most write nothing; a refused Pairing claim
				// deliberately writes the attempt it counted, because a
				// Plane that rolled that back would let a client guess for
				// as long as the offer lasts. A Command whose partial
				// writes must not survive its own failure wraps them in a
				// savepoint.
				let outcome_version = outcome_version(&result);
				tx.insert_command_receipt(&NewCommandReceipt {
					actor: actor_record,
					command_id: command_id.0,
					request_digest,
					recorded_at_unix_ms,
					outcome_version,
					outcome: encode_result(&redacted_for_receipt(&result))?,
				})
				.await?;
				Ok(result)
			})
			.await;
		// SQLite may have committed before writing the external audit head
		// failed. Publish the safe direction before propagating either error.
		if let Some(client_id) = invalidated_client {
			self.remote_sessions.invalidate(client_id);
		}
		let outcome = outcome??;
		// Carrying on past an integrity failure is not the daemon deciding
		// it is well again: it validates the chain it now vouches for.
		if matches!(outcome, CommandOutcome::AuditEpochBegun { .. }) {
			*self.security.write().await =
				SecurityState::of(self.store.validate_audit().await?);
		}
		// The Command is durable and acknowledged by its receipt; the
		// index follows in its own transaction, and a start or a later
		// Command finishes what an interruption here leaves (ADR-0036).
		self.turn_wake.send_replace(());
		self.run_work.notify_one();
		self.maintenance_work.notify_one();
		self.utility_wake.notify_one();
		self.index_search().await?;
		Ok(outcome)
	}
}

/// What the durable receipt keeps of a Command's result.
///
/// Everything, except a secret the Plane discloses once: the receipt
/// outlives the offer it belongs to by thirty days (ADR-0093), and a
/// pairing code that lived for two minutes has no business being there.
/// The retry is answered with the offer, and its owner opens another one.
pub(super) fn redacted_for_receipt(
	result: &Result<CommandOutcome, CoreError>,
) -> Result<CommandOutcome, CoreError> {
	match result {
		Ok(CommandOutcome::PairingOpened { pending, .. }) => {
			Ok(CommandOutcome::PairingOpened {
				pending: pending.clone(),
				disclosure: PairingDisclosure::AlreadyDisclosed,
			})
		}
		Ok(
			outcome @ (CommandOutcome::ApprovalRetryAuthorized { .. }
			| CommandOutcome::RemoteToolReviewed { .. }
			| CommandOutcome::GitDeliveryAcknowledged { .. }
			| CommandOutcome::GitDeliveryQueued { .. }
			| CommandOutcome::UtilityQueued { .. }
			| CommandOutcome::ExtensionChangeQueued { .. }
			| CommandOutcome::CraftInstallationQueued { .. }
			| CommandOutcome::CraftDisabled { .. }
			| CommandOutcome::UserEditApplied(_)
			| CommandOutcome::ConversationNamed(_)
			| CommandOutcome::RunNamed(_)
			| CommandOutcome::TurnWithdrawn(_)
			| CommandOutcome::AutoContinueConfigured
			| CommandOutcome::ScheduleCreated(_)
			| CommandOutcome::ScheduleCanceled { .. }
			| CommandOutcome::TurnAdmitted(_)
			| CommandOutcome::ConversationCreated(_)
			| CommandOutcome::ExecutionResolutionRecorded(_)
			| CommandOutcome::RunControlAccepted { .. }
			| CommandOutcome::RunCreated(_)
			| CommandOutcome::RunTransitioned(_)
			| CommandOutcome::SettingSet { .. }
			| CommandOutcome::SettingCleared { .. }
			| CommandOutcome::AccountBound(_)
			| CommandOutcome::AccountUnbound { .. }
			| CommandOutcome::AuditEpochBegun { .. }
			| CommandOutcome::RecoverySnapshotRestored(_)
			| CommandOutcome::PairingGateSet { .. }
			| CommandOutcome::PairingClaimed { .. }
			| CommandOutcome::PairingConfirmed { .. }
			| CommandOutcome::PairingCompleted { .. }
			| CommandOutcome::PairedClientAccessSet { .. }
			| CommandOutcome::PairedClientRevoked { .. }
			| CommandOutcome::ProjectRegistered(_)
			| CommandOutcome::WorkspacePromotionRecorded(_)
			| CommandOutcome::Terminal(_)
			| CommandOutcome::ConversationImported(_)),
		) => Ok(outcome.clone()),
		Err(error) => Err(error.clone()),
	}
}

#[cfg(test)]
mod tests {
	use std::time::{Duration, SystemTime};

	use jet_store::{EventClass, NewEvent};
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use crate::test_support::{
		FixedProbe, ManualClock, actor, command_id, equipped, request,
		request_with_id, start_core, start_core_with,
	};
	use crate::{
		Command, CommandEnvelope, CommandOutcome, Conversation, ConversationId,
		ConversationOrigin, ConversationSnapshot, Core, CoreError,
		ErrorCategory, EventKind, EventPage, EventPayload, EventSequence,
		Query, QueryResult, RetentionPolicy, Revision, Run, RunId,
		RunLifecycle, WorkingTree, WorkingTreeRequest,
	};

	async fn create_conversation(
		core: &Core,
		retention: RetentionPolicy,
	) -> Conversation {
		let outcome = core
			.execute(
				&actor(),
				request(Command::CreateConversation {
					retention,
					working_tree: WorkingTreeRequest::NoProject,
				}),
			)
			.await
			.unwrap();
		let CommandOutcome::ConversationCreated(conversation) = outcome else {
			panic!("expected CommandOutcome::ConversationCreated");
		};
		conversation
	}

	async fn create_run(core: &Core, conversation_id: ConversationId) -> Run {
		let outcome = core
			.execute(&actor(), request(Command::CreateRun { conversation_id }))
			.await
			.unwrap();
		let CommandOutcome::RunCreated(run) = outcome else {
			panic!("expected CommandOutcome::RunCreated");
		};
		run
	}

	async fn transition(core: &Core, run: Run, lifecycle: RunLifecycle) -> Run {
		let outcome = core
			.execute(
				&actor(),
				request(Command::TransitionRun {
					run_id: run.run_id,
					expected_revision: run.revision,
					lifecycle,
				}),
			)
			.await
			.unwrap();
		let CommandOutcome::RunTransitioned(run) = outcome else {
			panic!("expected CommandOutcome::RunTransitioned");
		};
		run
	}

	async fn snapshot(
		core: &Core,
		conversation_id: ConversationId,
	) -> ConversationSnapshot {
		let result = core
			.query(&actor(), Query::Conversation { conversation_id })
			.await
			.unwrap();
		let QueryResult::Conversation(snapshot) = result else {
			panic!("expected QueryResult::Conversation");
		};
		*snapshot
	}

	async fn events_after(core: &Core, after: EventSequence) -> EventPage {
		let result =
			core.query(&actor(), Query::Events { after }).await.unwrap();
		let QueryResult::Events(page) = result else {
			panic!("expected QueryResult::Events");
		};
		page
	}

	async fn event_kinds(
		core: &Core,
		after: EventSequence,
	) -> Vec<(u64, EventKind)> {
		events_after(core, after)
			.await
			.events
			.into_iter()
			.map(|event| (event.sequence.0, event.kind))
			.collect()
	}

	fn not_found(code: &str, message: &str) -> CoreError {
		CoreError {
			category: ErrorCategory::NotFound,
			code: code.into(),
			retryable: false,
			message: message.into(),
			detail: None,
			revision_conflict: None,
			recovery_actions: vec![],
		}
	}

	#[tokio::test]
	async fn a_conversation_exists_and_is_queryable_before_any_run() {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core(&dir.path().join("p.sqlite3")).await;

		let conversation =
			create_conversation(&core, RetentionPolicy::Retain).await;
		let conversation_name = conversation.name.clone();

		assert_eq!(
			snapshot(&core, conversation.conversation_id).await,
			ConversationSnapshot {
				cursor: EventSequence(1),
				conversation,
				workspace: None,
				runs: vec![],
			}
		);
		assert_eq!(
			event_kinds(&core, EventSequence(0)).await,
			vec![(
				1,
				EventKind::ConversationCreated {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTree::NoProject,
					origin: ConversationOrigin::New,
					name: Some(conversation_name),
				}
			)]
		);
	}

	#[tokio::test]
	async fn a_conversation_retains_its_terminal_runs_across_core_restarts() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("p.sqlite3");
		let first = start_core(&path).await;
		let conversation =
			create_conversation(&first, RetentionPolicy::Retain).await;
		let conversation_id = conversation.conversation_id;

		let run = create_run(&first, conversation_id).await;
		let run = transition(&first, run, RunLifecycle::Starting).await;
		let run = transition(&first, run, RunLifecycle::Active).await;
		let completed = transition(&first, run, RunLifecycle::Completed).await;
		let second_run = create_run(&first, conversation_id).await;
		let canceled =
			transition(&first, second_run, RunLifecycle::Canceled).await;
		drop(first);

		let second = start_core(&path).await;
		let restored = snapshot(&second, conversation_id).await;
		let third_run = create_run(&second, conversation_id).await;
		let kinds = event_kinds(&second, EventSequence(6)).await;
		assert!(completed.ended_at.is_some() && canceled.ended_at.is_some());

		assert_eq!(
			restored,
			ConversationSnapshot {
				cursor: EventSequence(7),
				conversation,
				workspace: None,
				runs: vec![completed, canceled],
			}
		);
		assert_eq!(
			kinds,
			vec![
				(
					7,
					EventKind::RunLifecycleChanged {
						from: RunLifecycle::Created,
						to: RunLifecycle::Canceled,
					}
				),
				(
					8,
					EventKind::RunCreated {
						name: Some(third_run.name.clone()),
					},
				),
			]
		);
		assert_eq!(
			third_run,
			Run {
				run_id: third_run.run_id,
				conversation_id,
				revision: Revision(1),
				lifecycle: RunLifecycle::Created,
				name: third_run.name.clone(),
				created_at: third_run.created_at,
				ended_at: None,
			}
		);
	}

	#[tokio::test]
	async fn a_second_run_is_refused_while_one_has_not_ended() {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core(&dir.path().join("p.sqlite3")).await;
		let conversation =
			create_conversation(&core, RetentionPolicy::Retain).await;
		let run = create_run(&core, conversation.conversation_id).await;
		let starting = transition(&core, run, RunLifecycle::Starting).await;

		let error = core
			.execute(
				&actor(),
				request(Command::CreateRun {
					conversation_id: conversation.conversation_id,
				}),
			)
			.await
			.unwrap_err();

		assert_eq!(
			error,
			CoreError {
				category: ErrorCategory::Conflict,
				code: "run.conversation_busy".into(),
				retryable: false,
				message:
					"the Conversation already has a Run that has not ended"
						.into(),
				detail: None,
				revision_conflict: None,
				recovery_actions: vec![],
			}
		);
		assert_eq!(
			snapshot(&core, conversation.conversation_id).await.runs,
			vec![starting]
		);
	}

	#[tokio::test]
	async fn a_run_lifecycle_only_moves_forward_and_never_leaves_a_terminal_state()
	 {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core(&dir.path().join("p.sqlite3")).await;
		let conversation =
			create_conversation(&core, RetentionPolicy::Retain).await;
		let run = create_run(&core, conversation.conversation_id).await;
		let refused = async |run: &Run, lifecycle: RunLifecycle| {
			core.execute(
				&actor(),
				request(Command::TransitionRun {
					run_id: run.run_id,
					expected_revision: run.revision,
					lifecycle,
				}),
			)
			.await
			.unwrap_err()
		};

		let skipped = refused(&run, RunLifecycle::Active).await;
		let never_active = refused(&run, RunLifecycle::Completed).await;
		let failed = transition(&core, run, RunLifecycle::Failed).await;
		let revived = refused(&failed, RunLifecycle::Active).await;

		let invalid = |message: &str| CoreError {
			category: ErrorCategory::Conflict,
			code: "run.invalid_transition".into(),
			retryable: false,
			message: message.into(),
			detail: None,
			revision_conflict: None,
			recovery_actions: vec![],
		};
		assert_eq!(
			(skipped, never_active, revived),
			(
				invalid("a created Run cannot move to active"),
				invalid("a created Run cannot move to completed"),
				invalid("a failed Run cannot move to active"),
			)
		);
	}

	#[tokio::test]
	async fn an_unknown_conversation_or_run_is_not_found() {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core(&dir.path().join("p.sqlite3")).await;
		let conversation_id = ConversationId(Uuid::now_v7());
		let run_id = RunId(Uuid::now_v7());

		let queried = core
			.query(&actor(), Query::Conversation { conversation_id })
			.await
			.unwrap_err();
		let run_created = core
			.execute(&actor(), request(Command::CreateRun { conversation_id }))
			.await
			.unwrap_err();
		let transitioned = core
			.execute(
				&actor(),
				request(Command::TransitionRun {
					run_id,
					expected_revision: Revision(1),
					lifecycle: RunLifecycle::Starting,
				}),
			)
			.await
			.unwrap_err();

		assert_eq!(
			(queried, run_created, transitioned),
			(
				not_found(
					"conversation.not_found",
					"the Conversation does not exist"
				),
				not_found(
					"conversation.not_found",
					"the Conversation does not exist"
				),
				not_found("run.not_found", "the Run does not exist"),
			)
		);
	}

	#[tokio::test]
	async fn a_command_identity_older_than_thirty_days_cannot_execute_again() {
		let dir = tempfile::tempdir().unwrap();
		let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
		let clock = ManualClock::at(start);
		let core = start_core_with(
			&dir.path().join("p.sqlite3"),
			clock.clone(),
			FixedProbe::new(equipped()),
		)
		.await;
		let command_id = command_id();
		let command = Command::CreateConversation {
			retention: RetentionPolicy::Retain,
			working_tree: WorkingTreeRequest::NoProject,
		};
		let original = core
			.execute(&actor(), request_with_id(command_id, command.clone()))
			.await
			.unwrap();
		clock.advance(Duration::from_hours(30 * 24));
		let within_window = core
			.execute(&actor(), request_with_id(command_id, command.clone()))
			.await
			.unwrap();
		clock.advance(Duration::from_millis(1));

		let error = core
			.execute(&actor(), request_with_id(command_id, command))
			.await
			.unwrap_err();

		assert_eq!(
			error,
			CoreError {
				category: ErrorCategory::InvalidInput,
				code: "command.identity_expired".into(),
				retryable: false,
				message: "the Command identity is older than thirty days"
					.into(),
				detail: None,
				revision_conflict: None,
				recovery_actions: vec![],
			}
		);
		let QueryResult::Conversations(conversations) =
			core.query(&actor(), Query::Conversations).await.unwrap()
		else {
			panic!("expected the Conversation list");
		};
		let CommandOutcome::ConversationCreated(original) = original else {
			panic!("expected the original Conversation");
		};
		assert_eq!(
			(within_window, conversations.conversations),
			(
				CommandOutcome::ConversationCreated(original.clone()),
				vec![Conversation {
					conversation_id: original.conversation_id,
					revision: original.revision,
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTree::NoProject,
					origin: ConversationOrigin::New,
					name: original.name.clone(),
					created_at: start,
				}]
			)
		);
	}

	#[tokio::test]
	async fn typed_command_content_is_bound_to_the_request_digest() {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core(&dir.path().join("p.sqlite3")).await;
		let command_id = command_id();
		core.execute(
			&actor(),
			CommandEnvelope::new(
				command_id,
				Command::CreateConversation {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRequest::NoProject,
				},
				b"same adapter bytes",
			)
			.unwrap(),
		)
		.await
		.unwrap();

		let error = core
			.execute(
				&actor(),
				CommandEnvelope::new(
					command_id,
					Command::CreateConversation {
						retention: RetentionPolicy::ForgetAfterFinalRun,
						working_tree: WorkingTreeRequest::NoProject,
					},
					b"same adapter bytes",
				)
				.unwrap(),
			)
			.await
			.unwrap_err();

		assert_eq!(
			error,
			CoreError {
				category: ErrorCategory::Conflict,
				code: "command.identity_reused".into(),
				retryable: false,
				message:
					"the Command identity was already used for different content"
						.into(),
				detail: None,
				revision_conflict: None,
				recovery_actions: vec![],
			}
		);
	}

	#[tokio::test]
	async fn events_written_by_a_newer_core_are_served_without_interpretation()
	{
		let dir = tempfile::tempdir().unwrap();
		let core = start_core(&dir.path().join("p.sqlite3")).await;
		let conversation =
			create_conversation(&core, RetentionPolicy::Retain).await;
		let future = EventPayload {
			kind: "run.teleported".into(),
			payload_version: 7,
			payload: serde_json::json!({"to": "another Plane"}),
		};
		core.store
			.write(async |tx| {
				tx.append_event(NewEvent {
					event_id: Uuid::now_v7(),
					actor: actor().record(),
					recorded_at_unix_ms: 0,
					conversation_id: Some(conversation.conversation_id.0),
					run_id: None,
					kind: future.kind.clone(),
					payload_version: future.payload_version,
					payload: future.payload.to_string(),
					class: EventClass::Semantic,
				})
				.await
			})
			.await
			.unwrap();

		let page = events_after(&core, EventSequence(1)).await;

		let kinds: Vec<_> =
			page.events.iter().map(|event| &event.kind).collect();
		assert_eq!(
			(page.cursor, kinds),
			(
				EventSequence(2),
				vec![&EventKind::Unrecognized(future.clone())]
			)
		);
		assert_eq!(
			EventKind::Unrecognized(future.clone()).encode().unwrap(),
			future
		);
	}
}
