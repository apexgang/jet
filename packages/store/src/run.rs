//! Current state of Runs: bounded executions of one Conversation
//! (ADR-0065).

use rusqlite::{OptionalExtension, Row};
use uuid::Uuid;

use crate::records::{
	NewRun, RunLifecycle, RunRecord, column_error, parse_uuid,
};
use crate::transaction::{ReadTransaction, WriteTransaction};
use crate::{StoreError, clock};

const COLUMNS: &str =
	"run_id, conversation_id, lifecycle, created_at_unix_ms, ended_at_unix_ms";

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
			lifecycle: RunLifecycle::Created,
			created_at_unix_ms: clock::unix_ms_now(),
			ended_at_unix_ms: None,
		};
		self.transaction.execute(
			&format!(
				"INSERT INTO runs ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5)"
			),
			(
				record.run_id.to_string(),
				record.conversation_id.to_string(),
				lifecycle_column(record.lifecycle),
				record.created_at_unix_ms,
				record.ended_at_unix_ms,
			),
		)?;
		Ok(record)
	}

	/// Moves `run_id` to `lifecycle`, stamping its end when the state is
	/// terminal, and returns the updated Run.
	///
	/// # Errors
	///
	/// Returns [`StoreError::Integrity`] when the Run is unknown, or another
	/// [`StoreError`] when the row cannot be written.
	pub fn update_run_lifecycle(
		&self,
		run_id: Uuid,
		lifecycle: RunLifecycle,
	) -> Result<RunRecord, StoreError> {
		let ended_at_unix_ms = lifecycle.is_terminal().then(clock::unix_ms_now);
		self.transaction.execute(
			"UPDATE runs SET lifecycle = ?2, ended_at_unix_ms = ?3
			 WHERE run_id = ?1",
			(
				run_id.to_string(),
				lifecycle_column(lifecycle),
				ended_at_unix_ms,
			),
		)?;
		self.run(run_id)?.ok_or_else(|| {
			StoreError::Integrity(format!("run {run_id} does not exist"))
		})
	}
}

fn read_row(row: &Row<'_>) -> rusqlite::Result<RunRecord> {
	let run_id: String = row.get(0)?;
	let conversation_id: String = row.get(1)?;
	let lifecycle: String = row.get(2)?;
	Ok(RunRecord {
		run_id: parse_uuid(0, &run_id)?,
		conversation_id: parse_uuid(1, &conversation_id)?,
		lifecycle: parse_lifecycle(&lifecycle)?,
		created_at_unix_ms: row.get(3)?,
		ended_at_unix_ms: row.get(4)?,
	})
}

fn lifecycle_column(lifecycle: RunLifecycle) -> &'static str {
	match lifecycle {
		RunLifecycle::Created => "created",
		RunLifecycle::Starting => "starting",
		RunLifecycle::Active => "active",
		RunLifecycle::Stopping => "stopping",
		RunLifecycle::Completed => "completed",
		RunLifecycle::Failed => "failed",
		RunLifecycle::Canceled => "canceled",
		RunLifecycle::Lost => "lost",
	}
}

fn parse_lifecycle(text: &str) -> rusqlite::Result<RunLifecycle> {
	match text {
		"created" => Ok(RunLifecycle::Created),
		"starting" => Ok(RunLifecycle::Starting),
		"active" => Ok(RunLifecycle::Active),
		"stopping" => Ok(RunLifecycle::Stopping),
		"completed" => Ok(RunLifecycle::Completed),
		"failed" => Ok(RunLifecycle::Failed),
		"canceled" => Ok(RunLifecycle::Canceled),
		"lost" => Ok(RunLifecycle::Lost),
		other => {
			Err(column_error(2, format!("unknown run lifecycle {other:?}")))
		}
	}
}
