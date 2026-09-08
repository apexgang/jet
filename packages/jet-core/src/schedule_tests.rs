use crate::test_support::{
	FixedProbe, ManualClock, actor, equipped, request, start_core_with,
};
use crate::{Command, CommandOutcome, RetentionPolicy, WorkingTreeRequest};
use pretty_assertions::assert_eq;
use std::time::SystemTime;

#[tokio::test]
async fn schedule_selects_first_valid_instant_and_preserves_it_after_restart() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	// 2026-03-08 00:00 UTC, before the New York spring transition.
	let clock = ManualClock::at(
		SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1772928000),
	);
	let core =
		start_core_with(&path, clock.clone(), FixedProbe::new(equipped()))
			.await;
	let CommandOutcome::ConversationCreated(conversation) = core
		.execute(
			&actor(),
			request(Command::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRequest::NoProject,
			}),
		)
		.await
		.unwrap()
	else {
		panic!("Conversation expected")
	};
	let command = request(Command::CreateSchedule {
		conversation_id: conversation.conversation_id,
		time_zone: "America/New_York".into(),
		local_time: "02:30:00".into(),
		prompt: "Continue".into(),
	});
	let outcome = core.execute(&actor(), command.clone()).await.unwrap();
	let CommandOutcome::ScheduleCreated(ref schedule) = outcome else {
		panic!("Schedule expected")
	};
	assert_eq!(
		(
			&schedule.next.intended_local[..],
			schedule.next.due_at_unix_ms
		),
		("2026-03-08T02:30:00", 1772953200000)
	);
	core.close().await;
	let core = start_core_with(&path, clock, FixedProbe::new(equipped())).await;
	assert_eq!(core.execute(&actor(), command).await.unwrap(), outcome);
}

async fn setup(
	now: &str,
) -> (
	tempfile::TempDir,
	crate::Core,
	std::sync::Arc<ManualClock>,
	crate::ConversationId,
) {
	let dir = tempfile::tempdir().unwrap();
	let time = chrono::DateTime::parse_from_rfc3339(now).unwrap();
	let clock = ManualClock::at(
		SystemTime::UNIX_EPOCH
			+ std::time::Duration::from_millis(time.timestamp_millis() as u64),
	);
	let core = start_core_with(
		&dir.path().join("plane.sqlite3"),
		clock.clone(),
		FixedProbe::new(equipped()),
	)
	.await;
	let CommandOutcome::ConversationCreated(conversation) = core
		.execute(
			&actor(),
			request(Command::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRequest::NoProject,
			}),
		)
		.await
		.unwrap()
	else {
		panic!("Conversation expected")
	};
	(dir, core, clock, conversation.conversation_id)
}
async fn schedule(
	core: &crate::Core,
	id: crate::ConversationId,
	zone: &str,
	time: &str,
) -> crate::ScheduledTask {
	let CommandOutcome::ScheduleCreated(task) = core
		.execute(
			&actor(),
			request(Command::CreateSchedule {
				conversation_id: id,
				time_zone: zone.into(),
				local_time: time.into(),
				prompt: "Continue".into(),
			}),
		)
		.await
		.unwrap()
	else {
		panic!("Schedule expected")
	};
	task
}
async fn queue(
	core: &crate::Core,
	id: crate::ConversationId,
) -> crate::TurnQueue {
	let crate::QueryResult::TurnQueue(queue) = core
		.query(
			&actor(),
			crate::Query::TurnQueue {
				conversation_id: id,
			},
		)
		.await
		.unwrap()
	else {
		panic!("Queue expected")
	};
	queue
}
#[tokio::test]
async fn schedule_folds_fire_once_at_the_earlier_instant() {
	let (dir, core, clock, id) = setup("2026-11-01T00:00:00Z").await;
	let task = schedule(&core, id, "America/New_York", "01:30:00").await;
	assert_eq!(task.next.due_at_unix_ms, 1793511000000); // 05:30 UTC
	clock.advance(std::time::Duration::from_secs(5 * 3600 + 30 * 60));
	core.perform_schedules().await.unwrap();
	let first = queue(&core, id).await.turns;
	assert_eq!(
		first.iter().map(|t| t.turn_id).collect::<Vec<_>>(),
		vec![task.next.firing_id]
	);
	core.close().await;
	clock.advance(std::time::Duration::from_secs(3600));
	let core = start_core_with(
		&dir.path().join("plane.sqlite3"),
		clock,
		FixedProbe::new(equipped()),
	)
	.await;
	core.perform_schedules().await.unwrap();
	assert_eq!(queue(&core, id).await.turns, first);
}

#[tokio::test]
async fn schedule_offline_catch_up_keeps_newest_and_journals_every_outcome() {
	let (dir, core, clock, id) = setup("2026-01-01T00:00:00Z").await;
	let task = schedule(&core, id, "UTC", "12:00:00").await;
	core.execute(
		&actor(),
		request(Command::SubmitTurn {
			conversation_id: id,
			source: crate::TurnSource::User,
			prompt: "Keep user work".into(),
		}),
	)
	.await
	.unwrap();
	core.close().await;
	clock.advance(std::time::Duration::from_secs(10 * 86400));
	let core = start_core_with(
		&dir.path().join("plane.sqlite3"),
		clock.clone(),
		FixedProbe::new(equipped()),
	)
	.await;
	core.perform_schedules().await.unwrap();
	let turns = queue(&core, id).await.turns;
	assert_eq!(
		turns
			.iter()
			.map(|t| (t.sequence, t.source))
			.collect::<Vec<_>>(),
		vec![
			(1, crate::TurnSource::User),
			(2, crate::TurnSource::Schedule)
		]
	);
	let crate::QueryResult::Events(page) = core
		.query(
			&actor(),
			crate::Query::Events {
				after: crate::EventSequence(0),
			},
		)
		.await
		.unwrap()
	else {
		panic!("Events expected")
	};
	let firings = page
		.events
		.iter()
		.filter_map(|event| match &event.kind {
			crate::EventKind::ScheduleFired {
				schedule_id,
				firing,
				outcome,
			} => {
				assert_eq!(
					event.actor,
					crate::EventActor::ScheduledTask {
						schedule_id: *schedule_id,
						authorized_by: task.authorized_by
					}
				);
				Some((firing.intended_local.clone(), *outcome))
			}
			_ => None,
		})
		.collect::<Vec<_>>();
	assert_eq!(
		firings,
		vec![
			(
				"2026-01-01T12:00:00".into(),
				crate::ScheduleFiringOutcome::Expired
			),
			(
				"2026-01-02T12:00:00".into(),
				crate::ScheduleFiringOutcome::Expired
			),
			(
				"2026-01-03T12:00:00".into(),
				crate::ScheduleFiringOutcome::Expired
			),
			(
				"2026-01-04T12:00:00".into(),
				crate::ScheduleFiringOutcome::Superseded
			),
			(
				"2026-01-05T12:00:00".into(),
				crate::ScheduleFiringOutcome::Superseded
			),
			(
				"2026-01-06T12:00:00".into(),
				crate::ScheduleFiringOutcome::Superseded
			),
			(
				"2026-01-07T12:00:00".into(),
				crate::ScheduleFiringOutcome::Superseded
			),
			(
				"2026-01-08T12:00:00".into(),
				crate::ScheduleFiringOutcome::Superseded
			),
			(
				"2026-01-09T12:00:00".into(),
				crate::ScheduleFiringOutcome::Superseded
			),
			(
				"2026-01-10T12:00:00".into(),
				crate::ScheduleFiringOutcome::Queued
			),
		]
	);
	core.perform_schedules().await.unwrap();
	assert_eq!(queue(&core, id).await.turns, turns);
	clock.advance(std::time::Duration::from_secs(86400));
	core.perform_schedules().await.unwrap();
	let replacement = queue(&core, id).await.turns;
	assert_eq!(
		(replacement[0].clone(), replacement[1].sequence),
		(turns[0].clone(), 3)
	);
	let cancel = request(Command::CancelSchedule {
		schedule_id: task.schedule_id,
	});
	let canceled = core.execute(&actor(), cancel.clone()).await.unwrap();
	assert_eq!(queue(&core, id).await.turns, vec![turns[0].clone()]);
	assert_eq!(core.execute(&actor(), cancel).await.unwrap(), canceled);
	let crate::QueryResult::ScheduledTasks(snapshot) = core
		.query(
			&actor(),
			crate::Query::ScheduledTasks {
				conversation_id: id,
			},
		)
		.await
		.unwrap()
	else {
		panic!("Schedules expected")
	};
	assert!(snapshot.tasks.is_empty());
}

#[tokio::test]
async fn schedule_handles_half_hour_and_whole_date_gaps() {
	for (now, zone, local, expected) in [
		(
			"2026-10-03T12:00:00Z",
			"Australia/Lord_Howe",
			"02:15:00",
			"2026-10-03T15:30:00Z",
		),
		(
			"2011-12-30T00:00:00Z",
			"Pacific/Apia",
			"12:00:00",
			"2011-12-30T10:00:00Z",
		),
	] {
		let (_dir, core, _clock, id) = setup(now).await;
		let task = schedule(&core, id, zone, local).await;
		assert_eq!(
			task.next.due_at_unix_ms,
			chrono::DateTime::parse_from_rfc3339(expected)
				.unwrap()
				.timestamp_millis()
		);
	}
}

#[tokio::test]
async fn schedule_waits_for_queue_capacity_without_discarding_user_turns() {
	let (_dir, core, clock, id) = setup("2026-01-01T00:00:00Z").await;
	let task = schedule(&core, id, "UTC", "12:00:00").await;
	for _ in 0..128 {
		core.execute(
			&actor(),
			request(Command::SubmitTurn {
				conversation_id: id,
				source: crate::TurnSource::User,
				prompt: "User work".into(),
			}),
		)
		.await
		.unwrap();
	}
	let users = queue(&core, id).await.turns;
	clock.advance(std::time::Duration::from_secs(86400));
	core.perform_schedules().await.unwrap();
	assert_eq!(queue(&core, id).await.turns, users);
	core.execute(
		&actor(),
		request(Command::WithdrawTurn {
			conversation_id: id,
			turn_id: users[0].turn_id,
		}),
	)
	.await
	.unwrap();
	core.perform_schedules().await.unwrap();
	let admitted = queue(&core, id).await.turns;
	assert_eq!(&admitted[..127], &users[1..]);
	assert_eq!(
		(admitted[127].turn_id, admitted[127].sequence),
		(task.next.firing_id, 129)
	);
}
