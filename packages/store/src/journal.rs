//! The append-only Event journal (ADR-0020, ADR-0096). Sequence numbers are
//! total and monotonic within this Plane only (ADR-0069).

use rusqlite::Row;
use uuid::Uuid;

use crate::records::{
	ActorRecord, EventRecord, NewEvent, column_error, parse_optional_uuid,
	parse_uuid,
};
use crate::transaction::{ReadTransaction, WriteTransaction};
use crate::{StoreError, clock};

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
	) -> Result<Vec<EventRecord>, StoreError> {
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
		Ok(rows.collect::<Result<_, _>>()?)
	}

	/// The sequence of the newest Event, or zero before any Event exists.
	/// Reading it inside the same snapshot as current state fences that
	/// snapshot for a later subscription (ADR-0092).
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the journal cannot be read.
	pub fn event_cursor(&self) -> Result<u64, StoreError> {
		let cursor: i64 = self.transaction.query_row(
			"SELECT COALESCE(MAX(sequence), 0) FROM events",
			[],
			|row| row.get(0),
		)?;
		Ok(parse_sequence(cursor)?)
	}
}

impl WriteTransaction<'_> {
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
		let recorded_at_unix_ms = clock::unix_ms_now();
		let (actor_kind, actor_id) = actor_columns(event.actor);
		self.transaction.execute(
			"INSERT INTO events (event_id, actor_kind, actor_id,
				recorded_at_unix_ms, conversation_id, run_id, kind,
				payload_version, payload)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
			(
				event.event_id.to_string(),
				actor_kind,
				actor_id.map(|id| id.to_string()),
				recorded_at_unix_ms,
				event.conversation_id.map(|id| id.to_string()),
				event.run_id.map(|id| id.to_string()),
				&event.kind,
				event.payload_version,
				&event.payload,
			),
		)?;
		Ok(EventRecord {
			sequence: parse_sequence(self.transaction.last_insert_rowid())?,
			event_id: event.event_id,
			actor: event.actor,
			recorded_at_unix_ms,
			conversation_id: event.conversation_id,
			run_id: event.run_id,
			kind: event.kind,
			payload_version: event.payload_version,
			payload: event.payload,
		})
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

fn actor_columns(actor: ActorRecord) -> (&'static str, Option<Uuid>) {
	match actor {
		ActorRecord::InteractiveClient { client_id } => {
			("interactive_client", Some(client_id))
		}
	}
}

fn parse_actor(kind: &str, id: Option<&str>) -> rusqlite::Result<ActorRecord> {
	match (kind, id) {
		("interactive_client", Some(client_id)) => {
			Ok(ActorRecord::InteractiveClient {
				client_id: parse_uuid(3, client_id)?,
			})
		}
		(kind, id) => Err(column_error(
			2,
			format!("unknown actor {kind:?} with id {id:?}"),
		)),
	}
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
