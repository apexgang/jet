//! Pairing through the public protocol, shared by connection tests.

use ed25519_dalek::{Signer, SigningKey};
use jet_protocol::{
	ClientPublicKey, PairedClient, PairingDisclosure, PairingGate,
	PairingKeyAlgorithm, PairingMethod, PairingProgress,
};
use uuid::Uuid;

/// What a GUI client signs to complete a Pairing, built here the way a
/// client that has only the protocol has to build it: from the Plane, the
/// offer, its own identity, the key it presented, and the challenge it was
/// answered with (ADR-0017).
fn transcript(
	plane_id: Uuid,
	offer_id: Uuid,
	client_id: Uuid,
	key: &ClientPublicKey,
	challenge: &[u8; 32],
) -> Vec<u8> {
	let mut transcript = Vec::new();
	transcript.extend_from_slice(b"jet.pairing.transcript.v1");
	transcript.push(0);
	transcript.extend_from_slice(plane_id.as_bytes());
	transcript.extend_from_slice(offer_id.as_bytes());
	transcript.extend_from_slice(client_id.as_bytes());
	transcript.extend_from_slice(b"ed25519");
	transcript.push(0);
	transcript.extend_from_slice(&key.key);
	transcript.extend_from_slice(challenge);
	transcript
}

/// Pairs `pairing_client` with the Plane `owner` is connected to, the way
/// the two people at each end would: a code read off the target, an
/// authentication string compared, and a challenge signed.
pub async fn pair(
	owner: &jet_client::Client,
	pairing_client: &jet_client::Client,
	client_id: Uuid,
	signing: &SigningKey,
) -> PairedClient {
	let key = ClientPublicKey {
		algorithm: PairingKeyAlgorithm::Ed25519,
		key: signing.verifying_key().to_bytes(),
	};
	owner
		.set_pairing_gate(Uuid::now_v7(), PairingGate::Open)
		.await
		.unwrap();
	let (_, disclosure) = owner
		.open_pairing(Uuid::now_v7(), PairingMethod::ManualCode)
		.await
		.unwrap();
	let PairingDisclosure::ManualCode { code } = disclosure else {
		panic!("unexpected disclosure {disclosure:?}");
	};
	let (claimed, challenge) = pairing_client
		.claim_pairing(Uuid::now_v7(), &code, key)
		.await
		.unwrap();
	let PairingProgress::AwaitingConfirmation {
		authentication_string,
		..
	} = claimed.progress.clone()
	else {
		panic!("unexpected progress {:?}", claimed.progress);
	};
	owner
		.confirm_pairing(
			Uuid::now_v7(),
			claimed.offer_id,
			&authentication_string,
		)
		.await
		.unwrap();
	let plane_id = owner.status().await.unwrap().plane_id;
	let signature = signing
		.sign(&transcript(
			plane_id,
			claimed.offer_id,
			client_id,
			&key,
			&challenge,
		))
		.to_bytes();
	pairing_client
		.complete_pairing(Uuid::now_v7(), claimed.offer_id, signature)
		.await
		.unwrap()
}
