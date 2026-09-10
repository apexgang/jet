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
	if let AutoContinuePolicy::Retry { message, .. } = &mut replacement {
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
async fn quota_evidence_without_structured_wait_cannot_authorize_retry() {
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
	assert_eq!(after.due_at_unix_ms, before.observed_at_unix_ms + 3_610_000);
	assert_eq!(sources(&h).await, vec![TurnSource::AutoContinue]);
	h.tick(50).await;
	assert!(h.host.inputs.lock().unwrap().is_empty());
}
#[tokio::test]
async fn scheduled_work_does_not_reuse_a_consumed_one_shot_policy() {
	let dir = tempfile::tempdir().unwrap();
	let mut h =
		driver::Harness::start(dir.path(), AutoContinuePolicy::Off).await;
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
async fn consumption_breakdown_does_not_change_the_native_model_selection() {
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
		driver::Harness::start(dir.path(), AutoContinuePolicy::Off).await;
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
async fn repeated_reports_without_reset_keep_the_original_fallback_deadline() {
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
