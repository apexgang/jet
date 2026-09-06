//! Versioned execution pins and projections, committed beside Run Events.

use crate::{ReadTransaction, StoreError, WriteTransaction};
use uuid::Uuid;

/// Execution metadata owned and versioned by the core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunExecutionRecord {
	/// Immutable launch plan, including the accepted Craft digest.
	pub plan: String,
	/// Current activity, processes, and native identity.
	pub state: String,
}

impl ReadTransaction {
	/// Reads a Run's execution metadata in the current snapshot.
	///
	/// # Errors
	/// Returns a store error if the query cannot complete.
	pub async fn run_execution(
		&mut self,
		run_id: Uuid,
	) -> Result<Option<RunExecutionRecord>, StoreError> {
		let id = run_id.to_string();
		Ok(sqlx::query_as!(
			RunExecutionRecord,
			"SELECT plan, state FROM run_executions WHERE run_id = ?1",
			id
		)
		.fetch_optional(self.connection())
		.await?)
	}
}

impl WriteTransaction {
	/// Inserts execution pins before any external work starts.
	///
	/// # Errors
	/// Returns a store error for unknown or duplicate Runs or invalid JSON.
	pub async fn insert_run_execution(
		&mut self,
		run_id: Uuid,
		record: &RunExecutionRecord,
	) -> Result<(), StoreError> {
		let id = run_id.to_string();
		sqlx::query!(
			"INSERT INTO run_executions (run_id, plan, state) VALUES (?1, ?2, ?3)",
			id,
			record.plan,
			record.state
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
	/// Stores a projection in the same transaction as its semantic Events.
	///
	/// # Errors
	/// Returns a store error when the update cannot complete.
	pub async fn update_run_execution(
		&mut self,
		run_id: Uuid,
		state: &str,
	) -> Result<(), StoreError> {
		let id = run_id.to_string();
		sqlx::query!(
			"UPDATE run_executions SET state = ?2 WHERE run_id = ?1",
			id,
			state
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
}
