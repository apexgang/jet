use std::time::{Duration, UNIX_EPOCH};

use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::test_support::{
	FixedProbe, ManualClock, actor, equipped, request, request_with_id,
	start_core, start_core_with,
};
use crate::{
	Actor, AuditOutcome, AuditRisk, AuditSequence, ClientId, ClientPublicKey,
	Command, CommandId, CommandOutcome, Core, CoreError, ErrorCategory,
	EventKind, EventSequence, PairingDisclosure, PairingEnd, PairingGate,
	PairingKeyAlgorithm, PairingMethod, PairingProgress, PairingSecret,
	PendingPairing, Query, QueryResult,
};

/// A fixed instant, so an offer has an exact window rather than whatever
/// the machine's clock said.
const NOW: Duration = Duration::from_millis(1_700_000_000_000);

/// The window ADR-0017 gives each step of a Pairing.
const WINDOW: Duration = Duration::from_secs(120);

/// The endpoint an owner advertises in a QR payload.
const ENDPOINT: &str = "alex@studio.example";

async fn start(dir: &tempfile::TempDir) -> Core {
	start_core(&dir.path().join("plane.sqlite3")).await
}

/// A core whose clock a test moves by hand, with the Pairing gate already
/// open.
async fn start_at(
	dir: &tempfile::TempDir,
	clock: &std::sync::Arc<ManualClock>,
) -> Core {
	let core = start_core_with(
		&dir.path().join("plane.sqlite3"),
		clock.clone(),
		FixedProbe::new(equipped()),
	)
	.await;
	open_gate(&core).await;
	core
}

/// The GUI client being paired, which is not the client that opened the
/// offer.
fn pairing_client() -> Actor {
	Actor::InteractiveClient {
		client_id: ClientId(Uuid::from_u128(7)),
	}
}

fn key() -> ClientPublicKey {
	ClientPublicKey {
		algorithm: PairingKeyAlgorithm::Ed25519,
		key: [4; 32],
	}
}

async fn open_gate(core: &Core) {
	core.execute(
		&actor(),
		request(Command::SetPairingGate {
			gate: PairingGate::Open,
		}),
	)
	.await
	.unwrap();
}

async fn open(
	core: &Core,
	method: PairingMethod,
) -> Result<(PendingPairing, PairingDisclosure), CoreError> {
	open_as(core, method, crate::test_support::command_id()).await
}

async fn open_as(
	core: &Core,
	method: PairingMethod,
	command_id: CommandId,
) -> Result<(PendingPairing, PairingDisclosure), CoreError> {
	let outcome = core
		.execute(
			&actor(),
			request_with_id(command_id, Command::OpenPairing { method }),
		)
		.await?;
	let CommandOutcome::PairingOpened {
		pending,
		disclosure,
	} = outcome
	else {
		panic!("unexpected outcome {outcome:?}");
	};
	Ok((pending, disclosure))
}

async fn claim(core: &Core, secret: &str) -> Result<CommandOutcome, CoreError> {
	core.execute(
		&pairing_client(),
		request(Command::ClaimPairing {
			secret: PairingSecret(secret.into()),
			key: key(),
		}),
	)
	.await
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

fn manual_code(disclosure: &PairingDisclosure) -> String {
	let PairingDisclosure::ManualCode { code } = disclosure else {
		panic!("unexpected disclosure {disclosure:?}");
	};
	code.clone()
}

fn qr_payload(disclosure: &PairingDisclosure) -> String {
	let PairingDisclosure::QrPayload { payload } = disclosure else {
		panic!("unexpected disclosure {disclosure:?}");
	};
	payload.clone()
}

/// The eight-digit numeric code ADR-0017 asks for, grouped so it can be
/// read out loud.
#[tokio::test]
async fn a_manual_code_is_eight_digits_a_person_can_read_out() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	open_gate(&core).await;

	let (_, disclosure) = open(&core, PairingMethod::ManualCode).await.unwrap();

	let code = manual_code(&disclosure);
	let (first, second) = code.split_once('-').expect("a grouped code");
	assert_eq!(
		(
			first.len(),
			second.len(),
			code.chars().filter(char::is_ascii_digit).count()
		),
		(4, 4, 8),
		"unexpected code shape {code:?}"
	);
}

/// The QR payload is versioned and carries the endpoint beside a 128-bit
/// one-time token (ADR-0017).
#[tokio::test]
async fn a_qr_payload_carries_the_endpoint_and_a_one_time_token() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	open_gate(&core).await;

	let (_, disclosure) = open(
		&core,
		PairingMethod::QrPayload {
			endpoint: ENDPOINT.into(),
		},
	)
	.await
	.unwrap();

	let payload = qr_payload(&disclosure);
	let [scheme, version, token, endpoint] =
		payload.splitn(4, ':').collect::<Vec<_>>()[..]
	else {
		panic!("unexpected payload {payload:?}");
	};
	assert_eq!(
		(
			scheme,
			version,
			token.len(),
			token.chars().all(|digit| digit.is_ascii_hexdigit()
				&& !digit.is_ascii_uppercase()),
			endpoint
		),
		("jet-pair", "1", 32, true, ENDPOINT)
	);
}

/// A Plane accepts no offer until its owner has opened the gate, and the
/// gate closing again ends the offer it left open (ADR-0017).
#[tokio::test]
async fn an_offer_needs_an_open_gate_and_ends_when_it_closes() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;

	let refused = open(&core, PairingMethod::ManualCode).await.unwrap_err();
	open_gate(&core).await;
	let (offered, disclosure) =
		open(&core, PairingMethod::ManualCode).await.unwrap();
	core.execute(
		&actor(),
		request(Command::SetPairingGate {
			gate: PairingGate::Closed,
		}),
	)
	.await
	.unwrap();
	let ended = pairing(&core).await;
	let too_late = claim(&core, &manual_code(&disclosure)).await.unwrap_err();

	assert_eq!(
		(
			(refused.category, refused.code.as_str()),
			ended.pending,
			(too_late.category, too_late.code.as_str())
		),
		(
			(ErrorCategory::Conflict, "pairing.gate_closed"),
			Some(PendingPairing {
				progress: PairingProgress::Ended {
					reason: PairingEnd::GateClosed,
				},
				..offered
			}),
			(ErrorCategory::Conflict, "pairing.offer_ended")
		)
	);
}

/// The secret is disclosed once. A retry of the Command that opened the
/// offer is answered with the offer, because the receipt that makes the
/// retry idempotent is durable and outlives the secret (ADR-0093).
#[tokio::test]
async fn the_secret_is_disclosed_once_and_a_retry_is_answered_without_it() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	open_gate(&core).await;
	let command_id = crate::test_support::command_id();

	let (offered, disclosure) =
		open_as(&core, PairingMethod::ManualCode, command_id)
			.await
			.unwrap();
	let (replayed, replayed_disclosure) =
		open_as(&core, PairingMethod::ManualCode, command_id)
			.await
			.unwrap();
	let claimed = claim(&core, &manual_code(&disclosure)).await;

	assert_eq!(
		(
			replayed,
			replayed_disclosure,
			claimed.is_ok(),
			pairing(&core).await.pending.map(|pending| pending.offer_id)
		),
		(
			offered.clone(),
			PairingDisclosure::AlreadyDisclosed,
			true,
			Some(offered.offer_id)
		)
	);
}

/// Claiming with the offer's secret leaves both sides an authentication
/// string to compare, and the offer waiting for the people at each end.
#[tokio::test]
async fn claiming_an_offer_leaves_both_sides_a_string_to_compare() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	open_gate(&core).await;
	let (_, disclosure) = open(&core, PairingMethod::ManualCode).await.unwrap();

	let outcome = claim(&core, &manual_code(&disclosure)).await.unwrap();

	let CommandOutcome::PairingClaimed { pending, challenge } = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	let PairingProgress::AwaitingConfirmation {
		client_id,
		authentication_string,
	} = pending.progress.clone()
	else {
		panic!("unexpected progress {:?}", pending.progress);
	};
	let digits = authentication_string.0.replace('-', "");
	assert_eq!(
		(
			client_id,
			digits.len(),
			digits.chars().all(|digit| digit.is_ascii_digit()),
			challenge.0 == [0; 32],
			pairing(&core).await.pending.as_ref(),
		),
		(ClientId(Uuid::from_u128(7)), 6, true, false, Some(&pending))
	);
}

/// The secret is single-use: the first claim spends it (ADR-0017).
#[tokio::test]
async fn a_claimed_offer_cannot_be_claimed_again() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	open_gate(&core).await;
	let (_, disclosure) = open(&core, PairingMethod::ManualCode).await.unwrap();
	let code = manual_code(&disclosure);

	claim(&core, &code).await.unwrap();
	let again = claim(&core, &code).await.unwrap_err();

	assert_eq!(
		(again.category, again.code.as_str()),
		(ErrorCategory::Conflict, "pairing.already_claimed")
	);
}

/// Five wrong secrets end the offer, and the right one is too late after
/// them (ADR-0017).
#[tokio::test]
async fn five_wrong_secrets_end_the_offer() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	open_gate(&core).await;
	let (_, disclosure) = open(&core, PairingMethod::ManualCode).await.unwrap();

	let mut remaining = Vec::new();
	for _ in 0..5 {
		let error = claim(&core, "0000-0000").await.unwrap_err();
		remaining.push((error.category, error.code));
	}
	let right_secret =
		claim(&core, &manual_code(&disclosure)).await.unwrap_err();

	assert_eq!(
		(
			remaining,
			pairing(&core)
				.await
				.pending
				.map(|pending| (pending.progress, pending.attempts_remaining)),
			(right_secret.category, right_secret.code.as_str())
		),
		(
			std::iter::repeat_n(
				(
					ErrorCategory::InvalidInput,
					"pairing.secret_rejected".to_owned()
				),
				5
			)
			.collect::<Vec<_>>(),
			Some((
				PairingProgress::Ended {
					reason: PairingEnd::TooManyAttempts,
				},
				0
			)),
			(ErrorCategory::Conflict, "pairing.offer_ended")
		)
	);
}

/// The secret stops being accepted two minutes after it was issued
/// (ADR-0017). Nothing is written when it does: an offer nobody used is
/// simply not accepted any more.
#[tokio::test]
async fn the_secret_stops_being_accepted_after_two_minutes() {
	let dir = tempfile::tempdir().unwrap();
	let clock = ManualClock::at(UNIX_EPOCH + NOW);
	let core = start_at(&dir, &clock).await;
	let (offered, disclosure) =
		open(&core, PairingMethod::ManualCode).await.unwrap();

	clock.advance(WINDOW + Duration::from_millis(1));
	let too_late = claim(&core, &manual_code(&disclosure)).await.unwrap_err();

	assert_eq!(
		(
			offered.expires_at,
			pairing(&core).await.pending.map(|pending| pending.progress),
			(too_late.category, too_late.code.as_str())
		),
		(
			UNIX_EPOCH + NOW + WINDOW,
			Some(PairingProgress::Ended {
				reason: PairingEnd::Expired,
			}),
			(ErrorCategory::Conflict, "pairing.offer_expired")
		)
	);
}

/// Confirming is done by people, so a claim gets its own window rather than
/// what is left of the secret's.
#[tokio::test]
async fn a_claim_gets_its_own_window_to_be_confirmed_in() {
	let dir = tempfile::tempdir().unwrap();
	let clock = ManualClock::at(UNIX_EPOCH + NOW);
	let core = start_at(&dir, &clock).await;
	let (_, disclosure) = open(&core, PairingMethod::ManualCode).await.unwrap();

	clock.advance(WINDOW - Duration::from_millis(1));
	let outcome = claim(&core, &manual_code(&disclosure)).await.unwrap();

	let CommandOutcome::PairingClaimed { pending, .. } = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	assert_eq!(
		pending.expires_at,
		UNIX_EPOCH + NOW + WINDOW + WINDOW - Duration::from_millis(1)
	);
}

/// An endpoint is the address a client reaches the Plane at, so the Plane
/// bounds it and refuses what no address looks like.
#[tokio::test]
async fn an_endpoint_that_is_not_an_address_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	open_gate(&core).await;

	let refused = open(
		&core,
		PairingMethod::QrPayload {
			endpoint: "alex@studio example".into(),
		},
	)
	.await
	.unwrap_err();

	assert_eq!(
		(
			refused.category,
			refused.code.as_str(),
			pairing(&core).await.pending
		),
		(
			ErrorCategory::InvalidInput,
			"pairing.endpoint_unsupported",
			None
		)
	);
}

/// The journal says a pairing was offered and claimed; the Security audit
/// says who decided it, and keeps the refusals as well (ADR-0105).
#[tokio::test]
async fn offers_and_the_secrets_presented_against_them_are_recorded() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	open_gate(&core).await;
	let (offered, disclosure) =
		open(&core, PairingMethod::ManualCode).await.unwrap();

	claim(&core, "0000-0000").await.unwrap_err();
	claim(&core, &manual_code(&disclosure)).await.unwrap();

	assert_eq!(
		(events(&core).await, decisions(&core).await),
		(
			vec![
				EventKind::PairingGateChanged {
					gate: PairingGate::Open,
				},
				EventKind::PairingOffered {
					offer_id: offered.offer_id,
					method: PairingMethod::ManualCode,
				},
				EventKind::PairingClaimed {
					offer_id: offered.offer_id,
					client_id: ClientId(Uuid::from_u128(7)),
				},
			],
			vec![
				(
					"pairing.gate_opened".into(),
					AuditRisk::Elevated,
					AuditOutcome::Succeeded,
				),
				(
					"pairing.offered".into(),
					AuditRisk::Elevated,
					AuditOutcome::Succeeded,
				),
				(
					"pairing.claimed".into(),
					AuditRisk::Elevated,
					AuditOutcome::Denied,
				),
				(
					"pairing.claimed".into(),
					AuditRisk::Elevated,
					AuditOutcome::Succeeded,
				),
			]
		)
	);
}
