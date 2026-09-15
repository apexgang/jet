//! Downsampled Jet-observed consumption: the hourly and daily aggregates
//! ADR-0045 keeps after the raw rows are gone, and the sweeps that keep
//! each tier inside its retention.
//!
//! An aggregate is never added to in place. Every write to a raw row
//! marks the hours it touched, and the rebuild recounts those hours from
//! the raw rows under the same deduplication `usage_totals` reads with,
//! so a measurement replaced after it was first counted is counted as
//! replaced. Raw rows leave the store whole hours at a time, so a marked
//! hour always still has every row it had. The tiers themselves, and when
//! each sweep is due, are the core's.

use crate::{
	ReadTransaction, StoreError, UsageTokensRecord, WriteTransaction,
	usage::read_count,
};
use uuid::Uuid;

/// One millisecond hour, which is what an hourly bucket spans.
const HOUR_MS: i64 = 60 * 60 * 1000;

/// One millisecond day, which is what a daily bucket spans.
const DAY_MS: i64 = 24 * HOUR_MS;

/// The width of the buckets one history read answers in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageResolutionRecord {
	/// One hour per point.
	Hour,
	/// One UTC day per point.
	Day,
}

/// One bucket of deduplicated consumption for one Model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageHistoryPointRecord {
	/// When the bucket starts.
	pub bucket_start_unix_ms: i64,
	/// The Model these counts belong to, absent when none was reported.
	pub model: Option<String>,
	/// The counts.
	pub tokens: UsageTokensRecord,
	/// How many measurements contributed.
	pub measurements: u64,
	/// How many of them Jet estimated.
	pub estimated: u64,
	/// How many of them could still change when they were counted.
	pub interim: u64,
}

/// The oldest row of each tier, which is what the next sweep's deadline
/// follows from. A tier with no rows is absent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageOldestRecord {
	/// When the oldest raw observation was observed.
	pub observation_unix_ms: Option<i64>,
	/// When the oldest hourly aggregate's hour starts.
	pub hour_unix_ms: Option<i64>,
	/// When the oldest quota snapshot that is not the freshest of its
	/// window was observed.
	pub snapshot_unix_ms: Option<i64>,
}

impl UsageResolutionRecord {
	/// The durable spelling.
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Hour => "hour",
			Self::Day => "day",
		}
	}

	/// How long one bucket lasts.
	#[must_use]
	pub fn span_ms(self) -> i64 {
		match self {
			Self::Hour => HOUR_MS,
			Self::Day => DAY_MS,
		}
	}

	/// The start of the bucket `unix_ms` falls in. Buckets are aligned to
	/// the epoch, so a day is a UTC day.
	#[must_use]
	pub fn start_of(self, unix_ms: i64) -> i64 {
		unix_ms.div_euclid(self.span_ms()) * self.span_ms()
	}
}

impl ReadTransaction {
	/// The aggregated consumption per Model for every bucket of
	/// `resolution` starting at or after `from_unix_ms` and before
	/// `until_unix_ms`, for one binding or for the whole Plane, ordered by
	/// Model and then by time.
	///
	/// # Errors
	///
	/// Returns a store error when the query fails, and an integrity error
	/// when a stored count does not fit the record.
	pub async fn usage_history(
		&mut self,
		binding_id: Option<Uuid>,
		resolution: UsageResolutionRecord,
		from_unix_ms: i64,
		until_unix_ms: i64,
	) -> Result<Vec<UsageHistoryPointRecord>, StoreError> {
		let binding_id = binding_id.map(|id| id.to_string());
		let resolution = resolution.as_str();
		// ASVS 1.2.4: SQL structure is static; every dynamic value in this
		// module is passed through SQLite parameters.
		let rows = sqlx::query!(
			r#"SELECT bucket_start_unix_ms, model,
				CAST(SUM(input_tokens) AS INTEGER) AS "input!: i64",
				CAST(SUM(cached_input_tokens) AS INTEGER) AS "cached!: i64",
				CAST(SUM(output_tokens) AS INTEGER) AS "output!: i64",
				CAST(SUM(reasoning_tokens) AS INTEGER) AS "reasoning!: i64",
				CAST(SUM(measurements) AS INTEGER) AS "measurements!: i64",
				CAST(SUM(estimated) AS INTEGER) AS "estimated!: i64",
				CAST(SUM(interim) AS INTEGER) AS "interim!: i64"
			 FROM usage_aggregates
			 WHERE resolution = ?1
			   AND (?2 IS NULL OR binding_id = ?2)
			   AND bucket_start_unix_ms >= ?3
			   AND bucket_start_unix_ms < ?4
			 GROUP BY model, bucket_start_unix_ms
			 ORDER BY model, bucket_start_unix_ms"#,
			resolution,
			binding_id,
			from_unix_ms,
			until_unix_ms
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter()
			.map(|row| {
				Ok(UsageHistoryPointRecord {
					bucket_start_unix_ms: row.bucket_start_unix_ms,
					model: row.model,
					tokens: UsageTokensRecord {
						input: read_count("input_tokens", row.input)?,
						cached_input: read_count(
							"cached_input_tokens",
							row.cached,
						)?,
						output: read_count("output_tokens", row.output)?,
						reasoning: read_count(
							"reasoning_tokens",
							row.reasoning,
						)?,
					},
					measurements: read_count("measurements", row.measurements)?,
					estimated: read_count("estimated", row.estimated)?,
					interim: read_count("interim", row.interim)?,
				})
			})
			.collect()
	}

	/// The oldest row of each tier: the oldest raw observation, the oldest
	/// hourly aggregate, and the oldest quota snapshot that is not the
	/// freshest of its window.
	///
	/// # Errors
	///
	/// Returns a store error when the query fails.
	pub async fn oldest_usage_rows(
		&mut self,
	) -> Result<UsageOldestRecord, StoreError> {
		let row = sqlx::query!(
			r#"SELECT
				(SELECT MIN(observed_at_unix_ms) FROM usage_observations)
					AS "observation: i64",
				(SELECT MIN(bucket_start_unix_ms) FROM usage_aggregates
				  WHERE resolution = 'hour') AS "hour: i64",
				(SELECT MIN(s.observed_at_unix_ms) FROM usage_quota_snapshots s
				  WHERE s.rowid <> (
					SELECT t.rowid FROM usage_quota_snapshots t
					 WHERE t.binding_id = s.binding_id
					   AND t.window_id = s.window_id
					   AND t.model IS s.model
					 ORDER BY t.observed_at_unix_ms DESC, t.rowid DESC
					 LIMIT 1)) AS "snapshot: i64""#
		)
		.fetch_one(self.connection())
		.await?;
		Ok(UsageOldestRecord {
			observation_unix_ms: row.observation,
			hour_unix_ms: row.hour,
			snapshot_unix_ms: row.snapshot,
		})
	}
}

impl WriteTransaction {
	/// Recount the hourly aggregates of every hour a raw write touched
	/// since the last rebuild, and the daily aggregates of the days those
	/// hours fall in, from the raw rows. Reports how many hours were
	/// recounted.
	///
	/// # Errors
	///
	/// Returns a store error when a statement fails.
	pub async fn rebuild_usage_aggregates(
		&mut self,
	) -> Result<u64, StoreError> {
		let dirty = sqlx::query_scalar!(
			r#"SELECT COUNT(*) AS "count!: i64" FROM usage_dirty_hours"#
		)
		.fetch_one(self.connection())
		.await?;
		if dirty == 0 {
			return Ok(0);
		}
		sqlx::query!(
			"DELETE FROM usage_aggregates
			  WHERE resolution = 'hour'
			    AND bucket_start_unix_ms IN
					(SELECT hour_start_unix_ms FROM usage_dirty_hours)"
		)
		.execute(self.connection())
		.await?;
		// The same counting rule as `usage_totals`: a Run whose Craft
		// restated a cumulative total contributes that total and not the
		// turns it already covers.
		sqlx::query!(
			"INSERT INTO usage_aggregates
				(resolution, bucket_start_unix_ms, binding_id, provider, model,
				 input_tokens, cached_input_tokens, output_tokens,
				 reasoning_tokens, measurements, estimated, interim)
			 SELECT 'hour', (o.observed_at_unix_ms / ?1) * ?1, o.binding_id,
				o.provider, o.model,
				SUM(o.input_tokens), SUM(o.cached_input_tokens),
				SUM(o.output_tokens), SUM(o.reasoning_tokens), COUNT(*),
				SUM(o.estimation = 'estimated'), SUM(o.finality = 'interim')
			 FROM usage_observations o
			 WHERE (o.observed_at_unix_ms / ?1) * ?1 IN
					(SELECT hour_start_unix_ms FROM usage_dirty_hours)
			   AND (o.scope = 'run'
			     OR NOT EXISTS (
				     SELECT 1 FROM usage_observations r
				      WHERE r.run_id = o.run_id AND r.scope = 'run'))
			 GROUP BY (o.observed_at_unix_ms / ?1) * ?1, o.binding_id,
				o.provider, o.model",
			HOUR_MS
		)
		.execute(self.connection())
		.await?;
		sqlx::query!(
			"DELETE FROM usage_aggregates
			  WHERE resolution = 'day'
			    AND bucket_start_unix_ms IN
					(SELECT (hour_start_unix_ms / ?1) * ?1
					   FROM usage_dirty_hours)",
			DAY_MS
		)
		.execute(self.connection())
		.await?;
		sqlx::query!(
			"INSERT INTO usage_aggregates
				(resolution, bucket_start_unix_ms, binding_id, provider, model,
				 input_tokens, cached_input_tokens, output_tokens,
				 reasoning_tokens, measurements, estimated, interim)
			 SELECT 'day', (h.bucket_start_unix_ms / ?1) * ?1, h.binding_id,
				h.provider, h.model,
				SUM(h.input_tokens), SUM(h.cached_input_tokens),
				SUM(h.output_tokens), SUM(h.reasoning_tokens),
				SUM(h.measurements), SUM(h.estimated), SUM(h.interim)
			 FROM usage_aggregates h
			 WHERE h.resolution = 'hour'
			   AND (h.bucket_start_unix_ms / ?1) * ?1 IN
					(SELECT (hour_start_unix_ms / ?1) * ?1
					   FROM usage_dirty_hours)
			 GROUP BY (h.bucket_start_unix_ms / ?1) * ?1, h.binding_id,
				h.provider, h.model",
			DAY_MS
		)
		.execute(self.connection())
		.await?;
		sqlx::query!("DELETE FROM usage_dirty_hours")
			.execute(self.connection())
			.await?;
		read_count("dirty hours", dirty)
	}

	/// Delete every raw observation of an hour that ends at or before
	/// `cutoff_unix_ms`. Whole hours leave together, so an hour the next
	/// write marks always still has every row it had. The hours removed
	/// are not marked: the aggregates already carry them, and that is what
	/// they now stand for.
	///
	/// # Errors
	///
	/// Returns a store error when the delete fails.
	pub async fn sweep_usage_observations_before(
		&mut self,
		cutoff_unix_ms: i64,
	) -> Result<u64, StoreError> {
		let cutoff = UsageResolutionRecord::Hour.start_of(cutoff_unix_ms);
		Ok(sqlx::query!(
			"DELETE FROM usage_observations WHERE observed_at_unix_ms < ?1",
			cutoff
		)
		.execute(self.connection())
		.await?
		.rows_affected())
	}

	/// Delete every hourly aggregate whose hour starts before
	/// `cutoff_unix_ms`. The daily aggregates covering them stay.
	///
	/// # Errors
	///
	/// Returns a store error when the delete fails.
	pub async fn sweep_usage_hours_before(
		&mut self,
		cutoff_unix_ms: i64,
	) -> Result<u64, StoreError> {
		Ok(sqlx::query!(
			"DELETE FROM usage_aggregates
			  WHERE resolution = 'hour' AND bucket_start_unix_ms < ?1",
			cutoff_unix_ms
		)
		.execute(self.connection())
		.await?
		.rows_affected())
	}

	/// Mark the hour of every raw row of one Run as needing a recount, plus
	/// `also_unix_ms`, the hour a row is about to be written into.
	pub(crate) async fn mark_run_usage_hours_dirty(
		&mut self,
		run_id: &str,
		also_unix_ms: i64,
	) -> Result<(), StoreError> {
		let also = UsageResolutionRecord::Hour.start_of(also_unix_ms);
		sqlx::query!(
			"INSERT OR IGNORE INTO usage_dirty_hours (hour_start_unix_ms)
			 SELECT (observed_at_unix_ms / ?2) * ?2 FROM usage_observations
			  WHERE run_id = ?1
			 UNION SELECT ?3",
			run_id,
			HOUR_MS,
			also
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
}

/// Mark the hour of every raw row of one Conversation as needing a
/// recount, before those rows are removed.
pub(crate) async fn mark_conversation_usage_hours_dirty(
	connection: &mut sqlx::SqliteConnection,
	conversation_id: &str,
) -> Result<(), StoreError> {
	sqlx::query!(
		"INSERT OR IGNORE INTO usage_dirty_hours (hour_start_unix_ms)
		 SELECT (observed_at_unix_ms / ?2) * ?2 FROM usage_observations
		  WHERE conversation_id = ?1",
		conversation_id,
		HOUR_MS
	)
	.execute(connection)
	.await?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use super::{
		DAY_MS, HOUR_MS, UsageHistoryPointRecord, UsageOldestRecord,
		UsageResolutionRecord,
	};
	use crate::{
		ConversationOriginRecord, NewConversation, NewRun, NewUsageObservation,
		RetentionPolicy, Store, StoreError, UsageEstimationRecord,
		UsageFinalityRecord, UsageScopeRecord, UsageTokensRecord,
		WorkingTreeRecord,
	};

	/// A moment forty minutes into an hour, so a bucket boundary is near
	/// enough to cross with a small step.
	const NOW_UNIX_MS: i64 = 1_700_000_000_000 + 40 * 60 * 1000;

	fn hour_start(unix_ms: i64) -> i64 {
		UsageResolutionRecord::Hour.start_of(unix_ms)
	}

	fn day_start(unix_ms: i64) -> i64 {
		UsageResolutionRecord::Day.start_of(unix_ms)
	}

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
		observed_at_unix_ms: i64,
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
			observed_at_unix_ms,
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

	fn point(
		bucket_start_unix_ms: i64,
		tokens: UsageTokensRecord,
		measurements: u64,
	) -> UsageHistoryPointRecord {
		UsageHistoryPointRecord {
			bucket_start_unix_ms,
			model: Some("claude-opus-5".into()),
			tokens,
			measurements,
			estimated: 0,
			interim: 0,
		}
	}

	async fn record(store: &Store, observation: NewUsageObservation) {
		store
			.write(async |tx| tx.record_usage_observation(&observation).await)
			.await
			.unwrap();
	}

	async fn rebuild(store: &Store) -> u64 {
		store
			.write(async |tx| tx.rebuild_usage_aggregates().await)
			.await
			.unwrap()
	}

	async fn history(
		store: &Store,
		resolution: UsageResolutionRecord,
	) -> Vec<UsageHistoryPointRecord> {
		store
			.read(async |tx| {
				tx.usage_history(None, resolution, i64::MIN, i64::MAX).await
			})
			.await
			.unwrap()
	}

	/// A measurement replaced after it was first counted is counted as
	/// replaced, in the hour and in the day (ADR-0045).
	#[tokio::test]
	async fn a_replaced_measurement_is_recounted_in_both_tiers() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let (conversation_id, run_id) = conversation(&store).await;
		record(
			&store,
			observation(
				conversation_id,
				run_id,
				"turn-1",
				tokens(10, 5),
				NOW_UNIX_MS,
			),
		)
		.await;
		let first = rebuild(&store).await;
		record(
			&store,
			observation(
				conversation_id,
				run_id,
				"turn-1",
				tokens(30, 20),
				NOW_UNIX_MS + 1,
			),
		)
		.await;
		let second = rebuild(&store).await;
		assert_eq!(
			(
				first,
				second,
				history(&store, UsageResolutionRecord::Hour).await,
				history(&store, UsageResolutionRecord::Day).await
			),
			(
				1,
				1,
				vec![point(hour_start(NOW_UNIX_MS), tokens(30, 20), 1)],
				vec![point(day_start(NOW_UNIX_MS), tokens(30, 20), 1)]
			)
		);
	}

	/// A cumulative Run total reported in a later hour takes the turns of
	/// an earlier hour out of that hour's count: they are covered, not added.
	#[tokio::test]
	async fn a_cumulative_total_recounts_the_hours_its_turns_were_in() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let (conversation_id, run_id) = conversation(&store).await;
		record(
			&store,
			observation(
				conversation_id,
				run_id,
				"turn-1",
				tokens(10, 5),
				NOW_UNIX_MS,
			),
		)
		.await;
		rebuild(&store).await;
		let later = NOW_UNIX_MS + HOUR_MS;
		let mut cumulative =
			observation(conversation_id, run_id, "run", tokens(45, 25), later);
		cumulative.scope = UsageScopeRecord::Run;
		record(&store, cumulative).await;
		rebuild(&store).await;
		assert_eq!(
			history(&store, UsageResolutionRecord::Hour).await,
			vec![point(hour_start(later), tokens(45, 25), 1)]
		);
	}

	/// A read answers the requested buckets in the requested range, per
	/// Model, and a day is the sum of its hours.
	#[tokio::test]
	async fn a_read_answers_the_range_at_the_requested_resolution() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let (conversation_id, run_id) = conversation(&store).await;
		let next_hour = NOW_UNIX_MS + HOUR_MS;
		let mut haiku = observation(
			conversation_id,
			run_id,
			"turn-3",
			tokens(1, 1),
			next_hour,
		);
		haiku.model = Some("claude-haiku-4-5".into());
		haiku.estimation = UsageEstimationRecord::Estimated;
		haiku.finality = UsageFinalityRecord::Interim;
		record(
			&store,
			observation(
				conversation_id,
				run_id,
				"turn-1",
				tokens(10, 5),
				NOW_UNIX_MS,
			),
		)
		.await;
		record(
			&store,
			observation(
				conversation_id,
				run_id,
				"turn-2",
				tokens(4, 1),
				next_hour,
			),
		)
		.await;
		record(&store, haiku).await;
		rebuild(&store).await;
		let (hours, days, bounded) = store
			.read(async |tx| {
				let hours = tx
					.usage_history(
						None,
						UsageResolutionRecord::Hour,
						i64::MIN,
						i64::MAX,
					)
					.await?;
				let days = tx
					.usage_history(
						None,
						UsageResolutionRecord::Day,
						i64::MIN,
						i64::MAX,
					)
					.await?;
				let bounded = tx
					.usage_history(
						None,
						UsageResolutionRecord::Hour,
						hour_start(next_hour),
						hour_start(next_hour) + HOUR_MS,
					)
					.await?;
				Ok::<_, StoreError>((hours, days, bounded))
			})
			.await
			.unwrap();
		let haiku_point = UsageHistoryPointRecord {
			bucket_start_unix_ms: hour_start(next_hour),
			model: Some("claude-haiku-4-5".into()),
			tokens: tokens(1, 1),
			measurements: 1,
			estimated: 1,
			interim: 1,
		};
		assert_eq!(
			(hours, days, bounded),
			(
				vec![
					haiku_point.clone(),
					point(hour_start(NOW_UNIX_MS), tokens(10, 5), 1),
					point(hour_start(next_hour), tokens(4, 1), 1),
				],
				vec![
					UsageHistoryPointRecord {
						bucket_start_unix_ms: day_start(next_hour),
						..haiku_point.clone()
					},
					point(day_start(NOW_UNIX_MS), tokens(14, 6), 2),
				],
				vec![
					haiku_point,
					point(hour_start(next_hour), tokens(4, 1), 1),
				]
			)
		);
	}

	/// Sweeping raw rows past their tier changes no hour the aggregates
	/// already carry, and rows leave whole hours at a time: a cutoff inside
	/// an hour leaves that hour's rows where they are.
	#[tokio::test]
	async fn sweeping_raw_rows_leaves_the_hours_they_were_counted_into() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let (conversation_id, run_id) = conversation(&store).await;
		record(
			&store,
			observation(
				conversation_id,
				run_id,
				"turn-1",
				tokens(10, 5),
				NOW_UNIX_MS,
			),
		)
		.await;
		record(
			&store,
			observation(
				conversation_id,
				run_id,
				"turn-2",
				tokens(4, 1),
				NOW_UNIX_MS + HOUR_MS,
			),
		)
		.await;
		rebuild(&store).await;
		let before = history(&store, UsageResolutionRecord::Hour).await;
		let (inside, past, oldest) = store
			.write(async |tx| {
				let inside = tx
					.sweep_usage_observations_before(NOW_UNIX_MS + HOUR_MS + 1)
					.await?;
				let past = tx
					.sweep_usage_observations_before(NOW_UNIX_MS + 2 * HOUR_MS)
					.await?;
				let oldest = tx.oldest_usage_rows().await?;
				Ok::<_, StoreError>((inside, past, oldest))
			})
			.await
			.unwrap();
		assert_eq!(
			(
				inside,
				past,
				oldest,
				history(&store, UsageResolutionRecord::Hour).await
			),
			(
				1,
				1,
				UsageOldestRecord {
					observation_unix_ms: None,
					hour_unix_ms: Some(hour_start(NOW_UNIX_MS)),
					snapshot_unix_ms: None,
				},
				before
			)
		);
	}

	/// A marked hour is recounted whenever the rebuild runs, however long
	/// after the write: the raw rows are what the aggregates are built from,
	/// and they are still there.
	#[tokio::test]
	async fn a_marked_hour_is_recounted_however_late_the_rebuild_runs() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let (conversation_id, run_id) = conversation(&store).await;
		let old = NOW_UNIX_MS - 100 * DAY_MS;
		record(
			&store,
			observation(conversation_id, run_id, "turn-1", tokens(10, 5), old),
		)
		.await;
		let recounted = rebuild(&store).await;
		assert_eq!(
			(recounted, history(&store, UsageResolutionRecord::Day).await),
			(1, vec![point(day_start(old), tokens(10, 5), 1)])
		);
	}

	/// Sweeping hourly aggregates past their tier changes no day the daily
	/// aggregates already carry.
	#[tokio::test]
	async fn sweeping_hours_leaves_the_days_they_were_summed_into() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let (conversation_id, run_id) = conversation(&store).await;
		record(
			&store,
			observation(
				conversation_id,
				run_id,
				"turn-1",
				tokens(10, 5),
				NOW_UNIX_MS,
			),
		)
		.await;
		record(
			&store,
			observation(
				conversation_id,
				run_id,
				"turn-2",
				tokens(4, 1),
				NOW_UNIX_MS + HOUR_MS,
			),
		)
		.await;
		rebuild(&store).await;
		let before = history(&store, UsageResolutionRecord::Day).await;
		let swept = store
			.write(async |tx| {
				tx.sweep_usage_hours_before(NOW_UNIX_MS + DAY_MS).await
			})
			.await
			.unwrap();
		assert_eq!(
			(
				swept,
				history(&store, UsageResolutionRecord::Hour).await,
				history(&store, UsageResolutionRecord::Day).await
			),
			(2, vec![], before)
		);
	}

	/// The oldest row of each tier is reported, and an absent tier is
	/// absent.
	#[tokio::test]
	async fn the_oldest_rows_of_each_tier_are_reported() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let (conversation_id, run_id) = conversation(&store).await;
		let empty = store
			.read(async |tx| tx.oldest_usage_rows().await)
			.await
			.unwrap();
		record(
			&store,
			observation(
				conversation_id,
				run_id,
				"turn-1",
				tokens(10, 5),
				NOW_UNIX_MS,
			),
		)
		.await;
		rebuild(&store).await;
		let raw = store
			.read(async |tx| tx.oldest_usage_rows().await)
			.await
			.unwrap();
		assert_eq!(
			(empty, raw),
			(
				UsageOldestRecord::default(),
				UsageOldestRecord {
					observation_unix_ms: Some(NOW_UNIX_MS),
					hour_unix_ms: Some(hour_start(NOW_UNIX_MS)),
					snapshot_unix_ms: None,
				}
			)
		);
	}
}
