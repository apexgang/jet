use pretty_assertions::assert_eq;

use super::{CORE_VERSION, PlaneStatus, Query, QueryResult};
use crate::test_support::{actor, start_core};

#[test]
fn status_reports_the_persisted_plane_across_core_restarts() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");

	let first = start_core(&path);
	let QueryResult::Status(before) =
		first.query(&actor(), Query::Status).unwrap()
	else {
		panic!("expected a status snapshot");
	};
	drop(first);

	let second = start_core(&path);
	let QueryResult::Status(after) =
		second.query(&actor(), Query::Status).unwrap()
	else {
		panic!("expected a status snapshot");
	};

	assert_eq!(
		(&before, &after),
		(
			&PlaneStatus {
				cursor: crate::EventSequence(0),
				plane_id: after.plane_id,
				daemon_starts: 1,
				started_at: before.started_at,
				core_version: CORE_VERSION,
			},
			&PlaneStatus {
				cursor: crate::EventSequence(0),
				plane_id: after.plane_id,
				daemon_starts: 2,
				started_at: after.started_at,
				core_version: CORE_VERSION,
			}
		)
	);
	assert!(after.started_at >= before.started_at);
}
