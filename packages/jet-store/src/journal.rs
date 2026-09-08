//! The append-only Event journal (ADR-0020, ADR-0096). Sequence numbers are
//! total and monotonic within this Plane only (ADR-0069).

use std::collections::BTreeMap;

use crate::StoreError;
use crate::records::{
	ActorRecord, EventRecord, NewEvent, VerifiedSnapshotCoverage, column_error,
	parse_optional_uuid, parse_uuid,
};
use crate::transaction::{ReadTransaction, WriteTransaction};

/// Most operational Events removed in one compaction transaction.
pub const EVENT_COMPACTION_BATCH_LIMIT: usize = 256;

/// Most semantic transcript Events copied into a Conversation fork.
const FORK_CONTEXT_EVENT_LIMIT: usize = 256;

/// A bounded semantic Event slice for building a fork's launch context.
pub struct ForkContextEvents {
	/// Newest transcript Events through the selected checkpoint, in order.
	pub events: Vec<EventRecord>,
	/// Durable turn boundaries recorded per Run through the checkpoint.
	pub checkpoint_counts: BTreeMap<uuid::Uuid, u32>,
	/// Whether older matching Events were omitted by the fixed store bound.
	pub earlier_events_omitted: bool,
}

/// One `events` row as SQLite stores it, before its text columns are parsed
/// back into domain types.
pub(crate) struct Row {
	pub(crate) sequence: i64,
	pub(crate) event_id: String,
	pub(crate) actor_kind: String,
	pub(crate) actor_id: Option<String>,
	pub(crate) recorded_at_unix_ms: i64,
	pub(crate) conversation_id: Option<String>,
	pub(crate) run_id: Option<String>,
	pub(crate) kind: String,
	pub(crate) payload_version: i64,
	pub(crate) payload: String,
}

impl ReadTransaction {
	/// The newest bounded transcript slice at or before one checkpoint.
	///
	/// # Errors
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn fork_context_events(
		&mut self,
		conversation_id: uuid::Uuid,
		run_id: uuid::Uuid,
		checkpoint_turn: u32,
	) -> Result<ForkContextEvents, StoreError> {
		let conversation_id = conversation_id.to_string();
		let run_id = run_id.to_string();
		let checkpoint_turn = i64::from(checkpoint_turn);
		let limit =
			i64::try_from(FORK_CONTEXT_EVENT_LIMIT + 1).unwrap_or(i64::MAX);
		// ASVS 1.2.4/2.2.2: both identity values and the fixed allocation
		// bound are parameters. Semantic transcript rows cannot be compacted.
		let mut rows = sqlx::query_as!(
			Row,
			r#"SELECT sequence AS "sequence!", event_id, actor_kind,
				actor_id, recorded_at_unix_ms, conversation_id, run_id, kind,
				payload_version, payload
			 FROM events
			 WHERE conversation_id = ?1
				AND kind IN ('turn.input', 'turn.changed', 'run.output')
				AND sequence <= (
					SELECT sequence FROM events
					WHERE run_id = ?2
						AND kind = 'change.checkpoint_recorded'
						AND json_extract(payload, '$.turn') = ?3
					ORDER BY sequence DESC LIMIT 1
				)
			 ORDER BY sequence DESC LIMIT ?4"#,
			conversation_id,
			run_id,
			checkpoint_turn,
			limit,
		)
		.fetch_all(self.connection())
		.await?;
		let mut earlier_events_omitted = rows.len() > FORK_CONTEXT_EVENT_LIMIT;
		rows.truncate(FORK_CONTEXT_EVENT_LIMIT);
		rows.reverse();
		let events = rows
			.into_iter()
			.map(read_event_row)
			.collect::<Result<_, _>>()?;
		let mut checkpoint_rows = sqlx::query!(
			r#"SELECT run_id AS "run_id!", COUNT(*) AS "count!" FROM events
			 WHERE conversation_id = ?1
				AND run_id IS NOT NULL
				AND kind = 'change.checkpoint_recorded'
				AND sequence <= (
					SELECT sequence FROM events
					WHERE run_id = ?2
						AND kind = 'change.checkpoint_recorded'
						AND json_extract(payload, '$.turn') = ?3
					ORDER BY sequence DESC LIMIT 1
				)
			 GROUP BY run_id ORDER BY MAX(sequence) DESC LIMIT ?4"#,
			conversation_id,
			run_id,
			checkpoint_turn,
			limit,
		)
		.fetch_all(self.connection())
		.await?;
		earlier_events_omitted |=
			checkpoint_rows.len() > FORK_CONTEXT_EVENT_LIMIT;
		checkpoint_rows.truncate(FORK_CONTEXT_EVENT_LIMIT);
		let checkpoint_counts = checkpoint_rows
			.into_iter()
			.map(|row| {
				Ok::<_, StoreError>((
					parse_uuid("run_id", &row.run_id)?,
					u32::try_from(row.count).map_err(|_| {
						StoreError::Integrity(
							"checkpoint count is out of range".into(),
						)
					})?,
				))
			})
			.collect::<Result<_, _>>()?;
		Ok(ForkContextEvents {
			events,
			checkpoint_counts,
			earlier_events_omitted,
		})
	}

	/// Up to `limit` Events strictly after `cursor`, in sequence order.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn events_after(
		&mut self,
		cursor: u64,
		limit: usize,
	) -> Result<(u64, Vec<EventRecord>), StoreError> {
		let (current_snapshot_revision, cursor, limit) =
			self.event_window(cursor, limit).await?;
		let rows = sqlx::query_as!(
			Row,
			r#"SELECT sequence AS "sequence!", event_id, actor_kind, actor_id,
				recorded_at_unix_ms, conversation_id, run_id, kind,
				payload_version, payload
			 FROM events
			 WHERE sequence > ?1
			 ORDER BY sequence
			 LIMIT ?2"#,
			cursor,
			limit
		)
		.fetch_all(self.connection())
		.await?;
		let events = rows
			.into_iter()
			.map(read_event_row)
			.collect::<Result<_, _>>()?;
		Ok((current_snapshot_revision, events))
	}

	/// Up to `limit` Events excluding name changes unknown before Jet 1.20.
	/// Filtering precedes the SQL limit so hidden Events cannot starve a
	/// legacy peer of later Events it can consume.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn legacy_events_after(
		&mut self,
		cursor: u64,
		limit: usize,
	) -> Result<(u64, Vec<EventRecord>), StoreError> {
		let (_, cursor, limit) = self.event_window(cursor, limit).await?;
		let visible_snapshot_revision = sqlx::query_scalar!(
			r#"SELECT MAX(?1, COALESCE(MAX(sequence), 0)) AS "sequence!: i64"
			 FROM events
			 WHERE kind != 'conversation.name_changed'
			   AND kind != 'run.name_changed'"#,
			cursor,
		)
		.fetch_one(self.connection())
		.await?;
		let rows = sqlx::query_as!(
			Row,
			r#"SELECT sequence AS "sequence!", event_id, actor_kind, actor_id,
				recorded_at_unix_ms, conversation_id, run_id, kind,
				payload_version, payload
			 FROM events
			 WHERE sequence > ?1
			   AND kind != 'conversation.name_changed'
			   AND kind != 'run.name_changed'
			 ORDER BY sequence
			 LIMIT ?2"#,
			cursor,
			limit
		)
		.fetch_all(self.connection())
		.await?;
		let events = rows
			.into_iter()
			.map(read_event_row)
			.collect::<Result<_, _>>()?;
		Ok((parse_sequence(visible_snapshot_revision)?, events))
	}

	async fn event_window(
		&mut self,
		cursor: u64,
		limit: usize,
	) -> Result<(u64, i64, i64), StoreError> {
		let (current_snapshot_revision, minimum_available_cursor) =
			self.journal_position().await?;
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
		Ok((
			current_snapshot_revision,
			sequence_column(cursor)?,
			i64::try_from(limit).unwrap_or(i64::MAX),
		))
	}

	/// The sequence of the newest Event, or zero before any Event exists.
	/// Reading it inside the same snapshot as current state fences that
	/// snapshot for a later subscription (ADR-0092).
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the journal cannot be read.
	pub async fn event_cursor(&mut self) -> Result<u64, StoreError> {
		Ok(self.journal_position().await?.0)
	}

	pub(crate) async fn journal_position(
		&mut self,
	) -> Result<(u64, u64), StoreError> {
		let row = sqlx::query!(
			"SELECT high_water_sequence, minimum_replay_cursor
			 FROM event_journal_state WHERE singleton = 1"
		)
		.fetch_one(self.connection())
		.await?;
		Ok((
			parse_sequence(row.high_water_sequence)?,
			parse_sequence(row.minimum_replay_cursor)?,
		))
	}
}

impl WriteTransaction {
	/// Verifies coverage of the current Event high-water mark by the durable
	/// normalized projection in this transaction.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the journal position cannot be read.
	pub async fn verified_projection_coverage(
		&mut self,
	) -> Result<VerifiedSnapshotCoverage, StoreError> {
		let plane_id = self.plane().await?.plane_id;
		let sequence = self.event_cursor().await?;
		Ok(VerifiedSnapshotCoverage { plane_id, sequence })
	}

	/// Appends `event` at the next Plane sequence and returns the row.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written, including
	/// when the payload exceeds the journal bound.
	pub async fn append_event(
		&mut self,
		event: NewEvent,
	) -> Result<EventRecord, StoreError> {
		let (actor_kind, actor_id) = event.actor.columns();
		let event_id = event.event_id.to_string();
		let actor_id = actor_id.to_string();
		let conversation_id =
			event.conversation_id.as_ref().map(ToString::to_string);
		let run_id = event.run_id.as_ref().map(ToString::to_string);
		let payload_version = i64::from(event.payload_version);
		let class = event.class.as_str();
		let sequence = sqlx::query_scalar!(
			r#"INSERT INTO events (event_id, actor_kind, actor_id,
				recorded_at_unix_ms, conversation_id, run_id, kind,
				payload_version, payload, class)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
			 RETURNING sequence AS "sequence!""#,
			event_id,
			actor_kind,
			actor_id,
			event.recorded_at_unix_ms,
			conversation_id,
			run_id,
			event.kind,
			payload_version,
			event.payload,
			class
		)
		.fetch_one(self.connection())
		.await?;
		Ok(EventRecord {
			sequence: parse_sequence(sequence)?,
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
	pub async fn compact_operational_events(
		&mut self,
		coverage: VerifiedSnapshotCoverage,
		grace_before_unix_ms: i64,
	) -> Result<usize, StoreError> {
		let (coverage_plane_id, covered_through) = coverage.parts();
		let plane_id = self.plane().await?.plane_id;
		if coverage_plane_id != plane_id {
			return Err(StoreError::Integrity(format!(
				"snapshot coverage belongs to Plane {coverage_plane_id}, not {plane_id}"
			)));
		}
		let (high_water, _) = self.journal_position().await?;
		if covered_through > high_water {
			return Err(StoreError::Integrity(format!(
				"snapshot coverage {covered_through} is ahead of Event cursor {high_water}"
			)));
		}
		// ASVS 1.2.4: all compaction bounds are parameterized. ASVS 2.3.3
		// and 15.4.2: deletion and its cursor tombstone share this
		// transaction.
		let covered_through = sequence_column(covered_through)?;
		let batch =
			i64::try_from(EVENT_COMPACTION_BATCH_LIMIT).unwrap_or(i64::MAX);
		let last_removed = sqlx::query_scalar!(
			r#"SELECT MAX(sequence) AS "last_removed?: i64" FROM (
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
			)"#,
			covered_through,
			grace_before_unix_ms,
			batch
		)
		.fetch_one(self.connection())
		.await?;
		let Some(last_removed) = last_removed else {
			return Ok(0);
		};
		let removed = sqlx::query!(
			"DELETE FROM events
			 WHERE class = 'operational' AND sequence <= ?1
				AND recorded_at_unix_ms < ?2",
			last_removed,
			grace_before_unix_ms
		)
		.execute(self.connection())
		.await?
		.rows_affected();
		sqlx::query!(
			"UPDATE event_journal_state
			 SET minimum_replay_cursor = MAX(minimum_replay_cursor, ?1)
			 WHERE singleton = 1",
			last_removed
		)
		.execute(self.connection())
		.await?;
		Ok(usize::try_from(removed).unwrap_or(usize::MAX))
	}
}

pub(crate) fn read_event_row(row: Row) -> Result<EventRecord, StoreError> {
	Ok(EventRecord {
		sequence: parse_sequence(row.sequence)?,
		event_id: parse_uuid("event_id", &row.event_id)?,
		actor: parse_actor(&row.actor_kind, row.actor_id.as_deref())?,
		recorded_at_unix_ms: row.recorded_at_unix_ms,
		conversation_id: parse_optional_uuid(
			"conversation_id",
			row.conversation_id.as_deref(),
		)?,
		run_id: parse_optional_uuid("run_id", row.run_id.as_deref())?,
		kind: row.kind,
		payload_version: parse_payload_version(row.payload_version)?,
		payload: row.payload,
	})
}

fn parse_actor(
	kind: &str,
	id: Option<&str>,
) -> Result<ActorRecord, StoreError> {
	let Some(id) = id else {
		return Err(column_error(
			"actor_id",
			format!("actor {kind:?} has no identity"),
		));
	};
	ActorRecord::parse(kind, id)
}

pub(crate) fn sequence_column(sequence: u64) -> Result<i64, StoreError> {
	i64::try_from(sequence).map_err(|_| {
		StoreError::Integrity(format!("event sequence {sequence} overflows"))
	})
}

fn parse_sequence(sequence: i64) -> Result<u64, StoreError> {
	u64::try_from(sequence).map_err(|_| {
		column_error(
			"sequence",
			format!("event sequence {sequence} is negative"),
		)
	})
}

fn parse_payload_version(version: i64) -> Result<u32, StoreError> {
	u32::try_from(version).map_err(|_| {
		column_error(
			"payload_version",
			format!("payload version {version} is out of range"),
		)
	})
}
