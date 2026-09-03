use pretty_assertions::assert_eq;

use super::{PlaneRecord, Store};

#[test]
fn plane_identity_and_start_count_survive_reopening_the_store() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");

	let first = Store::open(&path).unwrap();
	let after_first_start = first.record_daemon_start().unwrap();
	drop(first);

	let second = Store::open(&path).unwrap();
	let after_second_start = second.record_daemon_start().unwrap();

	assert_eq!(after_first_start.daemon_starts, 1);
	assert_eq!(
		after_second_start,
		PlaneRecord {
			plane_id: after_first_start.plane_id,
			daemon_starts: 2,
		}
	);
	assert_eq!(second.plane().unwrap(), after_second_start);
}

#[test]
fn a_fresh_store_has_a_plane_that_never_started_a_daemon() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3")).unwrap();

	let plane = store.plane().unwrap();
	assert_eq!(plane.daemon_starts, 0);
	assert!(!plane.plane_id.is_nil());
}

#[test]
fn opening_a_store_in_a_missing_directory_is_reported_as_unavailable() {
	let dir = tempfile::tempdir().unwrap();
	let error = Store::open(&dir.path().join("missing").join("plane.sqlite3"))
		.unwrap_err();

	assert!(
		matches!(error, super::StoreError::Unavailable(_)),
		"{error:?}"
	);
}
