use crate::test_support::{actor, bind_native_account, request, start_core};
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

use crate::checkpoint_tests::start_answering;
use crate::test_support::register_repository;
use crate::{
	AutoContinueStatus, ConversationId, Core, QuotaMeasure, QuotaReport,
	QuotaScope, QuotaUnit, RetentionPolicy, RunActivity, RunObservation,
	TurnSource, UsageEstimation, UsageFinality, UsageReport, VisaRunRequest,
	WorkingTreeRequest,
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
async fn retry(core: &Core, id: ConversationId) -> crate::AutoContinueSnapshot {
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
async fn structured_exhaustion_admits_one_retry_and_user_input_cancels_it() {
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
	let project_id = register_repository(&core, &dir.path().join("repo")).await;
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
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
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

#[path = "auto_continue_test_support.rs"]
mod driver;

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

#[path = "auto_continue_queue_tests.rs"]
mod queue_tests;
