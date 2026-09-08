use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{
	NewUsageObservation, UsageEstimationRecord, UsageFinalityRecord,
	UsageScopeRecord, UsageSelectionRecord, UsageTokensRecord,
	UsageTotalRecord,
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
