//! Provider-reported quota window rows, kept as the snapshot history
//! ADR-0045 retains rather than as one mutable current value.
//!
//! Reads select the freshest snapshot of each window: two windows of one
//! Account binding are separate limits and are never summed into one
//! another. Each window also carries when its Provider last confirmed it,
//! so a repeated answer keeps that window fresh without keeping the rest
//! fresh with it. Whether the Provider answered at all is recorded beside
//! them, so a Query can say `unreachable` instead of presenting an old
//! window as current.

use crate::{
	ReadTransaction, StoreError, UsageEstimationRecord, UsageFinalityRecord,
	WriteTransaction,
	records::{column_error, parse_bytes, parse_optional_uuid, parse_uuid},
	usage::{estimation, finality, read_count, stored_count},
};
use uuid::Uuid;

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
	/// When the Provider last confirmed this window, which is the last
	/// time it answered about this window rather than about another one.
	pub answered_at_unix_ms: i64,
	/// Digest of the reported content, so an unchanged response is a
	/// heartbeat rather than another row.
	pub digest: [u8; 32],
}

/// The newest snapshot of one window, reduced to what deduplication needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageQuotaHeartbeatRecord {
	/// The row it read, so a repeat can confirm the window it belongs to.
	pub snapshot_id: Uuid,
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
				s.finality, s.observed_at_unix_ms, s.answered_at_unix_ms,
				s.digest
			 FROM usage_quota_snapshots s
			 WHERE (?1 IS NULL OR s.binding_id = ?1)
			   AND s.rowid = (
				 SELECT t.rowid FROM usage_quota_snapshots t
				  WHERE t.binding_id = s.binding_id
				    AND t.window_id = s.window_id
				    AND t.model IS s.model
				  ORDER BY t.observed_at_unix_ms DESC, t.rowid DESC
				  LIMIT 1)
			 ORDER BY s.binding_id, s.window_id, s.model"#,
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
					used: read_count("used", row.used)?,
					limit_amount: row
						.limit_amount
						.map(|value| read_count("limit_amount", value))
						.transpose()?,
					window_seconds: row
						.window_seconds
						.map(|value| read_count("window_seconds", value))
						.transpose()?,
					resets_at_unix_ms: row.resets_at_unix_ms,
					estimation: estimation(&row.estimation)?,
					finality: finality(&row.finality)?,
					observed_at_unix_ms: row.observed_at_unix_ms,
					answered_at_unix_ms: row.answered_at_unix_ms,
					digest: parse_bytes("digest", row.digest)?,
				})
			})
			.collect()
	}

	/// The newest snapshot of one window of one binding. A window the
	/// Provider spells the same way for two Models is two windows, so
	/// what it covers is part of reading it back.
	///
	/// # Errors
	///
	/// Returns a store error when the query fails, and an integrity error
	/// when the stored digest is not a digest.
	pub async fn usage_quota_heartbeat(
		&mut self,
		binding_id: Uuid,
		window_id: &str,
		model: Option<&str>,
	) -> Result<Option<UsageQuotaHeartbeatRecord>, StoreError> {
		let binding_id = binding_id.to_string();
		let row = sqlx::query!(
			"SELECT snapshot_id, digest, observed_at_unix_ms
			 FROM usage_quota_snapshots
			 WHERE binding_id = ?1 AND window_id = ?2 AND model IS ?3
			 ORDER BY observed_at_unix_ms DESC, rowid DESC
			 LIMIT 1",
			binding_id,
			window_id,
			model
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(|row| {
			Ok(UsageQuotaHeartbeatRecord {
				snapshot_id: parse_uuid("snapshot_id", &row.snapshot_id)?,
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
		let used = stored_count("used", snapshot.used)?;
		let limit_amount = snapshot
			.limit_amount
			.map(|value| stored_count("limit_amount", value))
			.transpose()?;
		let window_seconds = snapshot
			.window_seconds
			.map(|value| stored_count("window_seconds", value))
			.transpose()?;
		let estimation = snapshot.estimation.as_str();
		let finality = snapshot.finality.as_str();
		let digest = snapshot.digest.as_slice();
		sqlx::query!(
			"INSERT INTO usage_quota_snapshots
				(snapshot_id, binding_id, provider, window_id, scope, model,
				 conversation_id, run_id, unit, used, limit_amount,
				 window_seconds, resets_at_unix_ms, estimation, finality,
				 observed_at_unix_ms, answered_at_unix_ms, digest)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
				 ?14, ?15, ?16, ?17, ?18)",
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
			snapshot.answered_at_unix_ms,
			digest
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Record that the Provider confirmed one stored window again. An
	/// unchanged response is a heartbeat rather than another snapshot, and
	/// this is what lets it count as an answer about the window it repeats,
	/// and about no other one (ADR-0045).
	///
	/// # Errors
	///
	/// Returns a store error when the write fails.
	pub async fn record_usage_quota_answer(
		&mut self,
		snapshot_id: Uuid,
		answered_at_unix_ms: i64,
	) -> Result<(), StoreError> {
		let snapshot_id = snapshot_id.to_string();
		sqlx::query!(
			"UPDATE usage_quota_snapshots
			    SET answered_at_unix_ms = ?2
			  WHERE snapshot_id = ?1 AND answered_at_unix_ms <= ?2",
			snapshot_id,
			answered_at_unix_ms
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

#[cfg(test)]
mod tests {
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
		let newest =
			snapshot(binding_id, "five_hour", 4_200, NOW_UNIX_MS + 60_000);
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
}
