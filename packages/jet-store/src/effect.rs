//! Durable external-work Effect outbox (ADR-0064).

use uuid::Uuid;

use crate::StoreError;
use crate::records::{
	EffectKindRecord, EffectRecord, EffectSafetyRecord, EffectStateRecord,
	NewEffect, column_error, parse_optional_uuid, parse_uuid,
};
use crate::transaction::{ReadTransaction, WriteTransaction};

/// One `effects` row as SQLite stores it, before its text columns are parsed
/// back into domain types.
struct Row {
	effect_id: String,
	command_id: String,
	run_id: Option<String>,
	promotion_id: Option<String>,
	kind: String,
	safety: String,
	external_key: Option<String>,
	max_attempts: i64,
	state: String,
	attempt_count: i64,
}

impl ReadTransaction {
	/// Effects that still require first execution or restart reconciliation.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the outbox cannot be read.
	pub async fn unresolved_effects(
		&mut self,
	) -> Result<Vec<EffectRecord>, StoreError> {
		// ASVS 1.2.4: SQL structure is static; every dynamic value in this
		// module is passed through SQLite parameters.
		let rows = sqlx::query_as!(
			Row,
			r#"SELECT effect_id AS "effect_id!", command_id, run_id,
				promotion_id, kind, safety, external_key, max_attempts, state,
				attempt_count
			 FROM effects
			 WHERE state IN ('pending', 'in_flight')
			 ORDER BY rowid"#
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter().map(read_row).collect()
	}

	/// The unresolved Effects of one kind, so a worker that performs one
	/// kind of work leaves the others to theirs.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the outbox cannot be read.
	pub async fn unresolved_effects_of(
		&mut self,
		kind: EffectKindRecord,
	) -> Result<Vec<EffectRecord>, StoreError> {
		let kind = kind.as_str();
		let rows = sqlx::query_as!(
			Row,
			r#"SELECT effect_id AS "effect_id!", command_id, run_id,
				promotion_id, kind, safety, external_key, max_attempts, state,
				attempt_count
			 FROM effects
			 WHERE state IN ('pending', 'in_flight') AND kind = ?1
			 ORDER BY rowid"#,
			kind
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter().map(read_row).collect()
	}
}

impl WriteTransaction {
	/// Adds an Effect to the transaction that records its initiating change.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the Effect cannot be recorded.
	pub async fn insert_effect(
		&mut self,
		effect: &NewEffect,
	) -> Result<(), StoreError> {
		let (safety, external_key, max_attempts) = effect.safety.columns();
		let effect_id = effect.effect_id.to_string();
		let command_id = effect.command_id.to_string();
		let run_id = effect.run_id.as_ref().map(ToString::to_string);
		let promotion_id =
			effect.promotion_id.as_ref().map(ToString::to_string);
		let kind = effect.kind.as_str();
		let external_key = external_key.as_ref().map(ToString::to_string);
		let max_attempts = i64::from(max_attempts);
		sqlx::query!(
			"INSERT INTO effects (
				effect_id, command_id, run_id, promotion_id, kind, safety,
				external_key, max_attempts, state, attempt_count
			 ) VALUES (
				?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', 0
			 )",
			effect_id,
			command_id,
			run_id,
			promotion_id,
			kind,
			safety,
			external_key,
			max_attempts
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Durably records that an Adapter is about to perform an Effect attempt.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the Effect is terminal or cannot be
	/// updated.
	pub async fn begin_effect_attempt(
		&mut self,
		effect_id: Uuid,
	) -> Result<EffectRecord, StoreError> {
		let effect_id_column = effect_id.to_string();
		let changed = sqlx::query!(
			"UPDATE effects
			 SET state = 'in_flight', attempt_count = attempt_count + 1
			 WHERE effect_id = ?1 AND state IN ('pending', 'in_flight')",
			effect_id_column
		)
		.execute(self.connection())
		.await?
		.rows_affected();
		if changed != 1 {
			return Err(StoreError::Integrity(format!(
				"Effect {effect_id} is terminal"
			)));
		}
		self.effect(effect_id).await?.ok_or_else(|| {
			StoreError::Integrity(format!("Effect {effect_id} disappeared"))
		})
	}

	/// Records a definite or safety-terminal outcome for an in-flight Effect.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the Effect is not in flight or cannot be
	/// updated.
	pub async fn finish_effect(
		&mut self,
		effect_id: Uuid,
		state: EffectStateRecord,
	) -> Result<EffectRecord, StoreError> {
		if !matches!(
			state,
			EffectStateRecord::Completed
				| EffectStateRecord::Failed
				| EffectStateRecord::OutcomeUnknown
		) {
			return Err(StoreError::Integrity(
				"an Effect can finish only in a terminal state".into(),
			));
		}
		let effect_id_column = effect_id.to_string();
		let state_column = state.as_str();
		let changed = sqlx::query!(
			"UPDATE effects SET state = ?2
			 WHERE effect_id = ?1 AND state = 'in_flight'",
			effect_id_column,
			state_column
		)
		.execute(self.connection())
		.await?
		.rows_affected();
		if changed != 1 {
			return Err(StoreError::Integrity(format!(
				"Effect {effect_id} is not in flight"
			)));
		}
		self.effect(effect_id).await?.ok_or_else(|| {
			StoreError::Integrity(format!("Effect {effect_id} disappeared"))
		})
	}

	async fn effect(
		&mut self,
		effect_id: Uuid,
	) -> Result<Option<EffectRecord>, StoreError> {
		let effect_id = effect_id.to_string();
		let row = sqlx::query_as!(
			Row,
			r#"SELECT effect_id AS "effect_id!", command_id, run_id,
				promotion_id, kind, safety, external_key, max_attempts, state,
				attempt_count
			 FROM effects
			 WHERE effect_id = ?1"#,
			effect_id
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(read_row).transpose()
	}
}

fn read_row(row: Row) -> Result<EffectRecord, StoreError> {
	let max_attempts = parse_attempt_count("max_attempts", row.max_attempts)?;
	Ok(EffectRecord {
		effect_id: parse_uuid("effect_id", &row.effect_id)?,
		command_id: parse_uuid("command_id", &row.command_id)?,
		run_id: parse_optional_uuid("run_id", row.run_id.as_deref())?,
		promotion_id: parse_optional_uuid(
			"promotion_id",
			row.promotion_id.as_deref(),
		)?,
		// ASVS 1.5.2: durable kind input is decoded through a closed
		// allowlist.
		kind: EffectKindRecord::parse(&row.kind).ok_or_else(|| {
			column_error("kind", format!("unknown Effect kind {:?}", row.kind))
		})?,
		safety: parse_safety(
			&row.safety,
			row.external_key.as_deref(),
			max_attempts,
		)?,
		state: EffectStateRecord::parse(&row.state).ok_or_else(|| {
			column_error(
				"state",
				format!("unknown Effect state {:?}", row.state),
			)
		})?,
		attempt_count: parse_attempt_count("attempt_count", row.attempt_count)?,
	})
}

fn parse_safety(
	safety: &str,
	external_key: Option<&str>,
	max_attempts: u32,
) -> Result<EffectSafetyRecord, StoreError> {
	match (safety, external_key) {
		("read_only", None) => {
			Ok(EffectSafetyRecord::ReadOnly { max_attempts })
		}
		("idempotent", Some(external_key)) => {
			Ok(EffectSafetyRecord::Idempotent {
				external_key: parse_uuid("external_key", external_key)?,
				max_attempts,
			})
		}
		("ambiguous", None) if max_attempts == 1 => {
			Ok(EffectSafetyRecord::Ambiguous)
		}
		_ => Err(column_error(
			"safety",
			format!("invalid Effect safety {safety:?}"),
		)),
	}
}

fn parse_attempt_count(column: &str, value: i64) -> Result<u32, StoreError> {
	u32::try_from(value).map_err(|_| {
		column_error(column, format!("Effect attempt count {value} is invalid"))
	})
}
