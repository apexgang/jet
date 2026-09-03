//! Jet core: the deep Module behind `jetd` (ADR-0047).
//!
//! The core exposes three conceptual entry points: execute an authenticated
//! Command, run a Query returning a snapshot, and subscribe from an Event
//! journal cursor. This slice implements the Query entry point; Commands and
//! Events arrive with the Conversation persistence work.
//!
//! Domain types here never double as wire types (ADR-0049); `jetd`
//! translates at the transport seam.

mod error;
mod status;

use std::time::SystemTime;

use jet_store::Store;
use uuid::Uuid;

pub use error::{CoreError, ErrorCategory};
pub use status::PlaneStatus;

/// Version of the running core, reported in status snapshots.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Durable identity of one Jet installation (see `Client identity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(pub Uuid);

/// Durable identity of one Plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlaneId(pub Uuid);

/// The authenticated origin of a Command or Query (ADR-0063).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
	/// An interactive GUI client authorized through owner-only local IPC.
	InteractiveClient {
		/// The client's durable identity.
		client_id: ClientId,
	},
}

/// Read-only requests answered with a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query {
	/// Snapshot of the daemon's Plane status.
	Status,
}

/// Snapshots returned by [`Core::query`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryResult {
	/// Snapshot of the daemon's Plane status.
	Status(PlaneStatus),
}

/// One running core bound to one Plane store.
#[derive(Debug)]
pub struct Core {
	store: Store,
	started_at: SystemTime,
}

impl Core {
	/// Starts the core on `store`, durably recording this daemon start.
	///
	/// # Errors
	///
	/// Returns [`CoreError`] with an `unavailable` or `internal` category
	/// when the start cannot be committed.
	pub fn start(store: Store) -> Result<Self, CoreError> {
		store.record_daemon_start()?;
		Ok(Self {
			store,
			started_at: SystemTime::now(),
		})
	}

	/// Runs `query` on behalf of `actor` and returns its snapshot.
	///
	/// # Errors
	///
	/// Returns [`CoreError`] when the Actor is not authorized or the store
	/// cannot answer.
	pub fn query(
		&self,
		actor: &Actor,
		query: Query,
	) -> Result<QueryResult, CoreError> {
		// Every authenticated local Actor may read Plane status.
		match actor {
			Actor::InteractiveClient { .. } => {}
		}
		match query {
			Query::Status => {
				let plane = self.store.plane()?;
				Ok(QueryResult::Status(PlaneStatus {
					plane_id: PlaneId(plane.plane_id),
					daemon_starts: plane.daemon_starts,
					started_at: self.started_at,
					core_version: CORE_VERSION,
				}))
			}
		}
	}
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
