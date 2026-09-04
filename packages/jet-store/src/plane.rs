//! The single Plane row: durable identity plus daemon lifecycle counters.

use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use crate::StoreError;
use crate::transaction::ReadTransaction;

/// Durable identity and daemon lifecycle counters of the Plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneRecord {
	/// Identity created when the store was first created.
	pub plane_id: Uuid,
	/// Number of authoritative `jetd` starts recorded so far.
	pub daemon_starts: u64,
}

impl ReadTransaction<'_> {
	/// Reads the Plane record inside this transaction's consistent snapshot.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the Plane row cannot be read.
	pub fn plane(&self) -> Result<PlaneRecord, StoreError> {
		read(&self.transaction)
	}
}

pub(crate) fn ensure_present(
	connection: &mut Connection,
) -> Result<(), StoreError> {
	let transaction = connection.transaction()?;
	let existing: Option<String> = transaction
		.query_row(
			"SELECT plane_id FROM plane WHERE singleton = 1",
			[],
			|row| row.get(0),
		)
		.optional()?;
	if existing.is_none() {
		transaction.execute(
			"INSERT INTO plane (singleton, plane_id, daemon_starts)
			 VALUES (1, ?1, 0)",
			[Uuid::now_v7().to_string()],
		)?;
	}
	transaction.commit()?;
	Ok(())
}

pub(crate) fn read(connection: &Connection) -> Result<PlaneRecord, StoreError> {
	let (plane_id, daemon_starts): (String, i64) = connection.query_row(
		"SELECT plane_id, daemon_starts FROM plane WHERE singleton = 1",
		[],
		|row| Ok((row.get(0)?, row.get(1)?)),
	)?;
	Ok(PlaneRecord {
		plane_id: Uuid::parse_str(&plane_id).map_err(|error| {
			StoreError::Integrity(format!("plane_id is not a UUID: {error}"))
		})?,
		daemon_starts: u64::try_from(daemon_starts).map_err(|_| {
			StoreError::Integrity("daemon_starts is negative".into())
		})?,
	})
}

pub(crate) fn record_daemon_start(
	connection: &mut Connection,
) -> Result<PlaneRecord, StoreError> {
	let transaction = connection.transaction()?;
	transaction.execute(
		"UPDATE plane SET daemon_starts = daemon_starts + 1 WHERE singleton = 1",
		[],
	)?;
	let record = read(&transaction)?;
	transaction.commit()?;
	Ok(record)
}
