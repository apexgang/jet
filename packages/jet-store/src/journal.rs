//! The append-only Event journal (ADR-0020, ADR-0096). Sequence numbers are
//! total and monotonic within this Plane only (ADR-0069).

use crate::StoreError;
use crate::records::{
	ActorRecord, EventRecord, NewEvent, VerifiedSnapshotCoverage, column_error,
	parse_optional_uuid, parse_uuid,
};
use crate::transaction::{ReadTransaction, WriteTransaction};
use rusqlite::{Row, params};

/// Most operational Events removed in one compaction transaction.
pub const EVENT_COMPACTION_BATCH_LIMIT: usize = 256;

const COLUMNS: &str = "sequence, event_id, actor_kind, actor_id, \
	recorded_at_unix_ms, conversation_id, run_id, kind, payload_version, \
	payload";

impl ReadTransaction<'_> {
	/// Up to `limit` Events strictly after `cursor`, in sequence order.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub fn events_after(
		&self,
		cursor: u64,
		limit: usize,
	) -> Result<(u64, Vec<EventRecord>), StoreError> {
		let (current_snapshot_revision, minimum_available_cursor) =
			self.journal_position()?;
		if cursor < minimum_available_cursor {
			return Err(StoreError::CursorExpired {
				minimum_available_cursor,
				current_snapshot_revision,
			});
		}
		if cursor > current_snapshot_revision {
			return Err(StoreError::CursorAhead {
				current_snapshot_revision,
			});
		}
		// ASVS 2.2.1/2.2.2: cap allocation-driving input again at the
		// trusted store seam, even when the caller already applies a limit.
		let limit = limit.min(EVENT_COMPACTION_BATCH_LIMIT);
		let mut statement = self.transaction.prepare(&format!(
			"SELECT {COLUMNS} FROM events WHERE sequence > ?1
			 ORDER BY sequence LIMIT ?2"
		))?;
		let rows = statement.query_map(
			(
				sequence_column(cursor)?,
				i64::try_from(limit).unwrap_or(i64::MAX),
			),
			read_row,
		)?;
		Ok((current_snapshot_revision, rows.collect::<Result<_, _>>()?))
	}

	/// The sequence of the newest Event, or zero before any Event exists.
	/// Reading it inside the same snapshot as current state fences that
	/// snapshot for a later subscription (ADR-0092).
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the journal cannot be read.
	pub fn event_cursor(&self) -> Result<u64, StoreError> {
		Ok(self.journal_position()?.0)
	}

	fn journal_position(&self) -> Result<(u64, u64), StoreError> {
		let (high_water, minimum): (i64, i64) = self.transaction.query_row(
			"SELECT high_water_sequence, minimum_replay_cursor
			 FROM event_journal_state WHERE singleton = 1",
			[],
			|row| Ok((row.get(0)?, row.get(1)?)),
		)?;
		Ok((parse_sequence(high_water)?, parse_sequence(minimum)?))
	}
}

impl WriteTransaction<'_> {
	/// Verifies coverage of the current Event high-water mark by the durable
	/// normalized projection in this transaction.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the journal position cannot be read.
	pub fn verified_projection_coverage(
		&self,
	) -> Result<VerifiedSnapshotCoverage, StoreError> {
		Ok(VerifiedSnapshotCoverage {
			plane_id: self.plane()?.plane_id,
			sequence: self.event_cursor()?,
		})
	}

	/// Appends `event` at the next Plane sequence and returns the row.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written, including
	/// when the payload exceeds the journal bound.
	pub fn append_event(
		&self,
		event: NewEvent,
	) -> Result<EventRecord, StoreError> {
		let (actor_kind, actor_id) = event.actor.columns();
		self.transaction.execute(
			"INSERT INTO events (event_id, actor_kind, actor_id,
				recorded_at_unix_ms, conversation_id, run_id, kind,
				payload_version, payload, class)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
			(
				event.event_id.to_string(),
				actor_kind,
				actor_id.to_string(),
				event.recorded_at_unix_ms,
				event.conversation_id.as_ref().map(ToString::to_string),
				event.run_id.as_ref().map(ToString::to_string),
				&event.kind,
				event.payload_version,
				&event.payload,
				event.class.as_str(),
			),
		)?;
		Ok(EventRecord {
			sequence: parse_sequence(self.transaction.last_insert_rowid())?,
			event_id: event.event_id,
			actor: event.actor,
			recorded_at_unix_ms: event.recorded_at_unix_ms,
			conversation_id: event.conversation_id,
			run_id: event.run_id,
			kind: event.kind,
			payload_version: event.payload_version,
			payload: event.payload,
		})
	}

	/// Removes one bounded batch of operational Events covered by a verified
	/// snapshot and older than the caller's grace-period cutoff. Semantic
	/// Conversation history is never selected (ADR-0078).
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the coverage is ahead of the journal or
	/// the compaction transaction cannot be completed.
	pub fn compact_operational_events(
		&self,
		coverage: VerifiedSnapshotCoverage,
		grace_before_unix_ms: i64,
	) -> Result<usize, StoreError> {
		let (coverage_plane_id, covered_through) = coverage.parts();
		let plane_id = self.plane()?.plane_id;
		if coverage_plane_id != plane_id {
			return Err(StoreError::Integrity(format!(
				"snapshot coverage belongs to Plane {coverage_plane_id}, not {plane_id}"
			)));
		}
		let (high_water, _) = self.journal_position()?;
		if covered_through > high_water {
			return Err(StoreError::Integrity(format!(
				"snapshot coverage {covered_through} is ahead of Event cursor {high_water}"
			)));
		}
		// ASVS 1.2.4: all compaction bounds are parameterized. ASVS 2.3.3
		// and 15.4.2: deletion and its cursor tombstone share this transaction.
		let last_removed: Option<i64> = self.transaction.query_row(
			"SELECT MAX(sequence) FROM (
				SELECT sequence FROM events
				WHERE class = 'operational'
					AND sequence <= ?1 AND recorded_at_unix_ms < ?2
					AND sequence < COALESCE((
						SELECT MIN(sequence) FROM events
						WHERE class = 'operational'
							AND sequence <= ?1
							AND recorded_at_unix_ms >= ?2
					), ?1 + 1)
				ORDER BY sequence
				LIMIT ?3
			)",
			params![
				sequence_column(covered_through)?,
				grace_before_unix_ms,
				i64::try_from(EVENT_COMPACTION_BATCH_LIMIT).unwrap_or(i64::MAX),
			],
			|row| row.get(0),
		)?;
		let Some(last_removed) = last_removed else {
			return Ok(0);
		};
		let removed = self.transaction.execute(
			"DELETE FROM events
			 WHERE class = 'operational' AND sequence <= ?1
				AND recorded_at_unix_ms < ?2",
			params![last_removed, grace_before_unix_ms],
		)?;
		self.transaction.execute(
			"UPDATE event_journal_state
			 SET minimum_replay_cursor = MAX(minimum_replay_cursor, ?1)
			 WHERE singleton = 1",
			[last_removed],
		)?;
		Ok(removed)
	}
}

fn read_row(row: &Row<'_>) -> rusqlite::Result<EventRecord> {
	let event_id: String = row.get(1)?;
	let actor_kind: String = row.get(2)?;
	let actor_id: Option<String> = row.get(3)?;
	let conversation_id: Option<String> = row.get(5)?;
	let run_id: Option<String> = row.get(6)?;
	Ok(EventRecord {
		sequence: parse_sequence(row.get(0)?)?,
		event_id: parse_uuid(1, &event_id)?,
		actor: parse_actor(&actor_kind, actor_id.as_deref())?,
		recorded_at_unix_ms: row.get(4)?,
		conversation_id: parse_optional_uuid(5, conversation_id.as_deref())?,
		run_id: parse_optional_uuid(6, run_id.as_deref())?,
		kind: row.get(7)?,
		payload_version: row.get(8)?,
		payload: row.get(9)?,
	})
}

fn parse_actor(kind: &str, id: Option<&str>) -> rusqlite::Result<ActorRecord> {
	let Some(id) = id else {
		return Err(column_error(2, format!("actor {kind:?} has no identity")));
	};
	ActorRecord::parse(kind, id, 3)
}

fn sequence_column(sequence: u64) -> Result<i64, StoreError> {
	i64::try_from(sequence).map_err(|_| {
		StoreError::Integrity(format!("event sequence {sequence} overflows"))
	})
}

fn parse_sequence(sequence: i64) -> rusqlite::Result<u64> {
	u64::try_from(sequence).map_err(|_| {
		column_error(0, format!("event sequence {sequence} is negative"))
	})
}
