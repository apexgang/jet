//! Plane status snapshot.

use std::time::SystemTime;

use crate::{EventSequence, PlaneId};

/// Point-in-time view of the daemon and the Plane it owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneStatus {
	/// Newest Event sequence visible when the status was read.
	pub cursor: EventSequence,
	/// The Plane's durable identity.
	pub plane_id: PlaneId,
	/// Authoritative daemon starts recorded for this Plane, including the
	/// current one.
	pub daemon_starts: u64,
	/// When the current daemon started.
	pub started_at: SystemTime,
	/// Version of the running core.
	pub core_version: &'static str,
}
