//! Remote authorization through the real stdio bridge and Jet protocol.

mod support;

use ed25519_dalek::{Signer, SigningKey};
use jet_protocol::{
	Frame, FrameReader, FrameWriter, PREFACE, decode_control, encode_control,
};
use pretty_assertions::assert_eq;
use std::process::Stdio;
use support::{Daemon, hello, start_jetd};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use uuid::Uuid;

fn proof(
	client_id: Uuid,
	nonce: &str,
	signing: &SigningKey,
) -> serde_json::Value {
	let mut transcript = b"jet.connection.v1\0ed25519\0".to_vec();
	transcript.extend(encode_control(&hello(client_id)).unwrap());
	for index in (0..nonce.len()).step_by(2) {
		transcript
			.push(u8::from_str_radix(&nonce[index..index + 2], 16).unwrap());
	}
	let signature: String = signing
		.sign(&transcript)
		.to_bytes()
		.iter()
		.map(|b| format!("{b:02x}"))
		.collect();
	serde_json::json!({"signature":signature})
}

struct Remote {
	_child: Child,
	reader: FrameReader<ChildStdout>,
	writer: FrameWriter<ChildStdin>,
}

struct Identity(Uuid, SigningKey);
impl jet_client::ClientIdentity for Identity {
	fn client_id(&self) -> Uuid {
		self.0
	}
	async fn sign(&self, transcript: &[u8]) -> std::io::Result<[u8; 64]> {
		Ok(self.1.sign(transcript).to_bytes())
	}
}

#[tokio::test]
async fn the_rust_remote_client_reads_the_same_plane_as_the_local_client() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let daemon = start_jetd(&home).await;
	let identity = Identity(Uuid::new_v4(), SigningKey::from_bytes(&[7; 32]));
	let owner = support::connect(&daemon, Uuid::new_v4()).await;
	let local = support::connect(&daemon, identity.0).await;
	support::pairing::pair(&owner, &local, identity.0, &identity.1).await;
	let mut child = Command::new(env!("CARGO_BIN_EXE_jetd"))
		.args(["connect", "--stdio", "--home"])
		.arg(&home)
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.kill_on_drop(true)
		.spawn()
		.unwrap();
	let remote = jet_client::Client::connect_remote(
		child.stdout.take().unwrap(),
		child.stdin.take().unwrap(),
		&identity,
	)
	.await
	.unwrap();
	assert_eq!(
		remote.status().await.unwrap(),
		local.status().await.unwrap()
	);
}

impl Remote {
	async fn open(daemon: &Daemon, client_id: Uuid) -> Self {
		Self::open_hello(daemon, &hello(client_id)).await
	}

	async fn open_hello(
		daemon: &Daemon,
		hello: &jet_protocol::ClientHello,
	) -> Self {
		let mut child = Command::new(env!("CARGO_BIN_EXE_jetd"))
			.args(["connect", "--stdio", "--home"])
			.arg(daemon.socket.parent().unwrap().parent().unwrap())
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::inherit())
			.kill_on_drop(true)
			.spawn()
			.unwrap();
		let mut input = child.stdin.take().unwrap();
		input.write_all(PREFACE).await.unwrap();
		let mut remote = Self {
			reader: FrameReader::new(child.stdout.take().unwrap()),
			writer: FrameWriter::new(input),
			_child: child,
		};
		remote.send(hello).await;
		remote
	}

	async fn send(&mut self, message: &impl serde::Serialize) {
		self.writer
			.write(&Frame::control(encode_control(message).unwrap()))
			.await
			.unwrap();
	}

	async fn receive(&mut self) -> serde_json::Value {
		let Frame::Control { payload, .. } = self.reader.read().await.unwrap()
		else {
			panic!("expected control frame");
		};
		decode_control(&payload).unwrap()
	}
}

#[tokio::test]
async fn ssh_endpoint_access_does_not_expose_plane_state() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut remote = Remote::open(&daemon, Uuid::new_v4()).await;
	assert_eq!(remote.receive().await["kind"], "challenge");
	remote
		.send(&jet_protocol::ClientMessage::Query {
			id: 1,
			query: jet_protocol::QueryRequest::Status,
		})
		.await;
	assert!(
		!matches!(remote.reader.read().await, Ok(Frame::Control { payload, .. })
        if decode_control::<serde_json::Value>(&payload).unwrap()["kind"] == "query_result")
	);
}

#[tokio::test]
async fn unknown_wrong_disabled_and_revoked_keys_are_refused_identically() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let id = Uuid::new_v4();
	let key = SigningKey::from_bytes(&[9; 32]);
	let owner = support::connect(&daemon, Uuid::new_v4()).await;
	let local = support::connect(&daemon, id).await;
	let mut refusals = Vec::new();
	for stage in 0..4 {
		if stage == 1 {
			support::pairing::pair(&owner, &local, id, &key).await;
		}
		if stage == 2 {
			owner
				.set_paired_client_access(
					Uuid::now_v7(),
					id,
					jet_protocol::PairedClientAccess::Disabled,
				)
				.await
				.unwrap();
		}
		if stage == 3 {
			owner
				.revoke_paired_client(Uuid::now_v7(), id)
				.await
				.unwrap();
		}
		let mut remote = Remote::open(&daemon, id).await;
		let nonce = remote.receive().await;
		let wrong = SigningKey::from_bytes(&[3; 32]);
		remote
			.send(&proof(
				id,
				nonce["nonce"].as_str().unwrap(),
				if stage == 1 { &wrong } else { &key },
			))
			.await;
		refusals.push(remote.receive().await);
	}
	assert_eq!(refusals, vec![refusals[0].clone(); 4]);
	assert_eq!(refusals[0]["error"]["code"], "connection.unauthorized");
}

#[tokio::test]
async fn legacy_handshakes_cannot_downgrade_remote_authentication() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut old = hello(Uuid::new_v4());
	old.minor = 6;
	let mut remote = Remote::open_hello(&daemon, &old).await;
	assert_eq!(
		remote.receive().await["error"]["code"],
		"protocol.remote_auth_required"
	);
}

#[tokio::test]
async fn a_new_remote_installation_pairs_without_local_client_authority() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let owner = support::connect(&daemon, Uuid::new_v4()).await;
	owner
		.set_pairing_gate(Uuid::now_v7(), jet_protocol::PairingGate::Open)
		.await
		.unwrap();
	let (_, disclosure) = owner
		.open_pairing(Uuid::now_v7(), jet_protocol::PairingMethod::ManualCode)
		.await
		.unwrap();
	let jet_protocol::PairingDisclosure::ManualCode { code } = disclosure
	else {
		panic!("not manual");
	};
	let identity = Identity(Uuid::new_v4(), SigningKey::from_bytes(&[5; 32]));
	let mut claim = Remote::open(&daemon, identity.0).await;
	assert_eq!(claim.receive().await["kind"], "challenge");
	claim
		.send(&jet_protocol::RemotePairingRequest::Claim {
			command_id: Uuid::now_v7(),
			secret: code,
			key: jet_protocol::ClientPublicKey {
				algorithm: jet_protocol::PairingKeyAlgorithm::Ed25519,
				key: identity.1.verifying_key().to_bytes(),
			},
		})
		.await;
	let claimed = claim.receive().await;
	assert_eq!(claimed["kind"], "claimed");
	let offer_id =
		Uuid::parse_str(claimed["pending"]["offer_id"].as_str().unwrap())
			.unwrap();
	let authentication_string =
		claimed["pending"]["progress"]["authentication_string"]
			.as_str()
			.unwrap();
	owner
		.confirm_pairing(Uuid::now_v7(), offer_id, authentication_string)
		.await
		.unwrap();
	let signing_bytes: Vec<u8> =
		serde_json::from_value(claimed["signing_bytes"].clone()).unwrap();
	let signature = identity.1.sign(&signing_bytes).to_bytes();
	let mut complete = Command::new(env!("CARGO_BIN_EXE_jetd"))
		.args(["connect", "--stdio", "--home"])
		.arg(daemon.socket.parent().unwrap().parent().unwrap())
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.kill_on_drop(true)
		.spawn()
		.unwrap();
	let completed = jet_client::Client::pair_remote(
		complete.stdout.take().unwrap(),
		complete.stdin.take().unwrap(),
		identity.0,
		&jet_protocol::RemotePairingRequest::Complete {
			command_id: Uuid::now_v7(),
			offer_id,
			signature,
		},
	)
	.await
	.unwrap();
	assert!(matches!(
		completed,
		jet_protocol::RemotePairingResponse::Completed { .. }
	));
	let mut login = Remote::open(&daemon, identity.0).await;
	let challenge = login.receive().await;
	login
		.send(&proof(
			identity.0,
			challenge["nonce"].as_str().unwrap(),
			&identity.1,
		))
		.await;
	assert_eq!(login.receive().await["kind"], "welcome");
}

#[tokio::test]
async fn revocation_closes_all_connections_and_preserves_hosted_runs() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let id = Uuid::new_v4();
	let key = SigningKey::from_bytes(&[9; 32]);
	let owner = support::connect(&daemon, Uuid::new_v4()).await;
	let local = support::connect(&daemon, id).await;
	support::pairing::pair(&owner, &local, id, &key).await;
	let conversation = owner
		.create_conversation(
			Uuid::now_v7(),
			jet_protocol::RetentionPolicy::Retain,
		)
		.await
		.unwrap();
	let run = owner
		.create_run(Uuid::now_v7(), conversation.conversation_id)
		.await
		.unwrap();
	let mut remotes = Vec::new();
	for _ in 0..2 {
		let mut remote = Remote::open(&daemon, id).await;
		let challenge = remote.receive().await;
		remote
			.send(&proof(id, challenge["nonce"].as_str().unwrap(), &key))
			.await;
		assert_eq!(remote.receive().await["kind"], "welcome");
		remotes.push(remote);
	}
	owner
		.revoke_paired_client(Uuid::now_v7(), id)
		.await
		.unwrap();
	for mut remote in remotes {
		assert!(
			tokio::time::timeout(
				std::time::Duration::from_secs(2),
				remote.reader.read()
			)
			.await
			.unwrap()
			.is_err()
		);
	}
	assert_eq!(
		owner
			.conversation(conversation.conversation_id)
			.await
			.unwrap()
			.runs,
		vec![run]
	);
}

#[tokio::test]
async fn a_paired_signature_authenticates_the_same_protocol_over_stdio() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let id = Uuid::new_v4();
	let key = SigningKey::from_bytes(&[9; 32]);
	let owner = support::connect(&daemon, Uuid::new_v4()).await;
	let local = support::connect(&daemon, id).await;
	support::pairing::pair(&owner, &local, id, &key).await;
	let mut remote = Remote::open(&daemon, id).await;
	let challenge = remote.receive().await;
	remote
		.send(&proof(id, challenge["nonce"].as_str().unwrap(), &key))
		.await;
	assert_eq!(remote.receive().await["kind"], "welcome");
}

#[tokio::test]
async fn disabling_a_paired_client_closes_an_idle_remote_connection() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let id = Uuid::new_v4();
	let key = SigningKey::from_bytes(&[9; 32]);
	let owner = support::connect(&daemon, Uuid::new_v4()).await;
	let local = support::connect(&daemon, id).await;
	support::pairing::pair(&owner, &local, id, &key).await;
	let mut remote = Remote::open(&daemon, id).await;
	let challenge = remote.receive().await;
	remote
		.send(&proof(id, challenge["nonce"].as_str().unwrap(), &key))
		.await;
	assert_eq!(remote.receive().await["kind"], "welcome");
	owner
		.set_paired_client_access(
			Uuid::now_v7(),
			id,
			jet_protocol::PairedClientAccess::Disabled,
		)
		.await
		.unwrap();
	let closed = tokio::time::timeout(
		std::time::Duration::from_secs(2),
		remote.reader.read(),
	)
	.await;
	assert!(
		matches!(closed, Ok(Err(_))),
		"disabled connection stayed open: {closed:?}"
	);
	assert!(
		local.status().await.is_ok(),
		"local ownership is independent of Pairing"
	);
}

#[tokio::test]
async fn reconnecting_rejects_replayed_signatures_and_records_the_denial() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let id = Uuid::new_v4();
	let key = SigningKey::from_bytes(&[9; 32]);
	let owner = support::connect(&daemon, Uuid::new_v4()).await;
	let local = support::connect(&daemon, id).await;
	support::pairing::pair(&owner, &local, id, &key).await;
	let mut first = Remote::open(&daemon, id).await;
	let challenge = first.receive().await;
	let signature = proof(id, challenge["nonce"].as_str().unwrap(), &key);
	first.send(&signature).await;
	assert_eq!(first.receive().await["kind"], "welcome");
	let mut second = Remote::open(&daemon, id).await;
	assert_ne!(second.receive().await["nonce"], challenge["nonce"]);
	second.send(&signature).await;
	let rejected = second.receive().await;
	assert_eq!(
		(&rejected["kind"], &rejected["error"]["code"]),
		(
			&serde_json::json!("rejected"),
			&serde_json::json!("connection.unauthorized")
		)
	);
	let audit = owner.security_audit_after(0).await.unwrap();
	assert!(
		audit
			.entries
			.iter()
			.any(|entry| entry.decision == "connection.authenticated"
				&& entry.outcome == jet_protocol::AuditOutcome::Denied)
	);
}
