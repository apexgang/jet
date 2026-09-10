//! Jet-observed consumption rows and the reads that keep them countable
//! (ADR-0023).
//!
//! What a Harness reported it used is identified so that repeating a
//! measurement replaces its row instead of adding to it. The
//! Provider-reported quota windows beside it live in `usage_quota`.

pub(crate) mod quota;

use crate::{
	ReadTransaction, StoreError, WriteTransaction, records::column_error,
};
use uuid::Uuid;

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
mod tests {
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use super::{
		NewUsageObservation, UsageEstimationRecord, UsageFinalityRecord,
		UsageScopeRecord, UsageSelectionRecord, UsageTokensRecord,
		UsageTotalRecord,
	};
	use crate::{
		ConversationOriginRecord, NewConversation, NewRun, RetentionPolicy,
		Store, WorkingTreeRecord,
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
				last_observed_at_unix_ms: NOW_UNIX_MS + 2,
			}]
		);
	}

	/// A Craft that restates one cumulative total per Model keeps one record
	/// per Model: the identity the Harness gave each measurement is what
	/// deduplicates them (ADR-0023).
	#[tokio::test]
	async fn cumulative_totals_of_two_models_are_two_measurements() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let (conversation_id, run_id) = conversation(&store).await;
		let mut opus =
			observation(conversation_id, run_id, "native:opus", tokens(10, 5));
		opus.scope = UsageScopeRecord::Run;
		let mut haiku =
			observation(conversation_id, run_id, "native:haiku", tokens(4, 1));
		haiku.scope = UsageScopeRecord::Run;
		haiku.model = Some("claude-haiku-4-5".into());
		let totals = store
			.write(async |tx| {
				tx.record_usage_observation(&observation(
					conversation_id,
					run_id,
					"turn:1",
					tokens(999, 999),
				))
				.await?;
				tx.record_usage_observation(&opus).await?;
				tx.record_usage_observation(&haiku).await?;
				tx.usage_totals(UsageSelectionRecord::default()).await
			})
			.await
			.unwrap();
		assert_eq!(
			totals,
			vec![
				UsageTotalRecord {
					model: Some("claude-haiku-4-5".into()),
					tokens: tokens(4, 1),
					measurements: 1,
					estimated: 0,
					interim: 0,
					last_observed_at_unix_ms: NOW_UNIX_MS,
				},
				UsageTotalRecord {
					model: Some("claude-opus-5".into()),
					tokens: tokens(10, 5),
					measurements: 1,
					estimated: 0,
					interim: 0,
					last_observed_at_unix_ms: NOW_UNIX_MS,
				}
			]
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
				last_observed_at_unix_ms: NOW_UNIX_MS + 5,
			}]
		);
	}
}
