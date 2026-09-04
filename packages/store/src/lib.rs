//! Concrete transactional SQLite store for authoritative Plane state
//! (ADR-0048).
//!
//! This is the only crate that links SQLite. It links one pinned bundled
//! build so every supported Plane shares identical transaction, migration,
//! and corruption behavior (ADR-0057). Authoritative state runs in WAL mode
//! with `synchronous=FULL`, so an acknowledged commit survives operating
//! system crash or power loss. SQL and migrations stay private; callers see
//! typed records and stable errors.

mod clock;
mod command;
mod conversation;
mod journal;
mod migrations;
mod plane;
mod records;
mod run;
mod transaction;

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

pub use plane::PlaneRecord;
pub use records::{
	ActorRecord, CommandReceiptRecord, ConversationRecord, EventRecord,
	NewCommandReceipt, NewConversation, NewEvent, NewRun, Retention,
	RunLifecycle, RunRecord,
};
pub use transaction::{ReadTransaction, WriteTransaction};

/// Failure inside the store, without native SQLite strings in the category.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
	/// The database could not be opened or reached.
	#[error("store unavailable: {0}")]
	Unavailable(String),
	/// The database is reachable but a statement or its data is broken.
	#[error("store integrity failure: {0}")]
	Integrity(String),
}

impl From<rusqlite::Error> for StoreError {
	fn from(error: rusqlite::Error) -> Self {
		match error {
			rusqlite::Error::SqliteFailure(code, _)
				if matches!(
					code.code,
					rusqlite::ErrorCode::CannotOpen
						| rusqlite::ErrorCode::DatabaseBusy
						| rusqlite::ErrorCode::DatabaseLocked
						| rusqlite::ErrorCode::SystemIoFailure
				) =>
			{
				Self::Unavailable(error.to_string())
			}
			other => Self::Integrity(other.to_string()),
		}
	}
}

/// One open Plane store. Current state and the Event journal are read and
/// written through [`Store::read`] and [`Store::write`].
#[derive(Debug)]
pub struct Store {
	connection: Mutex<Connection>,
}

impl Store {
	/// Opens or creates the store at `path` and applies pending migrations.
	///
	/// # Errors
	///
	/// Returns [`StoreError::Unavailable`] when the file cannot be opened
	/// and [`StoreError::Integrity`] when its schema cannot be prepared.
	pub fn open(path: &Path) -> Result<Self, StoreError> {
		let mut connection = Connection::open(path)?;
		connection.pragma_update(None, "journal_mode", "WAL")?;
		connection.pragma_update(None, "synchronous", "FULL")?;
		connection.pragma_update(None, "foreign_keys", "ON")?;
		verify_durability(&connection)?;
		migrations::apply(&mut connection)?;
		plane::ensure_present(&mut connection)?;
		Ok(Self {
			connection: Mutex::new(connection),
		})
	}

	/// Current Plane identity and daemon start count.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the Plane row cannot be read.
	pub fn plane(&self) -> Result<PlaneRecord, StoreError> {
		plane::read(&self.lock())
	}

	/// Durably records that an authoritative `jetd` started on this Plane and
	/// returns the updated record.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the increment cannot be committed.
	pub fn record_daemon_start(&self) -> Result<PlaneRecord, StoreError> {
		plane::record_daemon_start(&mut self.lock())
	}

	fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
		self.connection
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner)
	}
}

/// SQLite answers a refused `PRAGMA` with the mode it kept rather than an
/// error, so the durability settings are read back before any acknowledged
/// commit relies on them (ADR-0057, ADR-0071).
fn verify_durability(connection: &Connection) -> Result<(), StoreError> {
	let journal_mode: String =
		connection
			.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
	let synchronous: i64 =
		connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
	if journal_mode.eq_ignore_ascii_case("wal")
		&& synchronous == SYNCHRONOUS_FULL
	{
		Ok(())
	} else {
		Err(StoreError::Unavailable(format!(
			"the store runs with journal_mode {journal_mode} and synchronous {synchronous} instead of wal and full"
		)))
	}
}

/// SQLite's numeric value for `synchronous = FULL`.
const SYNCHRONOUS_FULL: i64 = 2;

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
