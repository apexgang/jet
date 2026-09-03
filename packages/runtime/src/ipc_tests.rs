use std::os::unix::fs::PermissionsExt;

use pretty_assertions::assert_eq;
use tokio::net::UnixStream;

use super::LocalListener;
use crate::JetHome;

#[tokio::test]
async fn the_socket_and_runtime_dir_are_owner_only_and_accept_the_owner() {
	let dir = tempfile::tempdir().unwrap();
	let home = JetHome::at(dir.path().join(".jet"));
	home.prepare().unwrap();

	let listener = LocalListener::bind(&home).unwrap();
	let socket_mode = std::fs::metadata(home.socket_path())
		.unwrap()
		.permissions()
		.mode()
		& 0o777;
	let runtime_mode = std::fs::metadata(home.runtime_dir())
		.unwrap()
		.permissions()
		.mode()
		& 0o777;

	let client = UnixStream::connect(home.socket_path());
	let (client, accepted) = tokio::join!(client, listener.accept());
	client.unwrap();
	accepted.unwrap();

	assert_eq!((socket_mode, runtime_mode), (0o600, 0o700));
}

#[tokio::test]
async fn binding_replaces_a_stale_socket_left_by_a_dead_daemon() {
	let dir = tempfile::tempdir().unwrap();
	let home = JetHome::at(dir.path().join(".jet"));
	home.prepare().unwrap();

	let stale = LocalListener::bind(&home).unwrap();
	std::mem::forget(stale);
	let fresh = LocalListener::bind(&home).unwrap();

	let client = UnixStream::connect(home.socket_path());
	let (client, accepted) = tokio::join!(client, fresh.accept());
	client.unwrap();
	assert!(accepted.is_ok());
}
