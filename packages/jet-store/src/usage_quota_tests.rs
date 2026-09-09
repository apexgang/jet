use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{
	ProviderReachRecord, QuotaScopeRecord, QuotaUnitRecord,
	UsageProviderReachRecord, UsageQuotaHeartbeatRecord,
	UsageQuotaSnapshotRecord,
};
use crate::{Store, UsageEstimationRecord, UsageFinalityRecord};

const NOW_UNIX_MS: i64 = 1_700_000_000_000;

async fn open(dir: &tempfile::TempDir) -> Store {
	Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap()
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
		answered_at_unix_ms: observed_at_unix_ms,
		digest: [u8::try_from(used % 256).unwrap_or_default(); 32],
	}
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

/// A Provider may spell one window's name the same way for two Models.
/// They are two windows, and each keeps its own freshest snapshot
/// (ADR-0023).
#[tokio::test]
async fn one_window_name_under_two_models_is_two_windows() {
	let dir = tempfile::tempdir().unwrap();
	let store = open(&dir).await;
	let binding_id = Uuid::now_v7();
	let opus = UsageQuotaSnapshotRecord {
		scope: QuotaScopeRecord::Model,
		model: Some("claude-opus-5".into()),
		..snapshot(binding_id, "five_hour", 1_500, NOW_UNIX_MS)
	};
	let haiku = UsageQuotaSnapshotRecord {
		scope: QuotaScopeRecord::Model,
		model: Some("claude-haiku-4-5".into()),
		..snapshot(binding_id, "five_hour", 9_000, NOW_UNIX_MS + 1_000)
	};
	let windows = store
		.write(async |tx| {
			tx.record_usage_quota_snapshot(&opus).await?;
			tx.record_usage_quota_snapshot(&haiku).await?;
			tx.usage_quota_windows(Some(binding_id)).await
		})
		.await
		.unwrap();
	assert_eq!(windows, vec![haiku, opus]);
}

/// Deduplication reads the newest snapshot of one window alone.
#[tokio::test]
async fn a_window_heartbeat_reads_its_newest_snapshot() {
	let dir = tempfile::tempdir().unwrap();
	let store = open(&dir).await;
	let binding_id = Uuid::now_v7();
	let newest = snapshot(binding_id, "five_hour", 42, NOW_UNIX_MS + 1_000);
	let heartbeat = store
		.write(async |tx| {
			tx.record_usage_quota_snapshot(&snapshot(
				binding_id,
				"five_hour",
				1_500,
				NOW_UNIX_MS,
			))
			.await?;
			tx.record_usage_quota_snapshot(&newest).await?;
			tx.usage_quota_heartbeat(binding_id, "five_hour", None)
				.await
		})
		.await
		.unwrap();
	assert_eq!(
		heartbeat,
		Some(UsageQuotaHeartbeatRecord {
			snapshot_id: newest.snapshot_id,
			digest: [42; 32],
			observed_at_unix_ms: NOW_UNIX_MS + 1_000,
		})
	);
}

/// A repeated answer confirms the window it repeats without storing
/// another snapshot of it, and without confirming any other window
/// (ADR-0045).
#[tokio::test]
async fn an_answer_confirms_one_window_without_adding_a_snapshot() {
	let dir = tempfile::tempdir().unwrap();
	let store = open(&dir).await;
	let binding_id = Uuid::now_v7();
	let five_hour = snapshot(binding_id, "five_hour", 4_200, NOW_UNIX_MS);
	let weekly = snapshot(binding_id, "weekly", 1_000, NOW_UNIX_MS);
	let windows = store
		.write(async |tx| {
			tx.record_usage_quota_snapshot(&five_hour).await?;
			tx.record_usage_quota_snapshot(&weekly).await?;
			tx.record_usage_quota_answer(
				five_hour.snapshot_id,
				NOW_UNIX_MS + 60_000,
			)
			.await?;
			// An answer older than the one already recorded never moves it
			// backwards.
			tx.record_usage_quota_answer(
				five_hour.snapshot_id,
				NOW_UNIX_MS - 1,
			)
			.await?;
			tx.usage_quota_windows(Some(binding_id)).await
		})
		.await
		.unwrap();
	assert_eq!(
		windows,
		vec![
			UsageQuotaSnapshotRecord {
				answered_at_unix_ms: NOW_UNIX_MS + 60_000,
				..five_hour
			},
			weekly
		]
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
