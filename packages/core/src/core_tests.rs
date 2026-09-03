use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{
	Actor, CORE_VERSION, ClientId, Core, PlaneStatus, Query, QueryResult,
};
use jet_store::Store;

fn actor() -> Actor {
	Actor::InteractiveClient {
		client_id: ClientId(Uuid::new_v4()),
	}
}

#[test]
fn status_reports_the_persisted_plane_across_core_restarts() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");

	let first = Core::start(Store::open(&path).unwrap()).unwrap();
	let QueryResult::Status(before) =
		first.query(&actor(), Query::Status).unwrap();
	drop(first);

	let second = Core::start(Store::open(&path).unwrap()).unwrap();
	let QueryResult::Status(after) =
		second.query(&actor(), Query::Status).unwrap();

	assert_eq!(before.daemon_starts, 1);
	assert_eq!(
		after,
		PlaneStatus {
			plane_id: before.plane_id,
			daemon_starts: 2,
			started_at: after.started_at,
			core_version: CORE_VERSION,
		}
	);
	assert!(after.started_at >= before.started_at);
}
