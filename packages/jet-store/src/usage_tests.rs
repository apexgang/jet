use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{
	NewUsageObservation, ProviderReachRecord, QuotaScopeRecord,
	QuotaUnitRecord, UsageEstimationRecord, UsageFinalityRecord,
	UsageProviderReachRecord, UsageQuotaHeartbeatRecord,
	UsageQuotaSnapshotRecord, UsageScopeRecord, UsageSelectionRecord,
	UsageTokensRecord, UsageTotalRecord,
};
use crate::{
	ConversationOriginRecord, NewConversation, NewRun, RetentionPolicy, Store,
	WorkingTreeRecord,
};

const NOW_UNIX_MS: i64 = 1_700_000_000_000;

async fn open(dir: &tempfile::TempDir) -> Store {
	Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap()
}

/// One Conversation with one Run, so Usage rows have the identities their
/// foreign keys require.
async fn conversation(store: &Store) -> (Uuid, Uuid) {
	let conversation_id = Uuid::now_v7();
	let run_id = Uuid::now_v7();
	store
		.write(async |tx| {
			tx.insert_conversation(NewConversation {
				conversation_id,
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRecord::NoProject,
				origin: ConversationOriginRecord::New,
				created_at_unix_ms: NOW_UNIX_MS,
			})
			.await?;
			tx.insert_run(NewRun {
				run_id,
				conversation_id,
				created_at_unix_ms: NOW_UNIX_MS,
			})
			.await
		})
		.await
		.unwrap();
	(conversation_id, run_id)
}

fn observation(
	conversation_id: Uuid,
	run_id: Uuid,
	measurement: &str,
	tokens: UsageTokensRecord,
) -> NewUsageObservation {
	NewUsageObservation {
		observation_id: Uuid::now_v7(),
		conversation_id,
		run_id,
		measurement: measurement.into(),
		native_usage_id: None,
		binding_id: None,
		provider: Some("anthropic".into()),
		model: Some("claude-opus-5".into()),
		scope: UsageScopeRecord::Turn,
		estimation: UsageEstimationRecord::Measured,
		finality: UsageFinalityRecord::Final,
		tokens,
		observed_at_unix_ms: NOW_UNIX_MS,
	}
}

fn tokens(input: u64, output: u64) -> UsageTokensRecord {
	UsageTokensRecord {
		input,
		cached_input: 0,
		output,
		reasoning: 0,
	}
}

fn snapshot(
	binding_id: Uuid,
	window_id: &str,
	used: u64,
	observed_at_unix_ms: i64,
) -> UsageQuotaSnapshotRecord {
	UsageQuotaSnapshotRecord {
		snapshot_id: Uuid::now_v7(),
		binding_id,
		provider: "anthropic".into(),
		window_id: window_id.into(),
		scope: QuotaScopeRecord::ProviderAccount,
		model: None,
		conversation_id: None,
		run_id: None,
		unit: QuotaUnitRecord::Share,
		used,
		limit_amount: Some(10_000),
		window_seconds: Some(18_000),
		resets_at_unix_ms: Some(observed_at_unix_ms + 3_600_000),
		estimation: UsageEstimationRecord::Measured,
		finality: UsageFinalityRecord::Interim,
		observed_at_unix_ms,
		digest: [u8::try_from(used % 256).unwrap_or_default(); 32],
	}
}

/// A measurement repeated under its own identity replaces its row, so the
/// same tokens are never counted twice (ADR-0023).
#[tokio::test]
async fn a_repeated_measurement_replaces_rather_than_adds() {
	let dir = tempfile::tempdir().unwrap();
	let store = open(&dir).await;
	let (conversation_id, run_id) = conversation(&store).await;
	let mut second =
		observation(conversation_id, run_id, "turn-1", tokens(30, 20));
	second.observation_id = Uuid::now_v7();
	second.observed_at_unix_ms = NOW_UNIX_MS + 1;
	let totals = store
		.write(async |tx| {
			let first = tx
				.record_usage_observation(&observation(
					conversation_id,
					run_id,
					"turn-1",
					tokens(10, 5),
				))
				.await?;
			let replaced = tx.record_usage_observation(&second).await?;
			let totals = tx
				.usage_totals(UsageSelectionRecord {
					run_id: Some(run_id),
					..UsageSelectionRecord::default()
				})
				.await?;
			Ok::<_, crate::StoreError>((first, replaced, totals))
		})
		.await
		.unwrap();
	assert_eq!(
		totals,
		(
			true,
			true,
			vec![UsageTotalRecord {
				model: Some("claude-opus-5".into()),
				tokens: tokens(30, 20),
				measurements: 1,
				estimated: 0,
				interim: 0,
				first_observed_at_unix_ms: NOW_UNIX_MS + 1,
				last_observed_at_unix_ms: NOW_UNIX_MS + 1,
			}]
		)
	);
}

/// An out-of-order repeat leaves the newer measurement in place.
#[tokio::test]
async fn an_older_repeat_does_not_overwrite_a_newer_measurement() {
	let dir = tempfile::tempdir().unwrap();
	let store = open(&dir).await;
	let (conversation_id, run_id) = conversation(&store).await;
	let mut newer =
		observation(conversation_id, run_id, "turn-1", tokens(30, 20));
	newer.observed_at_unix_ms = NOW_UNIX_MS + 10;
	let outcome = store
		.write(async |tx| {
			tx.record_usage_observation(&newer).await?;
			let stored = tx
				.record_usage_observation(&observation(
					conversation_id,
					run_id,
					"turn-1",
					tokens(1, 1),
				))
				.await?;
			let totals =
				tx.usage_totals(UsageSelectionRecord::default()).await?;
			Ok::<_, crate::StoreError>((stored, totals))
		})
		.await
		.unwrap();
	assert_eq!(
		outcome,
		(
			false,
			vec![UsageTotalRecord {
				model: Some("claude-opus-5".into()),
				tokens: tokens(30, 20),
				measurements: 1,
				estimated: 0,
				interim: 0,
				first_observed_at_unix_ms: NOW_UNIX_MS + 10,
				last_observed_at_unix_ms: NOW_UNIX_MS + 10,
			}]
		)
	);
}

/// A Run whose Craft reports a cumulative total contributes that total
/// alone: its turns are not added to a number that already covers them.
#[tokio::test]
async fn a_cumulative_run_total_replaces_the_turns_it_covers() {
	let dir = tempfile::tempdir().unwrap();
	let store = open(&dir).await;
	let (conversation_id, run_id) = conversation(&store).await;
	let mut cumulative =
		observation(conversation_id, run_id, "run", tokens(45, 25));
	cumulative.scope = UsageScopeRecord::Run;
	cumulative.observed_at_unix_ms = NOW_UNIX_MS + 2;
	let totals = store
		.write(async |tx| {
			tx.record_usage_observation(&observation(
				conversation_id,
				run_id,
				"turn-1",
				tokens(10, 5),
			))
			.await?;
			tx.record_usage_observation(&observation(
				conversation_id,
				run_id,
				"turn-2",
				tokens(35, 20),
			))
			.await?;
			tx.record_usage_observation(&cumulative).await?;
			tx.usage_totals(UsageSelectionRecord {
				conversation_id: Some(conversation_id),
				..UsageSelectionRecord::default()
			})
			.await
		})
		.await
		.unwrap();
	assert_eq!(
		totals,
		vec![UsageTotalRecord {
			model: Some("claude-opus-5".into()),
			tokens: tokens(45, 25),
			measurements: 1,
			estimated: 0,
			interim: 0,
			first_observed_at_unix_ms: NOW_UNIX_MS + 2,
			last_observed_at_unix_ms: NOW_UNIX_MS + 2,
		}]
	);
}

/// Turn-scoped measurements of one Run are summed, and estimated or
/// still-moving measurements stay counted apart from the total.
#[tokio::test]
async fn turns_are_summed_and_their_uncertainty_stays_visible() {
	let dir = tempfile::tempdir().unwrap();
	let store = open(&dir).await;
	let (conversation_id, run_id) = conversation(&store).await;
	let mut estimated =
		observation(conversation_id, run_id, "turn-2", tokens(4, 1));
	estimated.estimation = UsageEstimationRecord::Estimated;
	estimated.finality = UsageFinalityRecord::Interim;
	estimated.observed_at_unix_ms = NOW_UNIX_MS + 5;
	let totals = store
		.write(async |tx| {
			tx.record_usage_observation(&observation(
				conversation_id,
				run_id,
				"turn-1",
				tokens(10, 5),
			))
			.await?;
			tx.record_usage_observation(&estimated).await?;
			tx.usage_totals(UsageSelectionRecord::default()).await
		})
		.await
		.unwrap();
	assert_eq!(
		totals,
		vec![UsageTotalRecord {
			model: Some("claude-opus-5".into()),
			tokens: tokens(14, 6),
			measurements: 2,
			estimated: 1,
			interim: 1,
			first_observed_at_unix_ms: NOW_UNIX_MS,
			last_observed_at_unix_ms: NOW_UNIX_MS + 5,
		}]
	);
}

/// Every window keeps its own freshest snapshot, and two windows of one
/// binding are never folded into one another (ADR-0023).
#[tokio::test]
async fn each_quota_window_reports_its_own_freshest_snapshot() {
	let dir = tempfile::tempdir().unwrap();
	let store = open(&dir).await;
	let binding_id = Uuid::now_v7();
	let newest = snapshot(binding_id, "five_hour", 4_200, NOW_UNIX_MS + 60_000);
	let weekly = snapshot(binding_id, "weekly", 1_000, NOW_UNIX_MS);
	let windows = store
		.write(async |tx| {
			tx.record_usage_quota_snapshot(&snapshot(
				binding_id,
				"five_hour",
				1_500,
				NOW_UNIX_MS,
			))
			.await?;
			tx.record_usage_quota_snapshot(&newest).await?;
			tx.record_usage_quota_snapshot(&weekly).await?;
			tx.usage_quota_windows(Some(binding_id)).await
		})
		.await
		.unwrap();
	assert_eq!(windows, vec![newest, weekly]);
}

/// Deduplication reads the newest snapshot of one window alone.
#[tokio::test]
async fn a_window_heartbeat_reads_its_newest_snapshot() {
	let dir = tempfile::tempdir().unwrap();
	let store = open(&dir).await;
	let binding_id = Uuid::now_v7();
	let heartbeat = store
		.write(async |tx| {
			tx.record_usage_quota_snapshot(&snapshot(
				binding_id,
				"five_hour",
				1_500,
				NOW_UNIX_MS,
			))
			.await?;
			tx.record_usage_quota_snapshot(&snapshot(
				binding_id,
				"five_hour",
				42,
				NOW_UNIX_MS + 1_000,
			))
			.await?;
			tx.usage_quota_heartbeat(binding_id, "five_hour").await
		})
		.await
		.unwrap();
	assert_eq!(
		heartbeat,
		Some(UsageQuotaHeartbeatRecord {
			digest: [42; 32],
			observed_at_unix_ms: NOW_UNIX_MS + 1_000,
		})
	);
}

/// The last thing the Plane learned about a Provider survives a restart,
/// so a Query after one still reports an unreachable Provider honestly.
#[tokio::test]
async fn provider_reach_outlives_the_daemon_that_recorded_it() {
	let dir = tempfile::tempdir().unwrap();
	let binding_id = Uuid::now_v7();
	let unreachable = UsageProviderReachRecord {
		binding_id,
		reach: ProviderReachRecord::Unreachable {
			reason: "the Harness reported no quota window".into(),
		},
		observed_at_unix_ms: NOW_UNIX_MS + 1,
	};
	let first = open(&dir).await;
	first
		.write(async |tx| {
			tx.record_usage_provider_reach(&UsageProviderReachRecord {
				binding_id,
				reach: ProviderReachRecord::Reachable,
				observed_at_unix_ms: NOW_UNIX_MS,
			})
			.await?;
			tx.record_usage_provider_reach(&unreachable).await
		})
		.await
		.unwrap();
	first.close().await;

	let second = open(&dir).await;
	let reach = second
		.read(async |tx| tx.usage_provider_reach(None).await)
		.await
		.unwrap();
	assert_eq!(reach, vec![unreachable]);
}
