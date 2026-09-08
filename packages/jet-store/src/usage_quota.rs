//! Provider-reported quota window rows, kept as the snapshot history
//! ADR-0045 retains rather than as one mutable current value.
//!
//! Reads select the freshest snapshot of each window: two windows of one
//! Account binding are separate limits and are never summed into one
//! another. Whether the Provider answered at all is recorded beside them,
//! so a Query can say `unreachable` instead of presenting an old window as
//! current.

use uuid::Uuid;

use crate::records::{
	column_error, parse_bytes, parse_optional_uuid, parse_uuid,
};
use crate::usage::{estimation, finality, read_count, stored_count};
use crate::{
	ReadTransaction, StoreError, UsageEstimationRecord, UsageFinalityRecord,
	WriteTransaction,
};

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
				s.finality, s.observed_at_unix_ms, s.digest
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
			"SELECT digest, observed_at_unix_ms
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

#[cfg(test)]
#[path = "usage_quota_tests.rs"]
mod tests;
