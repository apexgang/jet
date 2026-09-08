//! Normalized Usage rows and the reads that keep them countable
//! (ADR-0023, ADR-0045).
//!
//! Two kinds of measurement live here. Jet-observed consumption is what a
//! Harness reported it used, identified so that repeating it replaces the
//! row instead of adding to it. Provider-reported quota windows are kept as
//! the snapshot history their diagrams are drawn from, and reads select the
//! freshest snapshot of each window rather than summing windows together.

use uuid::Uuid;

use crate::records::{
	column_error, parse_bytes, parse_optional_uuid, parse_uuid,
};
use crate::{ReadTransaction, StoreError, WriteTransaction};

/// What one Jet-observed measurement covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageScopeRecord {
	/// One turn of the Run.
	Turn,
	/// The Run so far, as a cumulative total.
	Run,
}

/// Whether Jet measured the numbers or estimated them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageEstimationRecord {
	/// Reported by the Harness or Provider.
	Measured,
	/// Derived by Jet, and never presented as a Provider's own accounting.
	Estimated,
}

/// Whether the measurement can still change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageFinalityRecord {
	/// The work it covers had not finished when it was reported.
	Interim,
	/// The work it covers is over and the numbers no longer move.
	Final,
}

/// What one Provider-reported quota window covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaScopeRecord {
	/// The Provider account as a whole.
	ProviderAccount,
	/// One Model of that account.
	Model,
}

/// The unit a Provider stated one window in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaUnitRecord {
	/// Inference tokens.
	Tokens,
	/// Requests.
	Requests,
	/// Provider-defined credits.
	Credits,
	/// Hundredths of a percent of the window, out of 10,000.
	Share,
}

/// Whether a binding's Provider answered the last time Jet asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderReachRecord {
	/// The Provider answered.
	Reachable,
	/// The Provider did not answer, for the reason it gave.
	Unreachable {
		/// Bounded, non-secret text naming why.
		reason: String,
	},
}

/// The token counts one measurement carries. Cache creation counts as
/// input; what the Harness reported in its own vocabulary stays in the
/// journalled native event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageTokensRecord {
	/// Tokens sent, including any the Provider wrote to its cache.
	pub input: u64,
	/// Tokens served from the Provider's cache.
	pub cached_input: u64,
	/// Tokens generated.
	pub output: u64,
	/// Tokens spent on reasoning, where the Provider counts them apart.
	pub reasoning: u64,
}

/// One Jet-observed measurement as a Craft reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUsageObservation {
	/// Durable row identity.
	pub observation_id: Uuid,
	/// The Conversation the Run belongs to.
	pub conversation_id: Uuid,
	/// The Run the work was done by.
	pub run_id: Uuid,
	/// What makes this measurement one measurement within its Run.
	pub measurement: String,
	/// The Provider's or Harness's own identity for it, where there is one.
	pub native_usage_id: Option<String>,
	/// The Account binding the Run authenticated through, when it named one.
	pub binding_id: Option<Uuid>,
	/// The Provider that binding authenticates to.
	pub provider: Option<String>,
	/// The Model that did the work.
	pub model: Option<String>,
	/// What the numbers cover.
	pub scope: UsageScopeRecord,
	/// Whether they were measured or estimated.
	pub estimation: UsageEstimationRecord,
	/// Whether they can still change.
	pub finality: UsageFinalityRecord,
	/// The counts themselves.
	pub tokens: UsageTokensRecord,
	/// When the Plane observed them.
	pub observed_at_unix_ms: i64,
}

/// Deduplicated consumption for one Model, summed only over the rows that
/// may be added together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageTotalRecord {
	/// The Model these counts belong to, absent when none was reported.
	pub model: Option<String>,
	/// The counts.
	pub tokens: UsageTokensRecord,
	/// How many measurements contributed.
	pub measurements: u64,
	/// How many of them Jet estimated.
	pub estimated: u64,
	/// How many of them can still change.
	pub interim: u64,
	/// When the oldest contributing measurement was observed.
	pub first_observed_at_unix_ms: i64,
	/// When the newest contributing measurement was observed.
	pub last_observed_at_unix_ms: i64,
}

/// One Provider-reported quota window snapshot, as written and as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageQuotaSnapshotRecord {
	/// Durable row identity.
	pub snapshot_id: Uuid,
	/// The Plane-local Account binding the window was reported for.
	pub binding_id: Uuid,
	/// The Provider that reported it.
	pub provider: String,
	/// The Provider's own name for the window.
	pub window_id: String,
	/// What the window covers.
	pub scope: QuotaScopeRecord,
	/// The Model, when the window covers one.
	pub model: Option<String>,
	/// The Conversation the response was observed in, when there was one.
	pub conversation_id: Option<Uuid>,
	/// The Run the response was observed in, when there was one.
	pub run_id: Option<Uuid>,
	/// The unit the Provider stated the window in.
	pub unit: QuotaUnitRecord,
	/// How much of the window the Provider reported as consumed.
	pub used: u64,
	/// The stated limit, where the Provider gave one.
	pub limit_amount: Option<u64>,
	/// How long the window lasts, where the Provider gave it.
	pub window_seconds: Option<u64>,
	/// When the window refills, where the Provider gave it.
	pub resets_at_unix_ms: Option<i64>,
	/// Whether the numbers were measured or estimated.
	pub estimation: UsageEstimationRecord,
	/// Whether the window has closed.
	pub finality: UsageFinalityRecord,
	/// When the Plane observed the response.
	pub observed_at_unix_ms: i64,
	/// Digest of the reported content, so an unchanged response is a
	/// heartbeat rather than another row.
	pub digest: [u8; 32],
}

/// The newest snapshot of one window, reduced to what deduplication needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageQuotaHeartbeatRecord {
	/// Digest of the content that snapshot reported.
	pub digest: [u8; 32],
	/// When it was observed.
	pub observed_at_unix_ms: i64,
}

/// Whether one binding's Provider answered, and when Jet last found out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageProviderReachRecord {
	/// The binding.
	pub binding_id: Uuid,
	/// What the Plane last saw.
	pub reach: ProviderReachRecord,
	/// When it saw it.
	pub observed_at_unix_ms: i64,
}

/// Which rows one Usage read covers. An absent field selects every value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageSelectionRecord {
	/// One Conversation.
	pub conversation_id: Option<Uuid>,
	/// One Run.
	pub run_id: Option<Uuid>,
	/// One Account binding.
	pub binding_id: Option<Uuid>,
}

impl UsageScopeRecord {
	/// The durable spelling.
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Turn => "turn",
			Self::Run => "run",
		}
	}
}

impl UsageEstimationRecord {
	/// The durable spelling.
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Measured => "measured",
			Self::Estimated => "estimated",
		}
	}

	fn parse(text: &str) -> Option<Self> {
		[Self::Measured, Self::Estimated]
			.into_iter()
			.find(|estimation| estimation.as_str() == text)
	}
}

impl UsageFinalityRecord {
	/// The durable spelling.
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Interim => "interim",
			Self::Final => "final",
		}
	}

	fn parse(text: &str) -> Option<Self> {
		[Self::Interim, Self::Final]
			.into_iter()
			.find(|finality| finality.as_str() == text)
	}
}

impl QuotaScopeRecord {
	/// The durable spelling.
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::ProviderAccount => "provider_account",
			Self::Model => "model",
		}
	}

	fn parse(text: &str) -> Option<Self> {
		[Self::ProviderAccount, Self::Model]
			.into_iter()
			.find(|scope| scope.as_str() == text)
	}
}

impl QuotaUnitRecord {
	/// The durable spelling.
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Tokens => "tokens",
			Self::Requests => "requests",
			Self::Credits => "credits",
			Self::Share => "share",
		}
	}

	fn parse(text: &str) -> Option<Self> {
		[Self::Tokens, Self::Requests, Self::Credits, Self::Share]
			.into_iter()
			.find(|unit| unit.as_str() == text)
	}
}

impl ProviderReachRecord {
	/// The durable spelling of the state and the reason it carries.
	fn columns(&self) -> (&'static str, Option<&str>) {
		match self {
			Self::Reachable => ("reachable", None),
			Self::Unreachable { reason } => {
				("unreachable", Some(reason.as_str()))
			}
		}
	}

	fn parse(state: &str, reason: Option<String>) -> Result<Self, StoreError> {
		match (state, reason) {
			("reachable", None) => Ok(Self::Reachable),
			("unreachable", Some(reason)) => Ok(Self::Unreachable { reason }),
			(state, _) => Err(column_error(
				"state",
				format!("unknown or incomplete Provider reach {state:?}"),
			)),
		}
	}
}

impl ReadTransaction {
	/// Deduplicated consumption per Model for the selected rows.
	///
	/// A Run whose Craft reported a cumulative total contributes that one
	/// freshest total; a Run whose Craft reported turns contributes its
	/// turns. The two are never added to each other (ADR-0023).
	///
	/// # Errors
	///
	/// Returns a store error when the query fails, and an integrity error
	/// when a stored count does not fit the record.
	pub async fn usage_totals(
		&mut self,
		selection: UsageSelectionRecord,
	) -> Result<Vec<UsageTotalRecord>, StoreError> {
		let conversation_id =
			selection.conversation_id.map(|id| id.to_string());
		let run_id = selection.run_id.map(|id| id.to_string());
		let binding_id = selection.binding_id.map(|id| id.to_string());
		// ASVS 1.2.4: SQL structure is static; every dynamic value in this
		// module is passed through SQLite parameters.
		let rows = sqlx::query!(
			r#"WITH counted AS (
				SELECT o.model, o.input_tokens, o.cached_input_tokens,
					o.output_tokens, o.reasoning_tokens, o.estimation,
					o.finality, o.observed_at_unix_ms
				 FROM usage_observations o
				 WHERE (?1 IS NULL OR o.conversation_id = ?1)
				   AND (?2 IS NULL OR o.run_id = ?2)
				   AND (?3 IS NULL OR o.binding_id = ?3)
				   AND ((o.scope = 'run' AND o.rowid = (
					     SELECT r.rowid FROM usage_observations r
					      WHERE r.run_id = o.run_id AND r.scope = 'run'
					      ORDER BY r.observed_at_unix_ms DESC, r.rowid DESC
					      LIMIT 1))
				     OR (o.scope = 'turn' AND NOT EXISTS (
					     SELECT 1 FROM usage_observations r
					      WHERE r.run_id = o.run_id AND r.scope = 'run')))
			)
			SELECT model,
				CAST(SUM(input_tokens) AS INTEGER) AS "input!: i64",
				CAST(SUM(cached_input_tokens) AS INTEGER) AS "cached!: i64",
				CAST(SUM(output_tokens) AS INTEGER) AS "output!: i64",
				CAST(SUM(reasoning_tokens) AS INTEGER) AS "reasoning!: i64",
				COUNT(*) AS "measurements!: i64",
				CAST(SUM(estimation = 'estimated') AS INTEGER)
					AS "estimated!: i64",
				CAST(SUM(finality = 'interim') AS INTEGER) AS "interim!: i64",
				CAST(MIN(observed_at_unix_ms) AS INTEGER) AS "first!: i64",
				CAST(MAX(observed_at_unix_ms) AS INTEGER) AS "last!: i64"
			 FROM counted
			 GROUP BY model
			 ORDER BY model"#,
			conversation_id,
			run_id,
			binding_id
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter()
			.map(|row| {
				Ok(UsageTotalRecord {
					model: row.model,
					tokens: UsageTokensRecord {
						input: count("input_tokens", row.input)?,
						cached_input: count("cached_input_tokens", row.cached)?,
						output: count("output_tokens", row.output)?,
						reasoning: count("reasoning_tokens", row.reasoning)?,
					},
					measurements: count("measurements", row.measurements)?,
					estimated: count("estimation", row.estimated)?,
					interim: count("finality", row.interim)?,
					first_observed_at_unix_ms: row.first,
					last_observed_at_unix_ms: row.last,
				})
			})
			.collect()
	}

	/// The freshest snapshot of every quota window, newest response per
	/// window and never a sum across windows (ADR-0023).
	///
	/// # Errors
	///
	/// Returns a store error when the query fails, and an integrity error
	/// when a stored value is not the value its column names.
	pub async fn usage_quota_windows(
		&mut self,
		binding_id: Option<Uuid>,
	) -> Result<Vec<UsageQuotaSnapshotRecord>, StoreError> {
		let binding_id = binding_id.map(|id| id.to_string());
		let rows = sqlx::query!(
			r#"SELECT s.snapshot_id AS "snapshot_id!", s.binding_id,
				s.provider, s.window_id, s.scope, s.model,
				s.conversation_id, s.run_id, s.unit, s.used, s.limit_amount,
				s.window_seconds, s.resets_at_unix_ms, s.estimation,
				s.finality, s.observed_at_unix_ms, s.digest
			 FROM usage_quota_snapshots s
			 WHERE (?1 IS NULL OR s.binding_id = ?1)
			   AND s.rowid = (
				 SELECT t.rowid FROM usage_quota_snapshots t
				  WHERE t.binding_id = s.binding_id
				    AND t.window_id = s.window_id
				  ORDER BY t.observed_at_unix_ms DESC, t.rowid DESC
				  LIMIT 1)
			 ORDER BY s.binding_id, s.window_id"#,
			binding_id
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter()
			.map(|row| {
				Ok(UsageQuotaSnapshotRecord {
					snapshot_id: parse_uuid("snapshot_id", &row.snapshot_id)?,
					binding_id: parse_uuid("binding_id", &row.binding_id)?,
					provider: row.provider,
					window_id: row.window_id,
					scope: QuotaScopeRecord::parse(&row.scope).ok_or_else(
						|| {
							column_error(
								"scope",
								format!("unknown quota scope {:?}", row.scope),
							)
						},
					)?,
					model: row.model,
					conversation_id: parse_optional_uuid(
						"conversation_id",
						row.conversation_id.as_deref(),
					)?,
					run_id: parse_optional_uuid(
						"run_id",
						row.run_id.as_deref(),
					)?,
					unit: QuotaUnitRecord::parse(&row.unit).ok_or_else(
						|| {
							column_error(
								"unit",
								format!("unknown quota unit {:?}", row.unit),
							)
						},
					)?,
					used: count("used", row.used)?,
					limit_amount: row
						.limit_amount
						.map(|value| count("limit_amount", value))
						.transpose()?,
					window_seconds: row
						.window_seconds
						.map(|value| count("window_seconds", value))
						.transpose()?,
					resets_at_unix_ms: row.resets_at_unix_ms,
					estimation: estimation(&row.estimation)?,
					finality: finality(&row.finality)?,
					observed_at_unix_ms: row.observed_at_unix_ms,
					digest: parse_bytes("digest", row.digest)?,
				})
			})
			.collect()
	}

	/// The newest snapshot of one window, as deduplication reads it.
	///
	/// # Errors
	///
	/// Returns a store error when the query fails, and an integrity error
	/// when the stored digest is not a digest.
	pub async fn usage_quota_heartbeat(
		&mut self,
		binding_id: Uuid,
		window_id: &str,
	) -> Result<Option<UsageQuotaHeartbeatRecord>, StoreError> {
		let binding_id = binding_id.to_string();
		let row = sqlx::query!(
			"SELECT digest, observed_at_unix_ms
			 FROM usage_quota_snapshots
			 WHERE binding_id = ?1 AND window_id = ?2
			 ORDER BY observed_at_unix_ms DESC, rowid DESC
			 LIMIT 1",
			binding_id,
			window_id
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(|row| {
			Ok(UsageQuotaHeartbeatRecord {
				digest: parse_bytes("digest", row.digest)?,
				observed_at_unix_ms: row.observed_at_unix_ms,
			})
		})
		.transpose()
	}

	/// Whether each selected binding's Provider answered when Jet last
	/// asked. Bindings Jet has never asked about are absent.
	///
	/// # Errors
	///
	/// Returns a store error when the query fails, and an integrity error
	/// when a stored state is not a state.
	pub async fn usage_provider_reach(
		&mut self,
		binding_id: Option<Uuid>,
	) -> Result<Vec<UsageProviderReachRecord>, StoreError> {
		let binding_id = binding_id.map(|id| id.to_string());
		let rows = sqlx::query!(
			r#"SELECT binding_id AS "binding_id!", state, reason,
				observed_at_unix_ms
			 FROM usage_provider_reach
			 WHERE (?1 IS NULL OR binding_id = ?1)
			 ORDER BY binding_id"#,
			binding_id
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter()
			.map(|row| {
				Ok(UsageProviderReachRecord {
					binding_id: parse_uuid("binding_id", &row.binding_id)?,
					reach: ProviderReachRecord::parse(&row.state, row.reason)?,
					observed_at_unix_ms: row.observed_at_unix_ms,
				})
			})
			.collect()
	}
}

impl WriteTransaction {
	/// Store one Jet-observed measurement, replacing the row a repeated
	/// measurement already wrote rather than adding to it. Reports whether
	/// the row changed: an older repeat of a measurement already stored
	/// leaves it alone.
	///
	/// # Errors
	///
	/// Returns a store error when the write fails, and an integrity error
	/// when a count does not fit the store.
	pub async fn record_usage_observation(
		&mut self,
		observation: &NewUsageObservation,
	) -> Result<bool, StoreError> {
		let observation_id = observation.observation_id.to_string();
		let conversation_id = observation.conversation_id.to_string();
		let run_id = observation.run_id.to_string();
		let binding_id = observation.binding_id.map(|id| id.to_string());
		let scope = observation.scope.as_str();
		let estimation = observation.estimation.as_str();
		let finality = observation.finality.as_str();
		let input = stored("input_tokens", observation.tokens.input)?;
		let cached =
			stored("cached_input_tokens", observation.tokens.cached_input)?;
		let output = stored("output_tokens", observation.tokens.output)?;
		let reasoning =
			stored("reasoning_tokens", observation.tokens.reasoning)?;
		let changed = sqlx::query!(
			"INSERT INTO usage_observations
				(observation_id, conversation_id, run_id, measurement,
				 native_usage_id, binding_id, provider, model, scope,
				 estimation, finality, input_tokens, cached_input_tokens,
				 output_tokens, reasoning_tokens, observed_at_unix_ms)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
				 ?14, ?15, ?16)
			 ON CONFLICT (run_id, measurement) DO UPDATE SET
				native_usage_id = excluded.native_usage_id,
				binding_id = excluded.binding_id,
				provider = excluded.provider,
				model = excluded.model,
				scope = excluded.scope,
				estimation = excluded.estimation,
				finality = excluded.finality,
				input_tokens = excluded.input_tokens,
				cached_input_tokens = excluded.cached_input_tokens,
				output_tokens = excluded.output_tokens,
				reasoning_tokens = excluded.reasoning_tokens,
				observed_at_unix_ms = excluded.observed_at_unix_ms
			 WHERE excluded.observed_at_unix_ms
				>= usage_observations.observed_at_unix_ms",
			observation_id,
			conversation_id,
			run_id,
			observation.measurement,
			observation.native_usage_id,
			binding_id,
			observation.provider,
			observation.model,
			scope,
			estimation,
			finality,
			input,
			cached,
			output,
			reasoning,
			observation.observed_at_unix_ms
		)
		.execute(self.connection())
		.await?
		.rows_affected();
		Ok(changed > 0)
	}

	/// Append one Provider-reported quota window snapshot.
	///
	/// # Errors
	///
	/// Returns a store error when the write fails, and an integrity error
	/// when a reported amount does not fit the store.
	pub async fn record_usage_quota_snapshot(
		&mut self,
		snapshot: &UsageQuotaSnapshotRecord,
	) -> Result<(), StoreError> {
		let snapshot_id = snapshot.snapshot_id.to_string();
		let binding_id = snapshot.binding_id.to_string();
		let conversation_id = snapshot.conversation_id.map(|id| id.to_string());
		let run_id = snapshot.run_id.map(|id| id.to_string());
		let scope = snapshot.scope.as_str();
		let unit = snapshot.unit.as_str();
		let used = stored("used", snapshot.used)?;
		let limit_amount = snapshot
			.limit_amount
			.map(|value| stored("limit_amount", value))
			.transpose()?;
		let window_seconds = snapshot
			.window_seconds
			.map(|value| stored("window_seconds", value))
			.transpose()?;
		let estimation = snapshot.estimation.as_str();
		let finality = snapshot.finality.as_str();
		let digest = snapshot.digest.as_slice();
		sqlx::query!(
			"INSERT INTO usage_quota_snapshots
				(snapshot_id, binding_id, provider, window_id, scope, model,
				 conversation_id, run_id, unit, used, limit_amount,
				 window_seconds, resets_at_unix_ms, estimation, finality,
				 observed_at_unix_ms, digest)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
				 ?14, ?15, ?16, ?17)",
			snapshot_id,
			binding_id,
			snapshot.provider,
			snapshot.window_id,
			scope,
			snapshot.model,
			conversation_id,
			run_id,
			unit,
			used,
			limit_amount,
			window_seconds,
			snapshot.resets_at_unix_ms,
			estimation,
			finality,
			snapshot.observed_at_unix_ms,
			digest
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Record whether one binding's Provider answered. An older observation
	/// never overwrites a newer one.
	///
	/// # Errors
	///
	/// Returns a store error when the write fails.
	pub async fn record_usage_provider_reach(
		&mut self,
		record: &UsageProviderReachRecord,
	) -> Result<(), StoreError> {
		let binding_id = record.binding_id.to_string();
		let (state, reason) = record.reach.columns();
		sqlx::query!(
			"INSERT INTO usage_provider_reach
				(binding_id, state, reason, observed_at_unix_ms)
			 VALUES (?1, ?2, ?3, ?4)
			 ON CONFLICT (binding_id) DO UPDATE SET
				state = excluded.state,
				reason = excluded.reason,
				observed_at_unix_ms = excluded.observed_at_unix_ms
			 WHERE excluded.observed_at_unix_ms
				>= usage_provider_reach.observed_at_unix_ms",
			binding_id,
			state,
			reason,
			record.observed_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
}

fn estimation(text: &str) -> Result<UsageEstimationRecord, StoreError> {
	UsageEstimationRecord::parse(text).ok_or_else(|| {
		column_error("estimation", format!("unknown estimation {text:?}"))
	})
}

fn finality(text: &str) -> Result<UsageFinalityRecord, StoreError> {
	UsageFinalityRecord::parse(text).ok_or_else(|| {
		column_error("finality", format!("unknown finality {text:?}"))
	})
}

fn count(column: &str, value: i64) -> Result<u64, StoreError> {
	u64::try_from(value).map_err(|_| {
		column_error(column, "the stored count is negative".into())
	})
}

fn stored(column: &str, value: u64) -> Result<i64, StoreError> {
	i64::try_from(value).map_err(|_| {
		column_error(column, "the reported count does not fit the store".into())
	})
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
