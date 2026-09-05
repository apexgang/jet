//! Black-box Pairing conformance tests at the public Jet protocol boundary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use ed25519_dalek::{Signer, SigningKey};
use jet_client::ClientError;
use jet_protocol::{
	ClientMessage, ClientPublicKey, ErrorCategory, PairedClient,
	PairedClientAccess, PairingDisclosure, PairingGate, PairingKeyAlgorithm,
	PairingMethod, PairingProgress, PairingSnapshot, QueryRequest,
	SECURITY_AUDIT_MINOR, ServerHello, ServerMessage,
};
use pretty_assertions::assert_eq;
use support::{connect, handshake_raw, hello, start_jetd};
use uuid::Uuid;

/// The gate is the Plane's own answer to whether it accepts new clients, so
/// it starts closed and survives the daemon that was told to open it
/// (ADR-0017).
#[tokio::test]
async fn the_gate_starts_closed_and_outlives_the_daemon_that_opened_it() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();

	let mut first = start_jetd(&home).await;
	let client = connect(&first, client_id).await;
	let untouched = client.pairing().await.unwrap();
	let opened = client
		.set_pairing_gate(Uuid::now_v7(), PairingGate::Open)
		.await
		.unwrap();
	first.child.kill().await.unwrap();

	let second = start_jetd(&home).await;
	let client = connect(&second, client_id).await;
	let after_restart = client.pairing().await.unwrap();

	assert_eq!(
		(untouched, opened, after_restart),
		(
			PairingSnapshot {
				cursor: 0,
				gate: PairingGate::Closed,
				pending: None,
				clients: vec![],
			},
			PairingGate::Open,
			PairingSnapshot {
				cursor: 1,
				gate: PairingGate::Open,
				pending: None,
				clients: vec![],
			}
		)
	);
}

/// The whole of one pairing as far as the people at each end: the owner
/// opens the gate, reads a code off the target, and the GUI client presents
/// it back and is given a string to compare (ADR-0017).
#[tokio::test]
async fn a_client_claims_an_offer_with_the_code_read_off_the_target() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let owner = connect(&daemon, Uuid::new_v4()).await;
	let pairing_client_id = Uuid::new_v4();
	let pairing_client = connect(&daemon, pairing_client_id).await;
	let key = ClientPublicKey {
		algorithm: PairingKeyAlgorithm::Ed25519,
		key: [9; 32],
	};

	owner
		.set_pairing_gate(Uuid::now_v7(), PairingGate::Open)
		.await
		.unwrap();
	let (offered, disclosure) = owner
		.open_pairing(Uuid::now_v7(), PairingMethod::ManualCode)
		.await
		.unwrap();
	let PairingDisclosure::ManualCode { code } = disclosure else {
		panic!("unexpected disclosure {disclosure:?}");
	};
	let wrong = pairing_client
		.claim_pairing(Uuid::now_v7(), "0000-0000", key)
		.await
		.unwrap_err();
	let (claimed, challenge) = pairing_client
		.claim_pairing(Uuid::now_v7(), &code, key)
		.await
		.unwrap();
	let seen_by_the_owner = owner.pairing().await.unwrap();

	let ClientError::Remote(wrong) = wrong else {
		panic!("expected a stable remote error, got {wrong:?}");
	};
	let PairingProgress::AwaitingConfirmation {
		client_id,
		authentication_string,
	} = claimed.progress.clone()
	else {
		panic!("unexpected progress {:?}", claimed.progress);
	};
	assert_eq!(
		(
			(wrong.category, wrong.code.as_str()),
			claimed.offer_id,
			client_id,
			authentication_string.len(),
			challenge == [0; 32],
			seen_by_the_owner.pending.as_ref(),
		),
		(
			(ErrorCategory::InvalidInput, "pairing.secret_rejected"),
			offered.offer_id,
			pairing_client_id,
			7,
			false,
			Some(&claimed)
		)
	);
}

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
async fn pair(
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

/// One whole Pairing over the protocol: the client proves it holds the
/// identity it presented, and the Plane keeps its public key across the
/// daemon that recorded it (ADR-0017, ADR-0090).
#[tokio::test]
async fn a_completed_pairing_outlives_the_daemon_that_recorded_it() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();
	let signing = SigningKey::from_bytes(&[5; 32]);

	let mut first = start_jetd(&home).await;
	let owner = connect(&first, Uuid::new_v4()).await;
	let pairing_client = connect(&first, client_id).await;
	let paired = pair(&owner, &pairing_client, client_id, &signing).await;
	first.child.kill().await.unwrap();

	let second = start_jetd(&home).await;
	let after_restart =
		connect(&second, client_id).await.pairing().await.unwrap();

	assert_eq!(
		(&paired, after_restart.pending, after_restart.clients),
		(
			&PairedClient {
				client_id,
				key: ClientPublicKey {
					algorithm: PairingKeyAlgorithm::Ed25519,
					key: signing.verifying_key().to_bytes(),
				},
				pairing_protocol: "jet.pairing.v1".into(),
				access: PairedClientAccess::Enabled,
				paired_at_unix_ms: paired.paired_at_unix_ms,
			},
			None,
			vec![paired.clone()]
		)
	);
}

/// Disabling and revoking are different durable decisions: one keeps the
/// key the Plane holds for a client, the other forgets it (ADR-0017).
#[tokio::test]
async fn disabling_keeps_a_client_the_plane_can_enable_and_revoking_does_not() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client_id = Uuid::new_v4();
	let owner = connect(&daemon, Uuid::new_v4()).await;
	let pairing_client = connect(&daemon, client_id).await;
	let paired = pair(
		&owner,
		&pairing_client,
		client_id,
		&SigningKey::from_bytes(&[6; 32]),
	)
	.await;

	let disabled = owner
		.set_paired_client_access(
			Uuid::now_v7(),
			client_id,
			PairedClientAccess::Disabled,
		)
		.await
		.unwrap();
	let revoked = owner
		.revoke_paired_client(Uuid::now_v7(), client_id)
		.await
		.unwrap();

	assert_eq!(
		(disabled, revoked, owner.pairing().await.unwrap().clients),
		(
			PairedClient {
				access: PairedClientAccess::Disabled,
				..paired
			},
			client_id,
			vec![]
		)
	);
}

/// A client that negotiated a minor without Pairing is answered with a
/// stable refusal rather than a guess (ADR-0019, ADR-0094).
#[tokio::test]
async fn a_client_below_the_pairing_minor_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut older = hello(Uuid::new_v4());
	older.minor = SECURITY_AUDIT_MINOR;

	let (mut connection, welcome) = handshake_raw(&daemon, &older).await;
	assert!(
		matches!(welcome, ServerHello::Welcome { minor, .. }
			if minor == SECURITY_AUDIT_MINOR),
		"expected a welcome at the older minor, got {welcome:?}"
	);
	connection
		.send(&ClientMessage::Query {
			id: 1,
			query: QueryRequest::Pairing,
		})
		.await;

	let ServerMessage::Error { id, error } = connection.receive().await else {
		panic!("expected a refusal");
	};
	assert_eq!(
		(
			id,
			error.category,
			error.code.as_str(),
			error.message.as_str()
		),
		(
			Some(1),
			ErrorCategory::Incompatible,
			"protocol.unsupported_minor",
			"the Pairing Query needs protocol minor 6"
		)
	);
}
