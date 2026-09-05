use std::time::{Duration, UNIX_EPOCH};

use ed25519_dalek::{Signer, SigningKey};
use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::pairing_secret::transcript;
use crate::test_support::{
	FixedProbe, ManualClock, actor, equipped, request, start_core_with,
};
use crate::{
	Actor, AuditOutcome, AuditRisk, AuditSequence, ClientId, ClientPublicKey,
	Command, CommandOutcome, Core, CoreError, ErrorCategory, EventKind,
	EventSequence, PairedClient, PairedClientAccess, PairingDisclosure,
	PairingGate, PairingKeyAlgorithm, PairingMethod, PairingProgress,
	PairingSecret, PairingSignature, Query, QueryResult,
};

/// A fixed instant, so a Paired client has an exact time.
const NOW: Duration = Duration::from_millis(1_700_000_000_000);

fn pairing_client() -> Actor {
	Actor::InteractiveClient {
		client_id: ClientId(Uuid::from_u128(7)),
	}
}

fn signing_key() -> SigningKey {
	SigningKey::from_bytes(&[3; 32])
}

fn public_key() -> ClientPublicKey {
	ClientPublicKey {
		algorithm: PairingKeyAlgorithm::Ed25519,
		key: signing_key().verifying_key().to_bytes(),
	}
}

/// A core that has already Paired with one client, which is what these
/// decisions are about.
async fn start_paired(dir: &tempfile::TempDir) -> Core {
	let core = start_core_with(
		&dir.path().join("plane.sqlite3"),
		ManualClock::at(UNIX_EPOCH + NOW),
		FixedProbe::new(equipped()),
	)
	.await;
	execute(
		&core,
		&actor(),
		Command::SetPairingGate {
			gate: PairingGate::Open,
		},
	)
	.await
	.unwrap();
	let opened = execute(
		&core,
		&actor(),
		Command::OpenPairing {
			method: PairingMethod::ManualCode,
		},
	)
	.await
	.unwrap();
	let CommandOutcome::PairingOpened {
		pending,
		disclosure: PairingDisclosure::ManualCode { code },
	} = opened
	else {
		panic!("unexpected outcome {opened:?}");
	};
	let claimed = execute(
		&core,
		&pairing_client(),
		Command::ClaimPairing {
			secret: PairingSecret(code),
			key: public_key(),
		},
	)
	.await
	.unwrap();
	let CommandOutcome::PairingClaimed { challenge, .. } = claimed else {
		panic!("unexpected outcome {claimed:?}");
	};
	let PairingProgress::AwaitingConfirmation {
		authentication_string,
		..
	} = pairing(&core).await.pending.unwrap().progress
	else {
		panic!("the offer was not claimed");
	};
	execute(
		&core,
		&actor(),
		Command::ConfirmPairing {
			offer_id: pending.offer_id,
			authentication_string,
		},
	)
	.await
	.unwrap();
	let plane_id = match core.query(&actor(), Query::Status).await.unwrap() {
		QueryResult::Status(status) => status.plane_id,
		result => panic!("unexpected result {result:?}"),
	};
	let signature = PairingSignature(
		signing_key()
			.sign(&transcript(
				plane_id.0,
				pending.offer_id.0,
				Uuid::from_u128(7),
				&public_key(),
				&challenge,
			))
			.to_bytes(),
	);
	execute(
		&core,
		&pairing_client(),
		Command::CompletePairing {
			offer_id: pending.offer_id,
			signature,
		},
	)
	.await
	.unwrap();
	core
}

#[tokio::test]
async fn revocation_forces_a_no_visa_operation_to_stop_within_a_bound() {
	use std::process::Stdio;
	use tokio::io::{AsyncBufReadExt, BufReader};
	let dir = tempfile::tempdir().unwrap();
	let core = start_paired(&dir).await;
	let transcript = b"connection supplied by the trusted transport";
	let remote = core
		.authenticate_remote(
			pairing_client().client_id(),
			transcript,
			PairingSignature(signing_key().sign(transcript).to_bytes()),
		)
		.await
		.unwrap();
	let Actor::RemoteClient { session } = &remote else {
		panic!("not remote");
	};
	let mut command = tokio::process::Command::new("sh");
	command
		.args([
			"-c",
			"trap '' TERM; printf 'ready\\n'; while :; do sleep 1; done",
		])
		.stdout(Stdio::piped());
	let mut operation =
		core.spawn_no_visa(session, &mut command).await.unwrap();
	let mut output = BufReader::new(operation.take_stdout().unwrap()).lines();
	assert_eq!(output.next_line().await.unwrap().as_deref(), Some("ready"));
	execute(
		&core,
		&actor(),
		Command::RevokePairedClient {
			client_id: pairing_client().client_id(),
		},
	)
	.await
	.unwrap();
	let stopped =
		tokio::time::timeout(Duration::from_secs(4), operation.wait())
			.await
			.unwrap()
			.unwrap();
	assert!(!stopped.success());
	assert!(core.query(&remote, Query::Status).await.is_err());
	assert!(core.spawn_no_visa(session, &mut command).await.is_err());
}

#[tokio::test]
async fn a_postcommit_audit_head_failure_still_revokes_live_authority() {
	let dir = tempfile::tempdir().unwrap();
	let core = start_paired(&dir).await;
	let transcript = b"connection supplied by the trusted transport";
	let remote = core
		.authenticate_remote(
			pairing_client().client_id(),
			transcript,
			PairingSignature(signing_key().sign(transcript).to_bytes()),
		)
		.await
		.unwrap();
	let head = jet_store::audit_head_path(&dir.path().join("plane.sqlite3"));
	let mut pending = head.into_os_string();
	pending.push(".pending");
	std::fs::create_dir(&pending).unwrap();
	let result = execute(
		&core,
		&actor(),
		Command::RevokePairedClient {
			client_id: pairing_client().client_id(),
		},
	)
	.await;
	assert!(result.is_err());
	std::fs::remove_dir(&pending).unwrap();
	assert_eq!(pairing(&core).await.clients, vec![]);
	assert_eq!(
		core.query(&remote, Query::Status)
			.await
			.unwrap_err()
			.category,
		ErrorCategory::Unauthorized
	);
	let Actor::RemoteClient { session } = remote else {
		panic!("not remote");
	};
	tokio::time::timeout(Duration::from_millis(100), session.revoked())
		.await
		.unwrap();
}

async fn execute(
	core: &Core,
	actor: &Actor,
	command: Command,
) -> Result<CommandOutcome, CoreError> {
	core.execute(actor, request(command)).await
}

async fn pairing(core: &Core) -> crate::PairingSnapshot {
	let result = core.query(&actor(), Query::Pairing).await.unwrap();
	let QueryResult::Pairing(snapshot) = result else {
		panic!("unexpected result {result:?}");
	};
	snapshot
}

async fn events(core: &Core) -> Vec<EventKind> {
	let result = core
		.query(
			&actor(),
			Query::Events {
				after: EventSequence(0),
			},
		)
		.await
		.unwrap();
	let QueryResult::Events(page) = result else {
		panic!("unexpected result {result:?}");
	};
	page.events.into_iter().map(|event| event.kind).collect()
}

async fn decisions(core: &Core) -> Vec<(String, AuditRisk, AuditOutcome)> {
	let result = core
		.query(
			&actor(),
			Query::SecurityAudit {
				after: AuditSequence(0),
			},
		)
		.await
		.unwrap();
	let QueryResult::SecurityAudit(page) = result else {
		panic!("unexpected result {result:?}");
	};
	page.entries
		.into_iter()
		.map(|entry| (entry.decision, entry.risk, entry.outcome))
		.collect()
}

fn paired(access: PairedClientAccess) -> PairedClient {
	PairedClient {
		client_id: ClientId(Uuid::from_u128(7)),
		key: public_key(),
		pairing_protocol: "jet.pairing.v1".into(),
		access,
		paired_at: UNIX_EPOCH + NOW,
	}
}

/// Disabling stops a client controlling the Plane and keeps its key, so
/// enabling it again needs nobody in front of either machine (ADR-0017).
#[tokio::test]
async fn a_disabled_client_keeps_its_key_and_can_be_enabled_again() {
	let dir = tempfile::tempdir().unwrap();
	let core = start_paired(&dir).await;

	let disabled = execute(
		&core,
		&actor(),
		Command::SetPairedClientAccess {
			client_id: ClientId(Uuid::from_u128(7)),
			access: PairedClientAccess::Disabled,
		},
	)
	.await
	.unwrap();
	let while_disabled = pairing(&core).await.clients;
	let enabled = execute(
		&core,
		&actor(),
		Command::SetPairedClientAccess {
			client_id: ClientId(Uuid::from_u128(7)),
			access: PairedClientAccess::Enabled,
		},
	)
	.await
	.unwrap();

	assert_eq!(
		(
			disabled,
			while_disabled,
			enabled,
			pairing(&core).await.clients
		),
		(
			CommandOutcome::PairedClientAccessSet {
				client: paired(PairedClientAccess::Disabled),
			},
			vec![paired(PairedClientAccess::Disabled)],
			CommandOutcome::PairedClientAccessSet {
				client: paired(PairedClientAccess::Enabled),
			},
			vec![paired(PairedClientAccess::Enabled)]
		)
	);
}

/// Revoking forgets the key, so the installation has to be Paired again
/// (ADR-0017).
#[tokio::test]
async fn revoking_forgets_the_client_and_its_key() {
	let dir = tempfile::tempdir().unwrap();
	let core = start_paired(&dir).await;

	let revoked = execute(
		&core,
		&actor(),
		Command::RevokePairedClient {
			client_id: ClientId(Uuid::from_u128(7)),
		},
	)
	.await
	.unwrap();
	let again = execute(
		&core,
		&actor(),
		Command::RevokePairedClient {
			client_id: ClientId(Uuid::from_u128(7)),
		},
	)
	.await
	.unwrap_err();

	assert_eq!(
		(
			revoked,
			pairing(&core).await.clients,
			(again.category, again.code.as_str())
		),
		(
			CommandOutcome::PairedClientRevoked {
				client_id: ClientId(Uuid::from_u128(7)),
			},
			vec![],
			(ErrorCategory::NotFound, "pairing.client_not_found")
		)
	);
}

/// The journal says what happened to the client; the audit says who decided
/// it and what it cost, and keeps saying which client it was after the
/// Plane has stopped keeping one (ADR-0105).
#[tokio::test]
async fn stopping_and_forgetting_a_client_are_recorded_as_decisions() {
	let dir = tempfile::tempdir().unwrap();
	let core = start_paired(&dir).await;

	execute(
		&core,
		&actor(),
		Command::SetPairedClientAccess {
			client_id: ClientId(Uuid::from_u128(7)),
			access: PairedClientAccess::Disabled,
		},
	)
	.await
	.unwrap();
	execute(
		&core,
		&actor(),
		Command::RevokePairedClient {
			client_id: ClientId(Uuid::from_u128(7)),
		},
	)
	.await
	.unwrap();

	assert_eq!(
		(
			events(&core).await.split_off(5),
			decisions(&core).await.split_off(5)
		),
		(
			vec![
				EventKind::PairedClientAccessChanged {
					client_id: ClientId(Uuid::from_u128(7)),
					access: PairedClientAccess::Disabled,
				},
				EventKind::PairedClientRevoked {
					client_id: ClientId(Uuid::from_u128(7)),
				},
			],
			vec![
				(
					"pairing.client_disabled".into(),
					AuditRisk::Routine,
					AuditOutcome::Succeeded,
				),
				(
					"pairing.client_revoked".into(),
					AuditRisk::Destructive,
					AuditOutcome::Succeeded,
				),
			]
		)
	);
}
