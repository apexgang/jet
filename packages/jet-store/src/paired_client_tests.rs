use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{NewPairedClient, PairedClientAccess, PairedClientRecord};
use crate::Store;
use crate::pairing_offer::PairingKeyAlgorithm;

const NOW_UNIX_MS: i64 = 1_700_000_000_000;

fn paired(client_id: Uuid, key: u8, at: i64) -> NewPairedClient {
	NewPairedClient {
		client_id,
		key_algorithm: PairingKeyAlgorithm::Ed25519,
		public_key: [key; 32],
		pairing_protocol: "jet.pairing.v1".into(),
		paired_at_unix_ms: at,
	}
}

fn recorded(client: &NewPairedClient) -> PairedClientRecord {
	PairedClientRecord {
		client_id: client.client_id,
		key_algorithm: client.key_algorithm,
		public_key: client.public_key,
		pairing_protocol: client.pairing_protocol.clone(),
		access: PairedClientAccess::Enabled,
		paired_at_unix_ms: client.paired_at_unix_ms,
	}
}

#[tokio::test]
async fn pairing_again_replaces_the_key_the_plane_held_for_that_client() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let returning = Uuid::now_v7();
	let other = Uuid::now_v7();
	let first = paired(returning, 1, NOW_UNIX_MS);
	let again = paired(returning, 2, NOW_UNIX_MS + 60_000);
	let beside = paired(other, 3, NOW_UNIX_MS + 30_000);

	let store = Store::open(&path).await.unwrap();
	store
		.write(async |tx| {
			tx.upsert_paired_client(first).await?;
			tx.upsert_paired_client(beside.clone()).await
		})
		.await
		.unwrap();
	store
		.write(async |tx| tx.upsert_paired_client(again.clone()).await)
		.await
		.unwrap();
	store.close().await;

	let reopened = Store::open(&path).await.unwrap();
	let clients = reopened
		.read(async |tx| tx.paired_clients().await)
		.await
		.unwrap();

	assert_eq!(clients, vec![recorded(&beside), recorded(&again)]);
}
