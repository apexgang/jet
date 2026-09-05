use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{
	NewPairingClaim, NewPairingOffer, PairingInvalidation, PairingKeyAlgorithm,
	PairingMethod, PairingOfferRecord, PairingOfferState,
};
use crate::Store;

const NOW_UNIX_MS: i64 = 1_700_000_000_000;
const EXPIRES_UNIX_MS: i64 = NOW_UNIX_MS + 120_000;

fn offer(method: PairingMethod) -> NewPairingOffer {
	NewPairingOffer {
		offer_id: Uuid::now_v7(),
		method,
		secret_salt: [7; 16],
		secret_digest: [9; 32],
		opened_by: Uuid::now_v7(),
		opened_at_unix_ms: NOW_UNIX_MS,
		expires_at_unix_ms: EXPIRES_UNIX_MS,
	}
}

fn claim() -> NewPairingClaim {
	NewPairingClaim {
		client_id: Uuid::now_v7(),
		key_algorithm: PairingKeyAlgorithm::Ed25519,
		public_key: [3; 32],
		challenge: [4; 32],
		authentication_string: "418-273".into(),
	}
}

fn recorded(
	offer: &NewPairingOffer,
	state: PairingOfferState,
	claim: Option<NewPairingClaim>,
) -> PairingOfferRecord {
	PairingOfferRecord {
		offer_id: offer.offer_id,
		method: offer.method.clone(),
		secret_salt: offer.secret_salt,
		secret_digest: offer.secret_digest,
		state,
		invalidation: None,
		failed_attempts: 0,
		opened_by: offer.opened_by,
		opened_at_unix_ms: offer.opened_at_unix_ms,
		expires_at_unix_ms: offer.expires_at_unix_ms,
		claim,
	}
}

#[tokio::test]
async fn opening_an_offer_replaces_the_one_the_plane_had_open() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let first = offer(PairingMethod::ManualCode);
	let second = offer(PairingMethod::QrPayload {
		endpoint: "alex@studio.example".into(),
	});

	let none_yet = store
		.read(async |tx| tx.pairing_offer().await)
		.await
		.unwrap();
	store
		.write(async |tx| tx.replace_pairing_offer(first.clone()).await)
		.await
		.unwrap();
	store
		.write(async |tx| tx.replace_pairing_offer(second.clone()).await)
		.await
		.unwrap();
	let open = store
		.read(async |tx| tx.pairing_offer().await)
		.await
		.unwrap();

	assert_eq!(
		(none_yet, open),
		(
			None,
			Some(recorded(&second, PairingOfferState::Offered, None))
		)
	);
}

#[tokio::test]
async fn a_claim_and_its_failed_attempts_are_recorded_against_the_offer() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let offered = offer(PairingMethod::ManualCode);
	let claim = claim();

	store
		.write(async |tx| tx.replace_pairing_offer(offered.clone()).await)
		.await
		.unwrap();
	let attempts = store
		.write(async |tx| {
			tx.record_failed_pairing_attempt().await?;
			tx.record_failed_pairing_attempt().await
		})
		.await
		.unwrap();
	store
		.write(async |tx| {
			tx.record_pairing_claim(&claim, EXPIRES_UNIX_MS + 120_000)
				.await
		})
		.await
		.unwrap();
	let claimed = store
		.read(async |tx| tx.pairing_offer().await)
		.await
		.unwrap();

	assert_eq!(
		(attempts, claimed),
		(
			2,
			Some(PairingOfferRecord {
				failed_attempts: 2,
				expires_at_unix_ms: EXPIRES_UNIX_MS + 120_000,
				..recorded(
					&offered,
					PairingOfferState::AwaitingConfirmation,
					Some(claim)
				)
			})
		)
	);
}

#[tokio::test]
async fn an_invalidated_offer_keeps_the_reason_it_died_of() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let offered = offer(PairingMethod::ManualCode);

	store
		.write(async |tx| tx.replace_pairing_offer(offered.clone()).await)
		.await
		.unwrap();
	store
		.write(async |tx| {
			tx.invalidate_pairing_offer(PairingInvalidation::TooManyAttempts)
				.await
		})
		.await
		.unwrap();
	store
		.write(async |tx| {
			tx.invalidate_pairing_offer(PairingInvalidation::GateClosed)
				.await
		})
		.await
		.unwrap();
	let dead = store
		.read(async |tx| tx.pairing_offer().await)
		.await
		.unwrap();

	assert_eq!(
		dead,
		Some(PairingOfferRecord {
			invalidation: Some(PairingInvalidation::TooManyAttempts),
			..recorded(&offered, PairingOfferState::Invalidated, None)
		})
	);
}
