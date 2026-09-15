//! Usage history: the retention tiers ADR-0045 sets for Jet-observed
//! consumption, the sweep that keeps the store inside them, and the Query
//! that answers a time series from whichever tier still holds it.
//!
//! Raw observations and changed quota snapshots stay ninety days, hourly
//! aggregates one year, and daily aggregates after that. Aggregates are
//! recounted from the raw rows of every hour a write touched, so a
//! measurement replaced after it was first counted is counted as
//! replaced; sweeping a tier removes rows the next tier already carries
//! and changes no total it holds. History covers Jet-observed activity
//! alone (ADR-0023): a Provider's quota windows are readings, not a
//! series.

use crate::{
	AccountBindingId, Core, CoreError, EventSequence, PlaneId, QueryResult,
	RecoveryMode,
	security::SecurityClass,
	system_time, unix_ms,
	usage::{ModelId, UsageTokens},
};
use jet_store::{
	UsageHistoryPointRecord, UsageOldestRecord, UsageResolutionRecord,
};
use std::time::SystemTime;

const HOUR: UsageResolutionRecord = UsageResolutionRecord::Hour;
const DAY: UsageResolutionRecord = UsageResolutionRecord::Day;

/// How long a raw observation, and a quota snapshot that is not its
/// window's freshest, is kept (ADR-0045).
const RAW_RETENTION_MS: i64 = 90 * 24 * 60 * 60 * 1000;

/// How long an hourly aggregate is kept (ADR-0045).
const HOURLY_RETENTION_MS: i64 = 365 * 24 * 60 * 60 * 1000;

/// What one Usage history Query covers. Aggregates are kept per Account
/// binding and Model, so a Conversation or Run has no series of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageHistorySelection {
	/// Every Account binding on this Plane.
	Plane,
	/// One Plane-local Account binding.
	Binding(AccountBindingId),
}

/// The width of the buckets a history is answered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageResolution {
	/// One hour per point, held for one year.
	Hour,
	/// One UTC day per point, held indefinitely.
	Day,
}

/// The half-open span of time one history Query asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageHistoryRange {
	/// The first instant covered.
	pub from: SystemTime,
	/// The first instant not covered.
	pub until: SystemTime,
}

/// One bucket of deduplicated consumption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsagePoint {
	/// When the bucket starts.
	pub start: SystemTime,
	/// The counts.
	pub tokens: UsageTokens,
	/// How many deduplicated measurements contributed.
	pub measurements: u64,
	/// How many of them Jet estimated rather than measured.
	pub estimated: u64,
	/// How many of them could still change when they were counted.
	pub interim: u64,
}

/// The series of one Model, in time order, with empty buckets left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSeries {
	/// The Model, absent where the Harness named none.
	pub model: Option<ModelId>,
	/// The buckets that hold anything.
	pub points: Vec<UsagePoint>,
}

/// What one Plane holds of Usage history for the selected scope, fenced
/// by the journal position it was read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageHistory {
	/// Newest Event sequence visible when the history was read.
	pub cursor: EventSequence,
	/// The Plane every point was observed on. A series covers this Plane
	/// alone (ADR-0016).
	pub plane_id: PlaneId,
	/// The resolution the history was answered at, which is the requested
	/// one unless the range reaches past the tier that holds it.
	pub resolution: UsageResolution,
	/// One series per Model.
	pub series: Vec<UsageSeries>,
}

/// What one sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageHistorySweep {
	/// Hours recounted from their raw rows.
	pub recounted_hours: u64,
	/// Raw observations removed past the raw tier.
	pub observations: u64,
	/// Quota snapshots removed past the raw tier.
	pub snapshots: u64,
	/// Hourly aggregates removed past the hourly tier.
	pub hours: u64,
}

impl UsageHistorySweep {
	/// Whether the sweep changed anything.
	#[must_use]
	pub fn is_empty(&self) -> bool {
		*self == Self::default()
	}
}

impl Core {
	/// Recounts the aggregates of every hour written since the last sweep,
	/// then removes what has left its tier: raw observations and
	/// superseded quota snapshots past ninety days, and hourly aggregates
	/// past a year. Removal follows the recount, so nothing leaves a tier
	/// before the next one carries it. Nothing runs while the Plane is in
	/// Recovery mode or cannot vouch for its Security audit (ADR-0105).
	///
	/// # Errors
	///
	/// Returns a store category [`CoreError`] when the store cannot be
	/// written.
	pub async fn sweep_usage_history(
		&self,
	) -> Result<UsageHistorySweep, CoreError> {
		if self.recovery_mode() != RecoveryMode::Serving
			|| self
				.security
				.read()
				.await
				.admit(SecurityClass::Guarded)
				.is_err()
		{
			return Ok(UsageHistorySweep::default());
		}
		let now = self.now_unix_ms();
		let raw_floor = now.saturating_sub(RAW_RETENTION_MS);
		let hourly_floor = hourly_floor(now);
		self.store
			.write(async |tx| {
				let recounted_hours = tx.rebuild_usage_aggregates().await?;
				let observations =
					tx.sweep_usage_observations_before(raw_floor).await?;
				let snapshots =
					tx.sweep_usage_quota_snapshots_before(raw_floor).await?;
				let hours = tx.sweep_usage_hours_before(hourly_floor).await?;
				Ok(UsageHistorySweep {
					recounted_hours,
					observations,
					snapshots,
					hours,
				})
			})
			.await
	}
}

/// When the next row leaves its tier, if any row will. Each deadline is
/// the first instant the sweep would actually remove the row, so a wake
/// at it is never idle.
pub(crate) async fn next_deadline(
	tx: &mut jet_store::ReadTransaction,
) -> Result<Option<i64>, CoreError> {
	Ok(deadline(tx.oldest_usage_rows().await?))
}

fn deadline(oldest: UsageOldestRecord) -> Option<i64> {
	let UsageOldestRecord {
		observation_unix_ms,
		hour_unix_ms,
		snapshot_unix_ms,
	} = oldest;
	// Raw rows leave whole hours at a time: an hour is gone once the raw
	// floor has passed its end.
	let observation = observation_unix_ms.map(|at| {
		(HOUR.start_of(at) + HOUR.span_ms()).saturating_add(RAW_RETENTION_MS)
	});
	// Hours leave whole days at a time, for the same reason.
	let hour = hour_unix_ms.map(|at| {
		(DAY.start_of(at) + DAY.span_ms()).saturating_add(HOURLY_RETENTION_MS)
	});
	let snapshot =
		snapshot_unix_ms.map(|at| at.saturating_add(RAW_RETENTION_MS + 1));
	[observation, hour, snapshot].into_iter().flatten().min()
}

/// The first hour the hourly tier still holds at `now`: whole days, so a
/// day is never summed from part of its hours.
fn hourly_floor(now_unix_ms: i64) -> i64 {
	DAY.start_of(now_unix_ms.saturating_sub(HOURLY_RETENTION_MS))
}

/// Reads the selected history with the journal position that fences it,
/// from the tier that still holds the whole range.
///
/// # Errors
///
/// Returns an `invalid_input` [`CoreError`] when the range ends before it
/// starts, and a store category one when the history cannot be read.
pub(crate) async fn history(
	core: &Core,
	selection: UsageHistorySelection,
	range: UsageHistoryRange,
	resolution: UsageResolution,
) -> Result<QueryResult, CoreError> {
	let from = unix_ms(range.from);
	let until = unix_ms(range.until);
	if until < from {
		return Err(CoreError::invalid_input(
			"usage.range_inverted",
			"the history range ends before it starts",
		));
	}
	// A range that reaches before the hourly tier is answered in days,
	// which every day since the Plane began still has.
	let answered = match resolution {
		UsageResolution::Hour if from < hourly_floor(core.now_unix_ms()) => {
			UsageResolution::Day
		}
		UsageResolution::Hour => UsageResolution::Hour,
		UsageResolution::Day => UsageResolution::Day,
	};
	let record = match answered {
		UsageResolution::Hour => HOUR,
		UsageResolution::Day => DAY,
	};
	// The bucket `from` falls in is part of the range asked about.
	let from = record.start_of(from);
	let binding = match selection {
		UsageHistorySelection::Plane => None,
		UsageHistorySelection::Binding(binding) => Some(binding.0),
	};
	core.store
		.read(async |tx| {
			// ASVS 2.3.3: the points and the position that fence them come
			// from one SQLite snapshot.
			let cursor = EventSequence(tx.event_cursor().await?);
			let plane_id = PlaneId(tx.plane().await?.plane_id);
			let points = tx.usage_history(binding, record, from, until).await?;
			Ok(QueryResult::UsageHistory(Box::new(UsageHistory {
				cursor,
				plane_id,
				resolution: answered,
				series: series(points),
			})))
		})
		.await
}

/// The store's per-Model, time-ordered points folded into one series per
/// Model.
fn series(points: Vec<UsageHistoryPointRecord>) -> Vec<UsageSeries> {
	let mut series: Vec<UsageSeries> = Vec::new();
	for point in points {
		let model = point.model.map(ModelId);
		let entry = match series.last_mut() {
			Some(last) if last.model == model => last,
			Some(_) | None => {
				series.push(UsageSeries {
					model,
					points: Vec::new(),
				});
				series.last_mut().expect("just pushed")
			}
		};
		entry.points.push(UsagePoint {
			start: system_time(point.bucket_start_unix_ms),
			tokens: UsageTokens {
				input: point.tokens.input,
				cached_input: point.tokens.cached_input,
				output: point.tokens.output,
				reasoning: point.tokens.reasoning,
			},
			measurements: point.measurements,
			estimated: point.estimated,
			interim: point.interim,
		});
	}
	series
}

#[cfg(test)]
mod tests {
	use std::time::{Duration, SystemTime};

	use pretty_assertions::assert_eq;

	use super::{
		DAY, HOUR, UsageHistory, UsageHistoryRange, UsageHistorySelection,
		UsageHistorySweep, UsagePoint, UsageResolution, UsageSeries, deadline,
	};
	use crate::test_support::actor;
	use crate::usage::tests::{
		bind, observed, record, run, start, tokens, window,
	};
	use crate::usage::{ObservedUsage, UsageMeasurement, UsageReport};
	use crate::{
		Core, ModelId, Query, QueryResult, UsageSelection, UsageTokens,
		system_time,
	};
	use jet_store::UsageOldestRecord;

	const AN_HOUR: Duration = Duration::from_secs(60 * 60);
	const A_DAY: Duration = Duration::from_secs(24 * 60 * 60);

	async fn history(
		core: &Core,
		range: UsageHistoryRange,
		resolution: UsageResolution,
	) -> UsageHistory {
		let QueryResult::UsageHistory(history) = core
			.query(
				&actor(),
				Query::UsageHistory {
					selection: UsageHistorySelection::Plane,
					range,
					resolution,
				},
			)
			.await
			.unwrap()
		else {
			panic!("Usage history")
		};
		*history
	}

	/// Everything since `origin`, at `resolution`.
	async fn since(
		core: &Core,
		origin: SystemTime,
		resolution: UsageResolution,
	) -> UsageHistory {
		history(
			core,
			UsageHistoryRange {
				from: origin - AN_HOUR,
				until: now(core) + AN_HOUR,
			},
			resolution,
		)
		.await
	}

	/// The Run's cumulative total, as a Craft that restates one reports it.
	fn cumulative(tokens: UsageTokens) -> UsageReport {
		let UsageReport::Observed(turn) = observed("run", tokens) else {
			panic!("an observed report")
		};
		UsageReport::Observed(ObservedUsage {
			measurement: UsageMeasurement::Run {
				native_usage_id: None,
			},
			..turn
		})
	}

	fn now(core: &Core) -> SystemTime {
		system_time(core.now_unix_ms())
	}

	fn hour_start(time: SystemTime) -> SystemTime {
		system_time(HOUR.start_of(crate::unix_ms(time)))
	}

	fn day_start(time: SystemTime) -> SystemTime {
		system_time(DAY.start_of(crate::unix_ms(time)))
	}

	/// The history `read` would be if it held `series` at `resolution`.
	fn expected(
		read: &UsageHistory,
		resolution: UsageResolution,
		series: Vec<UsageSeries>,
	) -> UsageHistory {
		UsageHistory {
			cursor: read.cursor,
			plane_id: read.plane_id,
			resolution,
			series,
		}
	}

	fn point(start: SystemTime, tokens: UsageTokens) -> UsagePoint {
		UsagePoint {
			start,
			tokens,
			measurements: 1,
			estimated: 0,
			interim: 0,
		}
	}

	fn opus(points: Vec<UsagePoint>) -> Vec<UsageSeries> {
		vec![UsageSeries {
			model: Some(ModelId("claude-opus-5".into())),
			points,
		}]
	}

	/// Aggregates account for every deduplicated measurement, including one
	/// replaced after it was first counted: a Run's cumulative total
	/// reported an hour after its turns leaves the turns' hour empty and
	/// counts the total in its own (ADR-0023, ADR-0045).
	#[tokio::test]
	async fn aggregates_count_a_measurement_as_replaced() {
		let dir = tempfile::tempdir().unwrap();
		let (core, clock) = start(&dir).await;
		let run = run(&core).await;
		let first_hour = now(&core);
		record(&core, &run, None, observed("turn-1", tokens(10, 5))).await;
		core.sweep_usage_history().await.unwrap();
		let before = since(&core, first_hour, UsageResolution::Hour).await;
		clock.advance(AN_HOUR);
		record(&core, &run, None, cumulative(tokens(45, 25))).await;
		let sweep = core.sweep_usage_history().await.unwrap();
		let after = since(&core, first_hour, UsageResolution::Hour).await;
		assert_eq!(
			(before.clone(), sweep, after.clone()),
			(
				expected(
					&before,
					UsageResolution::Hour,
					opus(vec![point(hour_start(first_hour), tokens(10, 5))])
				),
				UsageHistorySweep {
					recounted_hours: 2,
					..UsageHistorySweep::default()
				},
				expected(
					&after,
					UsageResolution::Hour,
					opus(vec![point(hour_start(now(&core)), tokens(45, 25))])
				)
			)
		);
	}

	/// A range that reaches past the hourly tier is answered in days, and
	/// says so, rather than in hours the Plane no longer holds.
	#[tokio::test]
	async fn a_range_past_the_hourly_tier_is_answered_in_days() {
		let dir = tempfile::tempdir().unwrap();
		let (core, _clock) = start(&dir).await;
		let run = run(&core).await;
		record(&core, &run, None, observed("turn-1", tokens(10, 5))).await;
		core.sweep_usage_history().await.unwrap();
		// A range starting inside the hour still covers that hour.
		let recent = history(
			&core,
			UsageHistoryRange {
				from: now(&core) - Duration::from_secs(1),
				until: now(&core) + AN_HOUR,
			},
			UsageResolution::Hour,
		)
		.await;
		let reaching = history(
			&core,
			UsageHistoryRange {
				from: now(&core) - 400 * A_DAY,
				until: now(&core) + AN_HOUR,
			},
			UsageResolution::Hour,
		)
		.await;
		assert_eq!(
			(recent.clone(), reaching.clone()),
			(
				expected(
					&recent,
					UsageResolution::Hour,
					opus(vec![point(hour_start(now(&core)), tokens(10, 5))])
				),
				expected(
					&reaching,
					UsageResolution::Day,
					opus(vec![point(day_start(now(&core)), tokens(10, 5))])
				)
			)
		);
	}

	/// Sweeping raw observations past ninety days and hourly aggregates
	/// past a year changes no total the surviving aggregates carry. The
	/// current-totals Query loses the raw rows it summed, and a window
	/// keeps its freshest snapshot whatever its age (ADR-0045).
	#[tokio::test]
	async fn sweeping_past_each_tier_changes_no_surviving_total() {
		let dir = tempfile::tempdir().unwrap();
		let (core, clock) = start(&dir).await;
		let run = run(&core).await;
		let binding = bind(&core).await;
		let origin = now(&core);
		record(
			&core,
			&run,
			Some(binding),
			observed("turn-1", tokens(10, 5)),
		)
		.await;
		record(&core, &run, Some(binding), window(2_500)).await;
		clock.advance(Duration::from_secs(60));
		record(&core, &run, Some(binding), window(4_000)).await;
		core.sweep_usage_history().await.unwrap();
		let hours = since(&core, origin, UsageResolution::Hour).await.series;
		let days = since(&core, origin, UsageResolution::Day).await.series;
		clock.advance(91 * A_DAY);
		let raw = core.sweep_usage_history().await.unwrap();
		let hours_after_raw =
			since(&core, origin, UsageResolution::Hour).await.series;
		let QueryResult::Usage(current) = core
			.query(
				&actor(),
				Query::Usage {
					selection: UsageSelection::Binding(binding),
				},
			)
			.await
			.unwrap()
		else {
			panic!("Usage")
		};
		clock.advance(365 * A_DAY);
		let hourly = core.sweep_usage_history().await.unwrap();
		let days_after_hourly =
			since(&core, origin, UsageResolution::Day).await.series;
		assert_eq!(
			(
				raw,
				hours_after_raw,
				current.consumption.measurements,
				current
					.quota_windows
					.iter()
					.map(|window| window.measure.used)
					.collect::<Vec<_>>(),
				hourly,
				days_after_hourly,
			),
			(
				UsageHistorySweep {
					observations: 1,
					snapshots: 1,
					..UsageHistorySweep::default()
				},
				hours,
				0,
				vec![4_000],
				UsageHistorySweep {
					hours: 1,
					..UsageHistorySweep::default()
				},
				days
			)
		);
	}

	/// A deadline is the first instant the sweep would remove the oldest
	/// row of its tier, so a wake at it is never idle: raw rows and hours
	/// leave whole hours and whole days at a time.
	#[test]
	fn a_deadline_is_when_the_sweep_would_first_remove_the_row() {
		let at = 1_700_000_000_000 + 40 * 60 * 1000;
		let raw_only = deadline(UsageOldestRecord {
			observation_unix_ms: Some(at),
			hour_unix_ms: None,
			snapshot_unix_ms: None,
		});
		let hours_only = deadline(UsageOldestRecord {
			observation_unix_ms: None,
			hour_unix_ms: Some(HOUR.start_of(at)),
			snapshot_unix_ms: None,
		});
		let snapshots_only = deadline(UsageOldestRecord {
			observation_unix_ms: None,
			hour_unix_ms: None,
			snapshot_unix_ms: Some(at),
		});
		assert_eq!(
			(
				raw_only,
				hours_only,
				snapshots_only,
				deadline(UsageOldestRecord::default())
			),
			(
				Some(
					HOUR.start_of(at)
						+ HOUR.span_ms() + super::RAW_RETENTION_MS
				),
				Some(
					DAY.start_of(at)
						+ DAY.span_ms() + super::HOURLY_RETENTION_MS
				),
				Some(at + super::RAW_RETENTION_MS + 1),
				None
			)
		);
	}
}
