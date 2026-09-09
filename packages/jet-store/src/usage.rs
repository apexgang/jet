//! Jet-observed consumption rows and the reads that keep them countable
//! (ADR-0023).
//!
//! What a Harness reported it used is identified so that repeating a
//! measurement replaces its row instead of adding to it. The
//! Provider-reported quota windows beside it live in `usage_quota`.

use uuid::Uuid;

use crate::records::column_error;
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
	/// When the newest contributing measurement was observed.
	pub last_observed_at_unix_ms: i64,
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

impl ReadTransaction {
	/// Deduplicated consumption per Model for the selected rows.
	///
	/// Each measurement counts once: its own row is already the freshest
	/// report of it. A Run whose Craft restated a cumulative total
	/// contributes those totals and not the turns they already cover; a
	/// Run whose Craft reported turns contributes its turns (ADR-0023).
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
				   AND (o.scope = 'run'
				     OR NOT EXISTS (
					     SELECT 1 FROM usage_observations r
					      WHERE r.run_id = o.run_id AND r.scope = 'run'))
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
					estimated: read_count("estimation", row.estimated)?,
					interim: read_count("finality", row.interim)?,
					last_observed_at_unix_ms: row.last,
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
		let input = stored_count("input_tokens", observation.tokens.input)?;
		let cached = stored_count(
			"cached_input_tokens",
			observation.tokens.cached_input,
		)?;
		let output = stored_count("output_tokens", observation.tokens.output)?;
		let reasoning =
			stored_count("reasoning_tokens", observation.tokens.reasoning)?;
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
}

pub(crate) fn estimation(
	text: &str,
) -> Result<UsageEstimationRecord, StoreError> {
	UsageEstimationRecord::parse(text).ok_or_else(|| {
		column_error("estimation", format!("unknown estimation {text:?}"))
	})
}

pub(crate) fn finality(text: &str) -> Result<UsageFinalityRecord, StoreError> {
	UsageFinalityRecord::parse(text).ok_or_else(|| {
		column_error("finality", format!("unknown finality {text:?}"))
	})
}

pub(crate) fn read_count(column: &str, value: i64) -> Result<u64, StoreError> {
	u64::try_from(value).map_err(|_| {
		column_error(column, "the stored count is negative".into())
	})
}

pub(crate) fn stored_count(
	column: &str,
	value: u64,
) -> Result<i64, StoreError> {
	i64::try_from(value).map_err(|_| {
		column_error(column, "the reported count does not fit the store".into())
	})
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
