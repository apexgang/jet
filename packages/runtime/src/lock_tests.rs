use pretty_assertions::assert_eq;

use super::{DaemonMetadata, InstallationChannel, LifetimeLock, LockError};
use crate::JetHome;

fn metadata(pid: u32) -> DaemonMetadata {
	DaemonMetadata {
		pid,
		version: "0.1.0".into(),
		channel: InstallationChannel::Development,
	}
}

#[test]
fn a_second_daemon_is_refused_and_told_who_owns_the_plane() {
	let dir = tempfile::tempdir().unwrap();
	let home = JetHome::at(dir.path().join(".jet"));
	home.prepare().unwrap();

	let _first = LifetimeLock::acquire(&home, &metadata(41)).unwrap();
	let error = LifetimeLock::acquire(&home, &metadata(42)).unwrap_err();

	assert_eq!(
		error,
		LockError::Held {
			owner: Some(metadata(41))
		}
	);
}

#[test]
fn stale_metadata_from_a_released_lock_does_not_establish_ownership() {
	let dir = tempfile::tempdir().unwrap();
	let home = JetHome::at(dir.path().join(".jet"));
	home.prepare().unwrap();

	let first = LifetimeLock::acquire(&home, &metadata(41)).unwrap();
	drop(first);
	let second = LifetimeLock::acquire(&home, &metadata(42));

	assert!(second.is_ok(), "{second:?}");
}
