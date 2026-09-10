//! Completing a Pairing: the mutual confirmation and the signed challenge
//! (ADR-0017, ADR-0090).
//!
//! Two things have to happen before a client controls a Plane, and they
//! happen at opposite ends. The person at the target confirms that the
//! authentication string on both screens is the same one, which is what
//! rules out a client that answered the code from somewhere else. The
//! client then signs the transcript of its own claim, which is what proves
//! that the key it presented is a key it holds. Neither is the other's to
//! do: the claiming client cannot confirm itself, and nobody else can
//! complete on its behalf.

use crate::{
	Actor, ClientId, PlaneId,
	audit::{self, AuditDecision, AuditSubject, Decision},
	command::CommandOutcome,
	error::CoreError,
	event::{EventKind, EventSubject},
	pairing::{
		self, AuthenticationString, ClientPublicKey, PAIRING_PROTOCOL,
		PAIRING_WINDOW_MS, PairingChallenge, PairingOfferId, PairingProgress,
		PairingSignature, identity as pairing_identity, offer as pairing_offer,
		secret,
	},
};
use jet_store::{NewPairedClient, WriteTransaction};

/// Confirms, on the target, that both screens show the same authentication
/// string.
///
/// The claiming client cannot do this for itself. Mutual confirmation is
/// the step that a client which answered the code from somewhere else
/// cannot pass, and a client allowed to confirm its own claim would be that
/// client passing it (ADR-0017).
///
/// # Errors
///
/// Returns a `not_found` [`CoreError`] when no offer is open, a `conflict`
/// one when the named offer is not the open one, is not waiting to be
/// confirmed, or is being confirmed by the client it would Pair, an
/// `invalid_input` one when the string does not match, and a store category
/// when the confirmation cannot be written.
pub(crate) async fn confirm(
	tx: &mut WriteTransaction,
	actor: &Actor,
	offer_id: PairingOfferId,
	presented: AuthenticationString,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let record = named_offer(tx, offer_id).await?;
	let progress = pairing::pending(&record, now_unix_ms).progress;
	let PairingProgress::AwaitingConfirmation {
		client_id,
		authentication_string,
	} = progress
	else {
		return Err(wrong_progress(&progress));
	};
	if client_id == actor.client_id() {
		return Err(CoreError::conflict(
			"pairing.confirmation_by_claimant",
			"the client being Paired cannot confirm its own Pairing; \
			 confirm it on the Plane being Paired with",
		));
	}
	if !secret::same_authentication_string(&authentication_string, &presented) {
		let remaining = pairing_offer::count_failure(
			tx,
			actor,
			offer_id,
			AuditDecision::PairingConfirmed,
			now_unix_ms,
		)
		.await?;
		// An authoritative refusal, so the attempt it counted commits.
		return Err(CoreError::invalid_input(
			"pairing.authentication_string_mismatch",
			format!(
				"that is not the authentication string this Pairing is \
				 showing; {remaining} attempts remain"
			),
		));
	}
	// Signing is done by a machine, but only once a person has looked at
	// two screens, so the client gets its own window to prove its key in.
	let expires_at_unix_ms = now_unix_ms.saturating_add(PAIRING_WINDOW_MS);
	tx.record_pairing_confirmation(actor.client_id().0, expires_at_unix_ms)
		.await?;
	tx.append_event(
		EventKind::PairingConfirmed {
			offer_id,
			client_id,
		}
		.to_record(actor, EventSubject::Plane, now_unix_ms)?,
	)
	.await?;
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::PairingConfirmed,
			AuditSubject::PairingOffer(offer_id),
		),
		now_unix_ms,
	)
	.await?;
	let confirmed = jet_store::PairingOfferRecord {
		confirmed_by: Some(actor.client_id().0),
		expires_at_unix_ms,
		..record
	};
	Ok(CommandOutcome::PairingConfirmed {
		pending: pairing::pending(&confirmed, now_unix_ms),
	})
}

/// Completes the Pairing with a signature over the claim's transcript,
/// leaving the Plane holding the client's durable public key.
///
/// A signature that does not verify is counted against the offer exactly as
/// a wrong secret is: both are something presenting itself as the client
/// being Paired and failing to show it.
///
/// # Errors
///
/// Returns a `not_found` [`CoreError`] when no offer is open, a `conflict`
/// one when the named offer is not the open one, is not confirmed, or is
/// being completed by another client, an `invalid_input` one when the
/// signature does not verify, and a store category when the Paired client
/// cannot be written.
pub(crate) async fn complete(
	tx: &mut WriteTransaction,
	actor: &Actor,
	offer_id: PairingOfferId,
	signature: PairingSignature,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let record = named_offer(tx, offer_id).await?;
	let progress = pairing::pending(&record, now_unix_ms).progress;
	let PairingProgress::Confirmed { client_id, .. } = progress else {
		return Err(wrong_progress(&progress));
	};
	if client_id != actor.client_id() {
		return Err(CoreError::conflict(
			"pairing.completion_by_other",
			"only the client this Pairing is for can complete it",
		));
	}
	let Some(claim) = record.claim.clone() else {
		return Err(CoreError::internal(
			"pairing.claim_missing",
			"a confirmed offer holds no claim",
		));
	};
	let key = ClientPublicKey {
		algorithm: claim.key_algorithm,
		key: claim.public_key,
	};
	let plane_id = PlaneId(tx.plane().await?.plane_id);
	let transcript = secret::transcript(
		plane_id.0,
		record.offer_id,
		claim.client_id,
		&key,
		&PairingChallenge(claim.challenge),
	);
	if !pairing_identity::verifies(&key, &transcript, &signature) {
		let remaining = pairing_offer::count_failure(
			tx,
			actor,
			offer_id,
			AuditDecision::PairingCompleted,
			now_unix_ms,
		)
		.await?;
		// An authoritative refusal, so the attempt it counted commits.
		return Err(CoreError::invalid_input(
			"pairing.signature_rejected",
			format!(
				"that signature is not this Pairing's challenge signed by \
				 the key it presented; {remaining} attempts remain"
			),
		));
	}
	let client = pairing::paired_client(
		tx.upsert_paired_client(NewPairedClient {
			client_id: claim.client_id,
			key_algorithm: claim.key_algorithm,
			public_key: claim.public_key,
			pairing_protocol: PAIRING_PROTOCOL.into(),
			paired_at_unix_ms: now_unix_ms,
		})
		.await?,
	);
	// What the offer established is the Paired client it leaves behind, so
	// the offer itself is gone and its challenge cannot be signed twice.
	tx.delete_pairing_offer().await?;
	tx.append_event(
		EventKind::PairingCompleted {
			offer_id,
			client_id: ClientId(claim.client_id),
		}
		.to_record(actor, EventSubject::Plane, now_unix_ms)?,
	)
	.await?;
	// ASVS 16.2.1: this is the record of a client gaining full-trust
	// control of the Plane, kept against the client rather than the offer,
	// because the offer is over and the client is what is left (ADR-0105).
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::PairingCompleted,
			AuditSubject::PairedClient(client.client_id),
		),
		now_unix_ms,
	)
	.await?;
	Ok(CommandOutcome::PairingCompleted { client })
}

/// The offer the Command names, if it is the one the Plane has open.
async fn named_offer(
	tx: &mut WriteTransaction,
	offer_id: PairingOfferId,
) -> Result<jet_store::PairingOfferRecord, CoreError> {
	let Some(record) = tx.pairing_offer().await? else {
		return Err(CoreError::not_found(
			"pairing.none_offered",
			"this Plane has no Pairing offer open",
		));
	};
	if record.offer_id != offer_id.0 {
		return Err(CoreError::conflict(
			"pairing.offer_superseded",
			"that Pairing offer is not the one this Plane has open",
		));
	}
	Ok(record)
}

/// Why the offer cannot take the step being asked of it, said from where
/// it actually is. Confirming and completing are consecutive steps, so each
/// refuses everything the other is for.
fn wrong_progress(progress: &PairingProgress) -> CoreError {
	match progress {
		PairingProgress::Offered => CoreError::conflict(
			"pairing.not_claimed",
			"nobody has presented this Pairing offer's secret yet",
		),
		PairingProgress::AwaitingConfirmation { .. } => CoreError::conflict(
			"pairing.not_confirmed",
			"nobody has confirmed this Pairing on the Plane being Paired \
			 with yet",
		),
		PairingProgress::Confirmed { .. } => CoreError::conflict(
			"pairing.already_confirmed",
			"this Pairing was already confirmed",
		),
		PairingProgress::Ended { reason } => ended(*reason),
	}
}

fn ended(reason: crate::pairing::PairingEnd) -> CoreError {
	CoreError::conflict(
		pairing_offer::ended_code(reason),
		format!("{}; open another one", pairing_offer::ended_message(reason)),
	)
}

#[cfg(test)]
pub(crate) mod tests {
	use std::time::{Duration, UNIX_EPOCH};

	use ed25519_dalek::{Signer, SigningKey};
	use pretty_assertions::assert_eq;
	use sha2::{Digest as _, Sha256};
	use uuid::Uuid;

	use crate::pairing::secret::{authentication_string, transcript};
	use crate::test_support::{
		FixedProbe, ManualClock, actor, equipped, request, start_core_with,
	};
	use crate::{
		Actor, AuditOutcome, AuditRisk, AuditSequence, AuthenticationString,
		ClientId, ClientPublicKey, Command, CommandOutcome, Core, CoreError,
		ErrorCategory, EventKind, EventSequence, PairedClient,
		PairedClientAccess, PairingChallenge, PairingDisclosure, PairingGate,
		PairingKeyAlgorithm, PairingMethod, PairingOfferId, PairingProgress,
		PairingSecret, PairingSignature, PairingSnapshot, PendingPairing,
		PlaneId, Query, QueryResult,
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
			panic!("expected QueryResult::Status");
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
			panic!("expected CommandOutcome::PairingOpened");
		};
		let PairingDisclosure::ManualCode { code } = disclosure else {
			panic!("expected PairingDisclosure::ManualCode");
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
		let CommandOutcome::PairingClaimed { pending, challenge } = outcome
		else {
			panic!("expected CommandOutcome::PairingClaimed");
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
			panic!("expected QueryResult::Pairing");
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
			panic!("expected QueryResult::Events");
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
			panic!("expected QueryResult::SecurityAudit");
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

		let confirmed =
			confirm(&core, &actor(), claim.offer_id, displayed(&claim))
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

		let refused = confirm(
			&core,
			&pairing_client(),
			claim.offer_id,
			displayed(&claim),
		)
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
		let too_late =
			confirm(&core, &actor(), claim.offer_id, displayed(&claim))
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
				vec![
					elevated("pairing.confirmed"),
					elevated("pairing.completed"),
				]
			)
		);
	}
}
