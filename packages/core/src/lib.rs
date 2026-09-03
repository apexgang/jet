//! Jet core: the deep Module behind `jetd` (ADR-0047).
//!
//! The core exposes three conceptual entry points: execute an authenticated
//! Command, run a Query returning a snapshot, and subscribe from an Event
//! journal cursor. This slice implements Commands and fenced Queries; live
//! subscriptions arrive with the Run execution work.
//!
//! Domain types here never double as wire types (ADR-0049); `jetd`
//! translates at the transport seam.

mod command;
mod conversation;
mod error;
mod event;
mod lifecycle;
mod query;
mod status;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jet_store::{ActorRecord, Store};
use uuid::Uuid;

pub use command::{Command, CommandOutcome};
pub use conversation::{
	Conversation, ConversationId, ConversationList, ConversationSnapshot, Run,
	RunId,
};
pub use error::{CoreError, ErrorCategory};
pub use event::{EVENT_PAGE_LIMIT, Event, EventId, EventKind, EventSequence};
pub use jet_store::{Retention, RunLifecycle};
pub use query::{Query, QueryResult};
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
}

/// Converts a stored wall-clock stamp back into a [`SystemTime`].
fn system_time(unix_ms: i64) -> SystemTime {
	UNIX_EPOCH + Duration::from_millis(u64::try_from(unix_ms).unwrap_or(0))
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
