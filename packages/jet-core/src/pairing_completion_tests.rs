use std::time::{Duration, UNIX_EPOCH};

use ed25519_dalek::{Signer, SigningKey};
use pretty_assertions::assert_eq;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::pairing_secret::{authentication_string, transcript};
use crate::test_support::{
	FixedProbe, ManualClock, actor, equipped, request, start_core_with,
};
use crate::{
	Actor, AuditOutcome, AuditRisk, AuditSequence, AuthenticationString,
	ClientId, ClientPublicKey, Command, CommandOutcome, Core, CoreError,
	ErrorCategory, EventKind, EventSequence, PairedClient, PairedClientAccess,
	PairingChallenge, PairingDisclosure, PairingGate, PairingKeyAlgorithm,
	PairingMethod, PairingOfferId, PairingProgress, PairingSecret,
	PairingSignature, PairingSnapshot, PendingPairing, PlaneId, Query,
	QueryResult,
};

/// A fixed instant, so a pairing has an exact window.
const NOW: Duration = Duration::from_millis(1_700_000_000_000);

/// The window ADR-0017 gives each step of a Pairing.
const WINDOW: Duration = Duration::from_secs(120);

/// The Client identity being Paired, which is not the one that opened the
/// offer.
fn pairing_client() -> Actor {
	Actor::InteractiveClient {
		client_id: ClientId(Uuid::from_u128(7)),
	}
}

/// The installation's durable identity. Its private half never leaves the
/// installation; a test holds one so it can sign as the client does.
fn signing_key() -> SigningKey {
	SigningKey::from_bytes(&[3; 32])
}

fn public_key() -> ClientPublicKey {
	ClientPublicKey {
		algorithm: PairingKeyAlgorithm::Ed25519,
		key: signing_key().verifying_key().to_bytes(),
	}
}

async fn start(
	dir: &tempfile::TempDir,
	clock: &std::sync::Arc<ManualClock>,
) -> Core {
	let core = start_core_with(
		&dir.path().join("plane.sqlite3"),
		clock.clone(),
		FixedProbe::new(equipped()),
	)
	.await;
	core.execute(
		&actor(),
		request(Command::SetPairingGate {
			gate: PairingGate::Open,
		}),
	)
	.await
	.unwrap();
	core
}

async fn plane_id(core: &Core) -> PlaneId {
	let result = core.query(&actor(), Query::Status).await.unwrap();
	let QueryResult::Status(status) = result else {
		panic!("unexpected result {result:?}");
	};
	status.plane_id
}

/// Opens an offer and claims it, returning what a client would then hold:
/// the offer and the challenge its key has to sign.
async fn claimed(core: &Core) -> (PendingPairing, PairingChallenge) {
	let outcome = core
		.execute(
			&actor(),
			request(Command::OpenPairing {
				method: PairingMethod::ManualCode,
			}),
		)
		.await
		.unwrap();
	let CommandOutcome::PairingOpened { disclosure, .. } = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	let PairingDisclosure::ManualCode { code } = disclosure else {
		panic!("unexpected disclosure {disclosure:?}");
	};
	let outcome = core
		.execute(
			&pairing_client(),
			request(Command::ClaimPairing {
				secret: PairingSecret(code),
				key: public_key(),
			}),
		)
		.await
		.unwrap();
	let CommandOutcome::PairingClaimed { pending, challenge } = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	(pending, challenge)
}

fn displayed(pending: &PendingPairing) -> AuthenticationString {
	match &pending.progress {
		PairingProgress::AwaitingConfirmation {
			authentication_string,
			..
		}
		| PairingProgress::Confirmed {
			authentication_string,
			..
		} => authentication_string.clone(),
		progress => panic!("unexpected progress {progress:?}"),
	}
}

async fn confirm(
	core: &Core,
	confirming: &Actor,
	offer_id: PairingOfferId,
	string: AuthenticationString,
) -> Result<CommandOutcome, CoreError> {
	core.execute(
		confirming,
		request(Command::ConfirmPairing {
			offer_id,
			authentication_string: string,
		}),
	)
	.await
}

/// The signature the client makes over the transcript of its own claim.
async fn signature(
	core: &Core,
	offer_id: PairingOfferId,
	challenge: PairingChallenge,
) -> PairingSignature {
	let transcript = transcript(
		plane_id(core).await.0,
		offer_id.0,
		Uuid::from_u128(7),
		&public_key(),
		&challenge,
	);
	PairingSignature(signing_key().sign(&transcript).to_bytes())
}

async fn complete(
	core: &Core,
	completing: &Actor,
	offer_id: PairingOfferId,
	signature: PairingSignature,
) -> Result<CommandOutcome, CoreError> {
	core.execute(
		completing,
		request(Command::CompletePairing {
			offer_id,
			signature,
		}),
	)
	.await
}

async fn pairing(core: &Core) -> PairingSnapshot {
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

/// The transcript a client signs, and the string both sides show, are a
/// contract with every GUI client: they build the same bytes or the
/// pairing does not complete. Pinned here so a change to either is a
/// change somebody has to make on purpose.
#[test]
fn the_transcript_and_its_string_are_what_clients_reproduce() {
	let key = ClientPublicKey {
		algorithm: PairingKeyAlgorithm::Ed25519,
		key: [4; 32],
	};

	let transcript = transcript(
		Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111),
		Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222),
		Uuid::from_u128(0x3333_3333_3333_3333_3333_3333_3333_3333),
		&key,
		&PairingChallenge([5; 32]),
	);

	assert_eq!(
		(
			transcript.len(),
			format!("{:x}", Sha256::digest(&transcript)),
			authentication_string(&transcript)
		),
		(
			146,
			"dc70abc4d5b3bc7d14625cb74c2164862ac8f3149064f6e57b1dd6bd232dab06"
				.to_owned(),
			AuthenticationString("760-318".into())
		)
	);
}

/// The whole of one pairing: a person confirms on the target that both
/// screens show the same string, the client signs the challenge, and the
/// Plane is left holding its public key (ADR-0017, ADR-0090).
#[tokio::test]
async fn a_pairing_completes_and_leaves_the_plane_holding_the_key() {
	let dir = tempfile::tempdir().unwrap();
	let clock = ManualClock::at(UNIX_EPOCH + NOW);
	let core = start(&dir, &clock).await;
	let (claim, challenge) = claimed(&core).await;

	let confirmed = confirm(&core, &actor(), claim.offer_id, displayed(&claim))
		.await
		.unwrap();
	let signature = signature(&core, claim.offer_id, challenge).await;
	let completed =
		complete(&core, &pairing_client(), claim.offer_id, signature)
			.await
			.unwrap();
	let after = pairing(&core).await;

	let paired = PairedClient {
		client_id: ClientId(Uuid::from_u128(7)),
		key: public_key(),
		pairing_protocol: "jet.pairing.v1".into(),
		access: PairedClientAccess::Enabled,
		paired_at: UNIX_EPOCH + NOW,
	};
	assert_eq!(
		(confirmed, completed, after),
		(
			CommandOutcome::PairingConfirmed {
				pending: PendingPairing {
					progress: PairingProgress::Confirmed {
						client_id: ClientId(Uuid::from_u128(7)),
						authentication_string: displayed(&claim),
					},
					expires_at: UNIX_EPOCH + NOW + WINDOW,
					..claim.clone()
				},
			},
			CommandOutcome::PairingCompleted {
				client: paired.clone(),
			},
			PairingSnapshot {
				cursor: EventSequence(5),
				gate: PairingGate::Open,
				pending: None,
				clients: vec![paired],
			}
		)
	);
}

/// Mutual confirmation is the step a client that answered the code from
/// somewhere else cannot pass, so it is not the claiming client's to make
/// (ADR-0017).
#[tokio::test]
async fn the_client_being_paired_cannot_confirm_its_own_pairing() {
	let dir = tempfile::tempdir().unwrap();
	let clock = ManualClock::at(UNIX_EPOCH + NOW);
	let core = start(&dir, &clock).await;
	let (claim, _) = claimed(&core).await;

	let refused =
		confirm(&core, &pairing_client(), claim.offer_id, displayed(&claim))
			.await
			.unwrap_err();

	assert_eq!(
		(
			refused.category,
			refused.code.as_str(),
			pairing(&core).await.pending.map(|pending| pending.progress)
		),
		(
			ErrorCategory::Conflict,
			"pairing.confirmation_by_claimant",
			Some(PairingProgress::AwaitingConfirmation {
				client_id: ClientId(Uuid::from_u128(7)),
				authentication_string: displayed(&claim),
			})
		)
	);
}

/// A string that is not the one on the screen is a failed proof like any
/// other, so it costs the offer an attempt; a Pairing nobody has confirmed
/// cannot be completed either (ADR-0017).
#[tokio::test]
async fn a_wrong_string_and_an_unconfirmed_pairing_are_both_refused() {
	let dir = tempfile::tempdir().unwrap();
	let clock = ManualClock::at(UNIX_EPOCH + NOW);
	let core = start(&dir, &clock).await;
	let (claim, challenge) = claimed(&core).await;

	let wrong_string = confirm(
		&core,
		&actor(),
		claim.offer_id,
		AuthenticationString("000-000".into()),
	)
	.await
	.unwrap_err();
	let signature = signature(&core, claim.offer_id, challenge).await;
	let too_early =
		complete(&core, &pairing_client(), claim.offer_id, signature)
			.await
			.unwrap_err();

	assert_eq!(
		(
			(wrong_string.category, wrong_string.code.as_str()),
			(too_early.category, too_early.code.as_str()),
			pairing(&core)
				.await
				.pending
				.map(|pending| pending.attempts_remaining)
		),
		(
			(
				ErrorCategory::InvalidInput,
				"pairing.authentication_string_mismatch"
			),
			(ErrorCategory::Conflict, "pairing.not_confirmed"),
			Some(4)
		)
	);
}

/// Completing proves the client holds the identity it presented, so only
/// that client can do it and only its own key's signature verifies.
#[tokio::test]
async fn only_the_paired_client_with_its_own_key_completes_the_pairing() {
	let dir = tempfile::tempdir().unwrap();
	let clock = ManualClock::at(UNIX_EPOCH + NOW);
	let core = start(&dir, &clock).await;
	let (claim, challenge) = claimed(&core).await;
	confirm(&core, &actor(), claim.offer_id, displayed(&claim))
		.await
		.unwrap();
	let signature = signature(&core, claim.offer_id, challenge).await;

	let by_another = complete(&core, &actor(), claim.offer_id, signature)
		.await
		.unwrap_err();
	let another_key = PairingSignature(
		SigningKey::from_bytes(&[8; 32])
			.sign(&transcript(
				plane_id(&core).await.0,
				claim.offer_id.0,
				Uuid::from_u128(7),
				&public_key(),
				&challenge,
			))
			.to_bytes(),
	);
	let forged =
		complete(&core, &pairing_client(), claim.offer_id, another_key)
			.await
			.unwrap_err();

	assert_eq!(
		(
			(by_another.category, by_another.code.as_str()),
			(forged.category, forged.code.as_str()),
			pairing(&core)
				.await
				.pending
				.map(|pending| pending.attempts_remaining),
			pairing(&core).await.clients
		),
		(
			(ErrorCategory::Conflict, "pairing.completion_by_other"),
			(ErrorCategory::InvalidInput, "pairing.signature_rejected"),
			Some(4),
			vec![]
		)
	);
}

/// Confirming has its own window: a pairing nobody looked at stops being
/// confirmable two minutes after it was claimed.
#[tokio::test]
async fn a_pairing_nobody_confirms_stops_being_confirmable() {
	let dir = tempfile::tempdir().unwrap();
	let clock = ManualClock::at(UNIX_EPOCH + NOW);
	let core = start(&dir, &clock).await;
	let (claim, _) = claimed(&core).await;

	clock.advance(WINDOW + Duration::from_millis(1));
	let too_late = confirm(&core, &actor(), claim.offer_id, displayed(&claim))
		.await
		.unwrap_err();

	assert_eq!(
		(too_late.category, too_late.code.as_str()),
		(ErrorCategory::Conflict, "pairing.offer_expired")
	);
}

/// The journal says the pairing was confirmed and completed; the Security
/// audit says who decided each step, and keeps the Paired client as what
/// the last one was about (ADR-0105).
#[tokio::test]
async fn confirmation_and_completion_are_journaled_and_recorded() {
	let dir = tempfile::tempdir().unwrap();
	let clock = ManualClock::at(UNIX_EPOCH + NOW);
	let core = start(&dir, &clock).await;
	let (claim, challenge) = claimed(&core).await;

	confirm(&core, &actor(), claim.offer_id, displayed(&claim))
		.await
		.unwrap();
	let signature = signature(&core, claim.offer_id, challenge).await;
	complete(&core, &pairing_client(), claim.offer_id, signature)
		.await
		.unwrap();

	let elevated = |decision: &str| {
		(
			decision.to_owned(),
			AuditRisk::Elevated,
			AuditOutcome::Succeeded,
		)
	};
	assert_eq!(
		(
			events(&core).await.split_off(3),
			decisions(&core).await.split_off(3)
		),
		(
			vec![
				EventKind::PairingConfirmed {
					offer_id: claim.offer_id,
					client_id: ClientId(Uuid::from_u128(7)),
				},
				EventKind::PairingCompleted {
					offer_id: claim.offer_id,
					client_id: ClientId(Uuid::from_u128(7)),
				},
			],
			vec![elevated("pairing.confirmed"), elevated("pairing.completed"),]
		)
	);
}
