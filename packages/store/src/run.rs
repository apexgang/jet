//! Current state of Runs: bounded executions of one Conversation
//! (ADR-0065).

use rusqlite::{OptionalExtension, Row};
use uuid::Uuid;

use crate::StoreError;
use crate::records::{
	NewRun, RunLifecycle, RunRecord, column_error, parse_uuid,
};
use crate::transaction::{ReadTransaction, WriteTransaction};

const COLUMNS: &str = "run_id, conversation_id, revision, lifecycle, created_at_unix_ms, \
	ended_at_unix_ms";

impl ReadTransaction<'_> {
	/// The Run identified by `run_id`, if recorded.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub fn run(&self, run_id: Uuid) -> Result<Option<RunRecord>, StoreError> {
		Ok(self
			.transaction
			.query_row(
				&format!("SELECT {COLUMNS} FROM runs WHERE run_id = ?1"),
				[run_id.to_string()],
				read_row,
			)
			.optional()?)
	}

	/// Every Run of `conversation_id` in creation order, terminal ones
	/// included.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub fn runs(
		&self,
		conversation_id: Uuid,
	) -> Result<Vec<RunRecord>, StoreError> {
		let mut statement = self.transaction.prepare(&format!(
			"SELECT {COLUMNS} FROM runs WHERE conversation_id = ?1
			 ORDER BY rowid"
		))?;
		let rows =
			statement.query_map([conversation_id.to_string()], read_row)?;
		Ok(rows.collect::<Result<_, _>>()?)
	}
}

impl WriteTransaction<'_> {
	/// Records a new Run in the `created` lifecycle state.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written, including
	/// when its Conversation is unknown or the identity is already taken.
	pub fn insert_run(&self, run: NewRun) -> Result<RunRecord, StoreError> {
		let record = RunRecord {
			run_id: run.run_id,
			conversation_id: run.conversation_id,
			revision: 1,
			lifecycle: RunLifecycle::Created,
			created_at_unix_ms: run.created_at_unix_ms,
			ended_at_unix_ms: None,
		};
		self.transaction.execute(
			&format!(
				"INSERT INTO runs ({COLUMNS})
				 VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
			),
			(
				record.run_id.to_string(),
				record.conversation_id.to_string(),
				i64::try_from(record.revision).unwrap_or(i64::MAX),
				record.lifecycle.as_str(),
				record.created_at_unix_ms,
				record.ended_at_unix_ms,
			),
		)?;
		Ok(record)
	}

	/// Moves `run_id` to `lifecycle`, stamping its end with `now_unix_ms` the
	/// first time the state is terminal, and returns the updated Run.
	///
	/// # Errors
	///
	/// Returns [`StoreError::Integrity`] when the Run is unknown, or another
	/// [`StoreError`] when the row cannot be written.
	pub fn update_run_lifecycle(
		&self,
		run_id: Uuid,
		lifecycle: RunLifecycle,
		now_unix_ms: i64,
	) -> Result<RunRecord, StoreError> {
		let ended_at_unix_ms = lifecycle.is_terminal().then_some(now_unix_ms);
		self.transaction.execute(
			"UPDATE runs
			 SET lifecycle = ?2,
			     ended_at_unix_ms = COALESCE(?3, ended_at_unix_ms)
			 WHERE run_id = ?1",
			(run_id.to_string(), lifecycle.as_str(), ended_at_unix_ms),
		)?;
		self.run(run_id)?.ok_or_else(|| {
			StoreError::Integrity(format!("run {run_id} does not exist"))
		})
	}
}

fn read_row(row: &Row<'_>) -> rusqlite::Result<RunRecord> {
	let run_id: String = row.get(0)?;
	let conversation_id: String = row.get(1)?;
	let lifecycle: String = row.get(3)?;
	Ok(RunRecord {
		run_id: parse_uuid(0, &run_id)?,
		conversation_id: parse_uuid(1, &conversation_id)?,
		revision: parse_revision(row.get(2)?)?,
		lifecycle: parse_lifecycle(&lifecycle)?,
		created_at_unix_ms: row.get(4)?,
		ended_at_unix_ms: row.get(5)?,
	})
}

fn parse_lifecycle(text: &str) -> rusqlite::Result<RunLifecycle> {
	RunLifecycle::parse(text).ok_or_else(|| {
		column_error(3, format!("unknown run lifecycle {text:?}"))
	})
}

fn parse_revision(revision: i64) -> rusqlite::Result<u64> {
	u64::try_from(revision).map_err(|_| {
		column_error(2, format!("run revision {revision} is negative"))
	})
}
