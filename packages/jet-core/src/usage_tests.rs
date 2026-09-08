use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use jet_store::{
	ConversationOriginRecord, NewConversation, NewRun, WorkingTreeRecord,
};
use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::test_support::{
	FixedProbe, ManualClock, actor, equipped, events, request, start_core_with,
};
use crate::usage::{
	ObservedUsage, QuotaMeasure, QuotaReport, QuotaScope, QuotaUnit,
	UsageEstimation, UsageFinality, UsageFreshness, UsageMeasurement,
	UsageReport, UsageSelection, UsageSource, UsageTokens,
};
use crate::{
	AccountBindingId, ClientId, Command, CommandOutcome, Core,
	CredentialSource, EventActor, EventKind, ModelConsumption, ModelId,
	PlaneUsage, ProviderId, Query, QueryResult, RetentionPolicy, Run,
};

/// A fixed instant, so a window's freshness follows the test's own clock
/// rather than the machine's.
const NOW: Duration = Duration::from_millis(1_700_000_000_000);

async fn start(dir: &tempfile::TempDir) -> (Core, Arc<ManualClock>) {
	let clock = ManualClock::at(UNIX_EPOCH + NOW);
	let core = start_core_with(
		&dir.path().join("plane.sqlite3"),
		clock.clone(),
		FixedProbe::new(equipped()),
	)
	.await;
	(core, clock)
}

/// One Conversation with one Run, which is everything a Usage record needs
/// to name what it belongs to.
async fn run(core: &Core) -> Run {
	let conversation_id = Uuid::now_v7();
	let run_id = Uuid::now_v7();
	let now = core.now_unix_ms();
	core.store
		.write(async |tx| {
			tx.insert_conversation(NewConversation {
				conversation_id,
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRecord::NoProject,
				origin: ConversationOriginRecord::New,
				created_at_unix_ms: now,
			})
			.await?;
			tx.insert_run(NewRun {
				run_id,
				conversation_id,
				created_at_unix_ms: now,
			})
			.await
		})
		.await
		.unwrap()
		.into()
}

/// One Account binding, which is what a Provider-reported window belongs
/// to.
async fn bind(core: &Core) -> AccountBindingId {
	let CommandOutcome::AccountBound(binding) = core
		.execute(
			&actor(),
			request(Command::BindAccount {
				provider: ProviderId("anthropic".into()),
				label: "Work".into(),
				provider_account: None,
				credential_source: CredentialSource::HarnessNative,
			}),
		)
		.await
		.unwrap()
	else {
		panic!("Account binding")
	};
	binding.binding_id
}

async fn record(
	core: &Core,
	run: &Run,
	binding: Option<AccountBindingId>,
	report: UsageReport,
) {
	let actor = EventActor::Harness {
		run_id: run.run_id,
		authorized_by: ClientId(Uuid::nil()),
	};
	let now = core.now_unix_ms();
	core.store
		.write(async |tx| {
			crate::usage_record::record(tx, &actor, run, binding, report, now)
				.await
		})
		.await
		.unwrap();
}

async fn usage(core: &Core, selection: UsageSelection) -> PlaneUsage {
	let QueryResult::Usage(usage) = core
		.query(&actor(), Query::Usage { selection })
		.await
		.unwrap()
	else {
		panic!("Usage")
	};
	*usage
}

/// Every Usage Event the journal holds, which is how a client learns there
/// is something new to read.
async fn recorded(core: &Core) -> Vec<EventKind> {
	events(core)
		.await
		.into_iter()
		.filter(|event| matches!(event, EventKind::UsageRecorded { .. }))
		.collect()
}

fn observed(turn: &str, tokens: UsageTokens) -> UsageReport {
	UsageReport::Observed(ObservedUsage {
		measurement: UsageMeasurement::Turn {
			turn: turn.into(),
			native_usage_id: None,
		},
		model: Some(ModelId("claude-opus-5".into())),
		estimation: UsageEstimation::Measured,
		finality: UsageFinality::Final,
		tokens,
	})
}

fn window(used: u64) -> UsageReport {
	UsageReport::ProviderQuota(QuotaReport {
		window: "five_hour".into(),
		scope: QuotaScope::ProviderAccount,
		measure: QuotaMeasure {
			unit: QuotaUnit::Share,
			used,
			limit: None,
		},
		window_seconds: Some(18_000),
		resets_in_seconds: Some(3_600),
		estimation: UsageEstimation::Measured,
		finality: UsageFinality::Interim,
	})
}

fn tokens(input: u64, output: u64) -> UsageTokens {
	UsageTokens {
		input,
		cached_input: 0,
		output,
		reasoning: 0,
	}
}

/// Consumption is summed only after deduplication, and the Event journal
/// carries one record per stored measurement (ADR-0023).
#[tokio::test]
async fn observed_consumption_is_recorded_once_per_measurement() {
	let dir = tempfile::tempdir().unwrap();
	let (core, _clock) = start(&dir).await;
	let run = run(&core).await;
	record(&core, &run, None, observed("turn-1", tokens(10, 5))).await;
	record(&core, &run, None, observed("turn-2", tokens(4, 1))).await;
	// The same turn reported again replaces what it already covers.
	record(&core, &run, None, observed("turn-2", tokens(6, 2))).await;
	let usage = usage(&core, UsageSelection::Run(run.run_id)).await;
	assert_eq!(
		(
			usage.consumption.tokens,
			usage.consumption.models,
			recorded(&core).await.len()
		),
		(
			tokens(16, 7),
			vec![ModelConsumption {
				model: Some(ModelId("claude-opus-5".into())),
				tokens: tokens(16, 7),
				measurements: 2,
				estimated: 0,
				interim: 0,
				last_observed_at: UNIX_EPOCH + NOW,
			}],
			3
		)
	);
}

/// A Provider that repeats itself is a heartbeat, not another snapshot,
/// and a Provider that changed is a new one (ADR-0045).
#[tokio::test]
async fn an_unchanged_provider_response_does_not_become_another_snapshot() {
	let dir = tempfile::tempdir().unwrap();
	let (core, clock) = start(&dir).await;
	let run = run(&core).await;
	let binding = bind(&core).await;
	record(&core, &run, Some(binding), window(2_500)).await;
	clock.advance(Duration::from_secs(60));
	record(&core, &run, Some(binding), window(2_500)).await;
	let unchanged = recorded(&core).await.len();
	clock.advance(Duration::from_secs(60));
	record(&core, &run, Some(binding), window(4_000)).await;
	let usage = usage(&core, UsageSelection::Binding(binding)).await;
	let window = usage.quota_windows.first().expect("one window");
	assert_eq!(
		(
			unchanged,
			recorded(&core).await.len(),
			usage.quota_windows.len(),
			window.measure,
			window.freshness.clone()
		),
		(
			1,
			2,
			1,
			QuotaMeasure {
				unit: QuotaUnit::Share,
				used: 4_000,
				// A share always reports against its fixed limit, whatever
				// the Craft sent.
				limit: Some(10_000),
			},
			UsageFreshness::Fresh
		)
	);
}

/// Past the interval a Plane refreshes on, a window is history rather than
/// a current reading (ADR-0045).
#[tokio::test]
async fn a_window_the_provider_has_not_confirmed_recently_is_stale() {
	let dir = tempfile::tempdir().unwrap();
	let (core, clock) = start(&dir).await;
	let run = run(&core).await;
	let binding = bind(&core).await;
	record(&core, &run, Some(binding), window(2_500)).await;
	clock.advance(Duration::from_secs(16 * 60));
	let usage = usage(&core, UsageSelection::Binding(binding)).await;
	assert_eq!(
		usage
			.quota_windows
			.into_iter()
			.map(|window| window.freshness)
			.collect::<Vec<_>>(),
		vec![UsageFreshness::Stale]
	);
}

/// A Provider that would not answer is reported as unreachable rather than
/// as the window it last reported (ADR-0023).
#[tokio::test]
async fn a_provider_that_would_not_answer_is_reported_as_unreachable() {
	let dir = tempfile::tempdir().unwrap();
	let (core, clock) = start(&dir).await;
	let run = run(&core).await;
	let binding = bind(&core).await;
	record(&core, &run, Some(binding), window(2_500)).await;
	clock.advance(Duration::from_secs(60));
	record(
		&core,
		&run,
		Some(binding),
		UsageReport::ProviderUnreachable {
			reason: "the Harness reported no quota window".into(),
		},
	)
	.await;
	let usage = usage(&core, UsageSelection::Binding(binding)).await;
	let window = usage.quota_windows.first().expect("one window");
	assert_eq!(
		(window.measure.used, window.freshness.clone()),
		(
			2_500,
			UsageFreshness::Unreachable {
				reason: "the Harness reported no quota window".into()
			}
		)
	);
}

/// A quota window belongs to an Account binding. A Run that named none
/// leaves the window unrecorded rather than attaching it to a guess.
#[tokio::test]
async fn a_quota_window_without_an_account_binding_is_not_recorded() {
	let dir = tempfile::tempdir().unwrap();
	let (core, _clock) = start(&dir).await;
	let run = run(&core).await;
	record(&core, &run, None, window(2_500)).await;
	let usage = usage(&core, UsageSelection::Plane).await;
	assert_eq!(
		(usage.quota_windows, recorded(&core).await),
		(vec![], vec![])
	);
}

/// A Conversation answers with what it consumed and leaves its account's
/// windows to the account they belong to.
#[tokio::test]
async fn a_conversation_answers_with_consumption_alone() {
	let dir = tempfile::tempdir().unwrap();
	let (core, _clock) = start(&dir).await;
	let run = run(&core).await;
	let binding = bind(&core).await;
	record(&core, &run, Some(binding), window(2_500)).await;
	record(
		&core,
		&run,
		Some(binding),
		observed("turn-1", tokens(10, 5)),
	)
	.await;
	let usage =
		usage(&core, UsageSelection::Conversation(run.conversation_id)).await;
	assert_eq!(
		(usage.quota_windows, usage.consumption.tokens),
		(vec![], tokens(10, 5))
	);
}

/// The Event says what was recorded, so a client knows to read the Query
/// again without the numbers being repeated into the journal.
#[tokio::test]
async fn the_journal_names_what_was_recorded_and_where_it_belongs() {
	let dir = tempfile::tempdir().unwrap();
	let (core, _clock) = start(&dir).await;
	let run = run(&core).await;
	let binding = bind(&core).await;
	record(&core, &run, Some(binding), window(2_500)).await;
	record(&core, &run, None, observed("turn-1", tokens(10, 5))).await;
	assert_eq!(
		recorded(&core).await,
		vec![
			EventKind::UsageRecorded {
				source: UsageSource::ProviderQuota,
				binding_id: Some(binding),
			},
			EventKind::UsageRecorded {
				source: UsageSource::JetObserved,
				binding_id: None,
			}
		]
	);
}

/// Bounded metadata is what a record carries; a Craft cannot make one out
/// of a payload.
#[tokio::test]
async fn a_report_that_is_not_bounded_metadata_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let (core, _clock) = start(&dir).await;
	let run = run(&core).await;
	let actor = EventActor::Harness {
		run_id: run.run_id,
		authorized_by: ClientId(Uuid::nil()),
	};
	let now = core.now_unix_ms();
	let refused = core
		.store
		.write(async |tx| {
			crate::usage_record::record(
				tx,
				&actor,
				&run,
				None,
				observed(&"t".repeat(129), tokens(1, 1)),
				now,
			)
			.await
		})
		.await
		.expect_err("bounded metadata");
	assert_eq!(refused.code, "usage.turn_unsupported");
}

/// A Plane answers for itself. Its snapshot names the Plane it came from
/// so a client never reads it as an account-wide total (ADR-0016).
#[tokio::test]
async fn a_snapshot_names_the_plane_it_covers() {
	let dir = tempfile::tempdir().unwrap();
	let (core, _clock) = start(&dir).await;
	let plane_id = core
		.store
		.read(async |tx| tx.plane().await)
		.await
		.unwrap()
		.plane_id;
	let usage = usage(&core, UsageSelection::Plane).await;
	assert_eq!(usage.plane_id.0, plane_id);
}
