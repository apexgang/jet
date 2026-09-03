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
	ActorRecord, ConversationRecord, EventRecord, NewConversation, NewEvent,
	NewRun, Retention, RunLifecycle, RunRecord,
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

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
