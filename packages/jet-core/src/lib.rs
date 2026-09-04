//! Jet core: the deep Module behind `jetd` (ADR-0047).
//!
//! The core exposes three conceptual entry points: execute an authenticated
//! Command, run a Query returning a snapshot, and subscribe from an Event
//! journal cursor. This slice implements Commands and fenced Queries; live
//! subscriptions arrive with the Run execution work.
//!
//! Domain types here never double as wire types (ADR-0049); `jetd`
//! translates at the transport seam.

mod clock;
mod command;
mod conversation;
mod error;
mod event;
mod lifecycle;
mod query;
mod status;
#[cfg(test)]
mod test_support;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jet_store::{ActorRecord, Store};
use uuid::Uuid;

use clock::{Clock, SystemClock};

pub use command::{Command, CommandEnvelope, CommandId, CommandOutcome};
pub use conversation::{
	Conversation, ConversationId, ConversationList, ConversationSnapshot,
	Revision, Run, RunId,
};
pub use error::{
	ConflictState, CoreError, ErrorCategory, RecoveryAction, RevisionConflict,
};
pub use event::{
	Event, EventId, EventKind, EventPage, EventPayload, EventSequence,
};
pub use jet_store::{RetentionPolicy, RunLifecycle};
pub use query::{Query, QueryResult};
pub use status::PlaneStatus;

/// Version of the running core, reported in status snapshots.
pub(crate) const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

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

impl Actor {
	/// Checks that the Actor may drive and read this Plane's Conversations.
	/// Every authenticated local Actor may in this slice; remote and
	/// automation Actors arrive with their own rules (ADR-0063).
	#[expect(
		clippy::unnecessary_wraps,
		reason = "the first non-local Actor turns this into a real check"
	)]
	fn authorize(&self) -> Result<(), CoreError> {
		match self {
			Self::InteractiveClient { .. } => Ok(()),
		}
	}

	fn record(&self) -> ActorRecord {
		match self {
			Self::InteractiveClient { client_id } => {
				ActorRecord::InteractiveClient {
					client_id: client_id.0,
				}
			}
		}
	}

	fn from_record(record: ActorRecord) -> Self {
		match record {
			ActorRecord::InteractiveClient { client_id } => {
				Self::InteractiveClient {
					client_id: ClientId(client_id),
				}
			}
		}
	}
}

/// One running core bound to one Plane store.
#[derive(Debug)]
pub struct Core {
	store: Store,
	clock: Arc<dyn Clock>,
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
		Self::start_with_clock(store, Arc::new(SystemClock))
	}

	/// Starts the core with an injected wall clock.
	///
	/// # Errors
	///
	/// Returns [`CoreError`] with an `unavailable` or `internal` category
	/// when the start cannot be committed.
	pub(crate) fn start_with_clock(
		store: Store,
		clock: Arc<dyn Clock>,
	) -> Result<Self, CoreError> {
		store.record_daemon_start()?;
		let started_at = clock.now();
		Ok(Self {
			store,
			clock,
			started_at,
		})
	}

	/// The core clock's current time as the store records it. Every stamp
	/// written by one Command comes from this one reading.
	pub(crate) fn now_unix_ms(&self) -> i64 {
		unix_ms(self.clock.now())
	}
}

/// Converts a stored wall-clock stamp back into a [`SystemTime`].
fn system_time(unix_ms: i64) -> SystemTime {
	UNIX_EPOCH + Duration::from_millis(u64::try_from(unix_ms).unwrap_or(0))
}

fn unix_ms(time: SystemTime) -> i64 {
	match time.duration_since(UNIX_EPOCH) {
		Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
		Err(behind) => i64::try_from(behind.duration().as_millis())
			.map_or(i64::MIN, |ms| -ms),
	}
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
