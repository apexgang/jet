//! Current state of Runs: bounded executions of one Conversation
//! (ADR-0065).

use uuid::Uuid;

use crate::StoreError;
use crate::records::{
	NewRun, RunLifecycle, RunRecord, column_error, parse_uuid,
};
use crate::transaction::{ReadTransaction, WriteTransaction};

/// One `runs` row as SQLite stores it, before its text columns are parsed
/// back into domain types.
struct Row {
	run_id: String,
	conversation_id: String,
	revision: i64,
	lifecycle: String,
	created_at_unix_ms: i64,
	ended_at_unix_ms: Option<i64>,
}

impl ReadTransaction {
	/// The Run identified by `run_id`, if recorded.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn run(
		&mut self,
		run_id: Uuid,
	) -> Result<Option<RunRecord>, StoreError> {
		let run_id = run_id.to_string();
		let row = sqlx::query_as!(
			Row,
			r#"SELECT run_id AS "run_id!", conversation_id, revision,
				lifecycle, created_at_unix_ms, ended_at_unix_ms
			 FROM runs
			 WHERE run_id = ?1"#,
			run_id
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(read_row).transpose()
	}

	/// Every Run of `conversation_id` in creation order, terminal ones
	/// included.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn runs(
		&mut self,
		conversation_id: Uuid,
	) -> Result<Vec<RunRecord>, StoreError> {
		let conversation_id = conversation_id.to_string();
		let rows = sqlx::query_as!(
			Row,
			r#"SELECT run_id AS "run_id!", conversation_id, revision,
				lifecycle, created_at_unix_ms, ended_at_unix_ms
			 FROM runs
			 WHERE conversation_id = ?1
			 ORDER BY rowid"#,
			conversation_id
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter().map(read_row).collect()
	}

	/// Every Run of every Conversation that works in the Local checkout of
	/// `project_id`, in creation order, terminal ones included. Jet admits
	/// one live managed Run there at a time (ADR-0025); the caller decides
	/// which of these are live.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn local_checkout_runs(
		&mut self,
		project_id: Uuid,
	) -> Result<Vec<RunRecord>, StoreError> {
		let project_id = project_id.to_string();
		let rows = sqlx::query_as!(
			Row,
			r#"SELECT runs.run_id AS "run_id!", runs.conversation_id,
				runs.revision, runs.lifecycle, runs.created_at_unix_ms,
				runs.ended_at_unix_ms
			 FROM runs
			 JOIN conversations USING (conversation_id)
			 WHERE conversations.project_id = ?1
			   AND conversations.working_tree = 'local_checkout'
			 ORDER BY runs.rowid"#,
			project_id
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter().map(read_row).collect()
	}
}

impl WriteTransaction {
	/// Records a new Run in the `created` lifecycle state.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written, including
	/// when its Conversation is unknown or the identity is already taken.
	pub async fn insert_run(
		&mut self,
		run: NewRun,
	) -> Result<RunRecord, StoreError> {
		let record = RunRecord {
			run_id: run.run_id,
			conversation_id: run.conversation_id,
			revision: 1,
			lifecycle: RunLifecycle::Created,
			created_at_unix_ms: run.created_at_unix_ms,
			ended_at_unix_ms: None,
		};
		let run_id = record.run_id.to_string();
		let conversation_id = record.conversation_id.to_string();
		let revision = i64::try_from(record.revision).unwrap_or(i64::MAX);
		let lifecycle = record.lifecycle.as_str();
		sqlx::query!(
			"INSERT INTO runs (run_id, conversation_id, revision, lifecycle,
				created_at_unix_ms, ended_at_unix_ms)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
			run_id,
			conversation_id,
			revision,
			lifecycle,
			record.created_at_unix_ms,
			record.ended_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(record)
	}

	/// Moves `run_id` to `lifecycle`, stamping its end with `now_unix_ms` the
	/// first time the state is terminal, and returns the updated Run.
	///
	/// # Errors
	///
	/// Returns [`StoreError::Integrity`] when the Run is unknown, or another
	/// [`StoreError`] when the row cannot be written.
	pub async fn update_run_lifecycle(
		&mut self,
		run_id: Uuid,
		lifecycle: RunLifecycle,
		now_unix_ms: i64,
	) -> Result<RunRecord, StoreError> {
		let ended_at_unix_ms = lifecycle.is_terminal().then_some(now_unix_ms);
		let run_id_column = run_id.to_string();
		let lifecycle = lifecycle.as_str();
		sqlx::query!(
			"UPDATE runs
			 SET lifecycle = ?2,
			     ended_at_unix_ms = COALESCE(?3, ended_at_unix_ms)
			 WHERE run_id = ?1",
			run_id_column,
			lifecycle,
			ended_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		// Re-read rather than `RETURNING`: the revision trigger fires after
		// the update, so a returned row would carry the stale revision.
		self.run(run_id).await?.ok_or_else(|| {
			StoreError::Integrity(format!("run {run_id} does not exist"))
		})
	}
}

fn read_row(row: Row) -> Result<RunRecord, StoreError> {
	Ok(RunRecord {
		run_id: parse_uuid("run_id", &row.run_id)?,
		conversation_id: parse_uuid("conversation_id", &row.conversation_id)?,
		revision: parse_revision(row.revision)?,
		lifecycle: parse_lifecycle(&row.lifecycle)?,
		created_at_unix_ms: row.created_at_unix_ms,
		ended_at_unix_ms: row.ended_at_unix_ms,
	})
}

fn parse_lifecycle(text: &str) -> Result<RunLifecycle, StoreError> {
	RunLifecycle::parse(text).ok_or_else(|| {
		column_error("lifecycle", format!("unknown run lifecycle {text:?}"))
	})
}

fn parse_revision(revision: i64) -> Result<u64, StoreError> {
	u64::try_from(revision).map_err(|_| {
		column_error("revision", format!("run revision {revision} is negative"))
	})
}
