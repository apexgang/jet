//! Durable external-work Effect outbox (ADR-0064).

use rusqlite::Row;
use uuid::Uuid;

use crate::StoreError;
use crate::records::{
	EffectKindRecord, EffectRecord, EffectSafetyRecord, EffectStateRecord,
	NewEffect, column_error, parse_optional_uuid, parse_uuid,
};
use crate::transaction::{ReadTransaction, WriteTransaction};

const COLUMNS: &str = "effect_id, command_id, run_id, kind, safety, \
	external_key, max_attempts, state, attempt_count";

impl ReadTransaction<'_> {
	/// Effects that still require first execution or restart reconciliation.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the outbox cannot be read.
	pub fn unresolved_effects(&self) -> Result<Vec<EffectRecord>, StoreError> {
		// ASVS 1.2.4: SQL structure is static; every dynamic value in this
		// module is passed through SQLite parameters.
		let mut statement = self.transaction.prepare(&format!(
			"SELECT {COLUMNS} FROM effects
			 WHERE state IN ('pending', 'in_flight') ORDER BY rowid"
		))?;
		let rows = statement.query_map([], read_row)?;
		Ok(rows.collect::<Result<_, _>>()?)
	}
}

impl WriteTransaction<'_> {
	/// Adds an Effect to the transaction that records its initiating change.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the Effect cannot be recorded.
	pub fn insert_effect(&self, effect: &NewEffect) -> Result<(), StoreError> {
		let (safety, external_key, max_attempts) = effect.safety.columns();
		self.transaction.execute(
			"INSERT INTO effects (
				effect_id, command_id, run_id, kind, safety, external_key,
				max_attempts, state, attempt_count
			 ) VALUES (
				?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', 0
			 )",
			(
				effect.effect_id.to_string(),
				effect.command_id.to_string(),
				effect.run_id.as_ref().map(ToString::to_string),
				effect.kind.as_str(),
				safety,
				external_key.as_ref().map(ToString::to_string),
				i64::from(max_attempts),
			),
		)?;
		Ok(())
	}

	/// Durably records that an Adapter is about to perform an Effect attempt.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the Effect is terminal or cannot be updated.
	pub fn begin_effect_attempt(
		&self,
		effect_id: Uuid,
	) -> Result<EffectRecord, StoreError> {
		let changed = self.transaction.execute(
			"UPDATE effects
			 SET state = 'in_flight', attempt_count = attempt_count + 1
			 WHERE effect_id = ?1 AND state IN ('pending', 'in_flight')",
			[effect_id.to_string()],
		)?;
		if changed != 1 {
			return Err(StoreError::Integrity(format!(
				"Effect {effect_id} is terminal"
			)));
		}
		self.effect(effect_id)?.ok_or_else(|| {
			StoreError::Integrity(format!("Effect {effect_id} disappeared"))
		})
	}

	/// Records a definite or safety-terminal outcome for an in-flight Effect.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the Effect is not in flight or cannot be
	/// updated.
	pub fn finish_effect(
		&self,
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
		let changed = self.transaction.execute(
			"UPDATE effects SET state = ?2
			 WHERE effect_id = ?1 AND state = 'in_flight'",
			(effect_id.to_string(), state.as_str()),
		)?;
		if changed != 1 {
			return Err(StoreError::Integrity(format!(
				"Effect {effect_id} is not in flight"
			)));
		}
		self.effect(effect_id)?.ok_or_else(|| {
			StoreError::Integrity(format!("Effect {effect_id} disappeared"))
		})
	}

	fn effect(
		&self,
		effect_id: Uuid,
	) -> Result<Option<EffectRecord>, StoreError> {
		use rusqlite::OptionalExtension;

		Ok(self
			.transaction
			.query_row(
				&format!("SELECT {COLUMNS} FROM effects WHERE effect_id = ?1"),
				[effect_id.to_string()],
				read_row,
			)
			.optional()?)
	}
}

fn read_row(row: &Row<'_>) -> rusqlite::Result<EffectRecord> {
	let effect_id: String = row.get(0)?;
	let command_id: String = row.get(1)?;
	let run_id: Option<String> = row.get(2)?;
	let kind: String = row.get(3)?;
	let safety: String = row.get(4)?;
	let external_key: Option<String> = row.get(5)?;
	let max_attempts = parse_attempt_count(6, row.get(6)?)?;
	let state: String = row.get(7)?;
	Ok(EffectRecord {
		effect_id: parse_uuid(0, &effect_id)?,
		command_id: parse_uuid(1, &command_id)?,
		run_id: parse_optional_uuid(2, run_id.as_deref())?,
		// ASVS 1.5.2: durable kind input is decoded through a closed allowlist.
		kind: EffectKindRecord::parse(&kind).ok_or_else(|| {
			column_error(3, format!("unknown Effect kind {kind:?}"))
		})?,
		safety: parse_safety(&safety, external_key.as_deref(), max_attempts)?,
		state: EffectStateRecord::parse(&state).ok_or_else(|| {
			column_error(7, format!("unknown Effect state {state:?}"))
		})?,
		attempt_count: parse_attempt_count(8, row.get(8)?)?,
	})
}

fn parse_safety(
	safety: &str,
	external_key: Option<&str>,
	max_attempts: u32,
) -> rusqlite::Result<EffectSafetyRecord> {
	match (safety, external_key) {
		("read_only", None) => {
			Ok(EffectSafetyRecord::ReadOnly { max_attempts })
		}
		("idempotent", Some(external_key)) => {
			Ok(EffectSafetyRecord::Idempotent {
				external_key: parse_uuid(5, external_key)?,
				max_attempts,
			})
		}
		("ambiguous", None) if max_attempts == 1 => {
			Ok(EffectSafetyRecord::Ambiguous)
		}
		_ => Err(column_error(4, format!("invalid Effect safety {safety:?}"))),
	}
}

fn parse_attempt_count(index: usize, value: i64) -> rusqlite::Result<u32> {
	u32::try_from(value).map_err(|_| {
		column_error(index, format!("Effect attempt count {value} is invalid"))
	})
}
