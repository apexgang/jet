use pretty_assertions::assert_eq;

use super::PairingGate;
use crate::Store;

const NOW_UNIX_MS: i64 = 1_700_000_000_000;

#[tokio::test]
async fn the_gate_starts_closed_and_stays_where_the_owner_left_it() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");

	let store = Store::open(&path).await.unwrap();
	let new_plane = store
		.read(async |tx| tx.pairing_gate().await)
		.await
		.unwrap();
	store
		.write(async |tx| {
			tx.set_pairing_gate(PairingGate::Open, NOW_UNIX_MS).await
		})
		.await
		.unwrap();
	store.close().await;

	let reopened = Store::open(&path).await.unwrap();
	let after_restart = reopened
		.read(async |tx| tx.pairing_gate().await)
		.await
		.unwrap();

	assert_eq!(
		(new_plane, after_restart),
		(PairingGate::Closed, PairingGate::Open)
	);
}
