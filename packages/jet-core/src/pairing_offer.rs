//! Issuing the Plane's one Pairing offer, and claiming it (ADR-0017).
//!
//! An offer is opened by the owner in front of the target and claimed by
//! the client that presents its secret back. Claiming proves fresh intent
//! and nothing more: it leaves the Pairing waiting for the people at both
//! ends to compare an authentication string, and the durable credential is
//! the public key the claim carried.

use jet_store::{
	NewPairingClaim, NewPairingOffer, PairingGate, PairingInvalidation,
	PairingMethod, PairingOfferRecord, PairingOfferState, WriteTransaction,
};
use uuid::Uuid;

use crate::audit::{self, AuditDecision, AuditSubject, Decision};
use crate::command::CommandOutcome;
use crate::error::CoreError;
use crate::event::{EventKind, EventSubject};
use crate::pairing::{
	self, ClientPublicKey, MAX_PAIRING_ATTEMPTS, PAIRING_WINDOW_MS, PairingEnd,
	PairingOfferId, PairingProgress, PairingSecret,
};
use crate::pairing_secret as secret;
use crate::{Actor, PlaneId};

/// Longest endpoint a QR payload may advertise. A host name and a user are
/// metadata a person reads, not a payload.
const MAX_ENDPOINT_CHARS: usize = 255;

/// Issues the Plane's one Pairing offer, replacing whatever was open, and
/// discloses its one-time secret to the owner who asked for it.
///
/// # Errors
///
/// Returns a `conflict` [`CoreError`] when the Pairing gate is closed, an
/// `invalid_input` one when the endpoint is not an endpoint, an
/// `unavailable` one when the Plane cannot draw a secret, and a store
/// category when the offer cannot be written.
pub(crate) async fn open(
	tx: &mut WriteTransaction,
	actor: &Actor,
	method: PairingMethod,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	require_endpoint(&method)?;
	if tx.pairing_gate().await? == PairingGate::Closed {
		return Err(CoreError::conflict(
			"pairing.gate_closed",
			"this Plane is not accepting new Pairings; open its Pairing gate \
			 first",
		));
	}
	let (secret, disclosure) = secret::issue(&method)?;
	let salt = secret::salt()?;
	let secret_digest = secret::digest(&salt, &method, &secret);
	let record = tx
		.replace_pairing_offer(NewPairingOffer {
			offer_id: Uuid::now_v7(),
			method: method.clone(),
			secret_salt: salt,
			secret_digest,
			opened_by: actor.client_id().0,
			opened_at_unix_ms: now_unix_ms,
			expires_at_unix_ms: now_unix_ms.saturating_add(PAIRING_WINDOW_MS),
		})
		.await?;
	let offer_id = PairingOfferId(record.offer_id);
	// The journal records that an offer was made and how it was handed
	// over, and no part of the secret itself.
	tx.append_event(EventKind::PairingOffered { offer_id, method }.to_record(
		actor,
		EventSubject::Plane,
		now_unix_ms,
	)?)
	.await?;
	// ASVS 16.2.1: opening the window in which an unknown client can come
	// to control the Plane is a Security audit decision (ADR-0105).
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::PairingOffered,
			AuditSubject::PairingOffer(offer_id),
		),
		now_unix_ms,
	)
	.await?;
	Ok(CommandOutcome::PairingOpened {
		pending: pairing::pending(&record, now_unix_ms),
		disclosure,
	})
}

/// Claims the open offer with the secret a person presented and the public
/// key of the Client identity presenting it.
///
/// A wrong secret is counted against the offer and commits: five of them
/// end it, and a Plane that forgot the four before would let a client guess
/// for as long as the window lasts (ADR-0017).
///
/// # Errors
///
/// Returns a `not_found` [`CoreError`] when no offer is open, a `conflict`
/// one when the offer is over or already claimed, an `invalid_input` one
/// when the secret does not match, and a store category when the claim
/// cannot be written.
pub(crate) async fn claim(
	tx: &mut WriteTransaction,
	actor: &Actor,
	presented: PairingSecret,
	key: ClientPublicKey,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let record = open_offer(tx, now_unix_ms).await?;
	let offer_id = PairingOfferId(record.offer_id);
	if !secret::matches(
		&record.secret_salt,
		&record.secret_digest,
		&record.method,
		&presented,
	) {
		return refuse(tx, actor, &record, now_unix_ms).await;
	}
	let plane_id = PlaneId(tx.plane().await?.plane_id);
	let challenge = secret::challenge()?;
	let authentication_string =
		secret::authentication_string(&secret::transcript(
			plane_id.0,
			record.offer_id,
			actor.client_id().0,
			&key,
			&challenge,
		));
	let claim = NewPairingClaim {
		client_id: actor.client_id().0,
		key_algorithm: key.algorithm,
		public_key: key.key,
		challenge: challenge.0,
		authentication_string: authentication_string.0,
	};
	// Comparing the authentication string is done by people, so the claim
	// gets its own window rather than what is left of the secret's.
	let expires_at_unix_ms = now_unix_ms.saturating_add(PAIRING_WINDOW_MS);
	tx.record_pairing_claim(&claim, expires_at_unix_ms).await?;
	let claimed = PairingOfferRecord {
		state: PairingOfferState::AwaitingConfirmation,
		claim: Some(claim),
		expires_at_unix_ms,
		..record
	};
	tx.append_event(
		EventKind::PairingClaimed {
			offer_id,
			client_id: actor.client_id(),
		}
		.to_record(actor, EventSubject::Plane, now_unix_ms)?,
	)
	.await?;
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::PairingClaimed,
			AuditSubject::PairingOffer(offer_id),
		),
		now_unix_ms,
	)
	.await?;
	Ok(CommandOutcome::PairingClaimed {
		pending: pairing::pending(&claimed, now_unix_ms),
		challenge,
	})
}

/// The offer a claim may still be made against.
async fn open_offer(
	tx: &mut WriteTransaction,
	now_unix_ms: i64,
) -> Result<PairingOfferRecord, CoreError> {
	let Some(record) = tx.pairing_offer().await? else {
		return Err(CoreError::not_found(
			"pairing.none_offered",
			"this Plane has no Pairing offer open",
		));
	};
	match pairing::pending(&record, now_unix_ms).progress {
		PairingProgress::Offered => Ok(record),
		// The secret is single-use: the first claim spends it, and a second
		// one is not a guess to be counted but an offer that is over.
		PairingProgress::AwaitingConfirmation { .. } => {
			Err(CoreError::conflict(
				"pairing.already_claimed",
				"this Pairing offer was already claimed; open another one",
			))
		}
		PairingProgress::Ended { reason } => Err(CoreError::conflict(
			ended_code(reason),
			format!("{}; open another one", ended_message(reason)),
		)),
	}
}

/// Counts one wrong secret against the offer, ending it once too many have
/// been presented, and refuses the claim.
async fn refuse(
	tx: &mut WriteTransaction,
	actor: &Actor,
	record: &PairingOfferRecord,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let offer_id = PairingOfferId(record.offer_id);
	let attempts = tx.record_failed_pairing_attempt().await?;
	let remaining = MAX_PAIRING_ATTEMPTS.saturating_sub(attempts);
	// ASVS 16.2.1: a secret presented and refused is what an attempt to
	// take control of the Plane looks like, so the audit keeps it.
	audit::record(
		tx,
		actor,
		Decision {
			decision: AuditDecision::PairingClaimed,
			subject: AuditSubject::PairingOffer(offer_id),
			outcome: jet_store::AuditOutcome::Denied,
		},
		now_unix_ms,
	)
	.await?;
	if remaining == 0 {
		tx.invalidate_pairing_offer(PairingInvalidation::TooManyAttempts)
			.await?;
		tx.append_event(
			EventKind::PairingOfferEnded {
				offer_id,
				reason: PairingEnd::TooManyAttempts,
			}
			.to_record(actor, EventSubject::Plane, now_unix_ms)?,
		)
		.await?;
		audit::record(
			tx,
			actor,
			Decision::succeeded(
				AuditDecision::PairingOfferInvalidated,
				AuditSubject::PairingOffer(offer_id),
			),
			now_unix_ms,
		)
		.await?;
	}
	// An authoritative refusal, so the attempt it counted commits with it.
	Err(CoreError::invalid_input(
		"pairing.secret_rejected",
		format!(
			"that is not this Pairing offer's secret; {remaining} attempts \
			 remain"
		),
	))
}

fn ended_code(reason: PairingEnd) -> &'static str {
	match reason {
		PairingEnd::Expired => "pairing.offer_expired",
		PairingEnd::TooManyAttempts | PairingEnd::GateClosed => {
			"pairing.offer_ended"
		}
	}
}

fn ended_message(reason: PairingEnd) -> &'static str {
	match reason {
		PairingEnd::Expired => "this Pairing offer expired",
		PairingEnd::TooManyAttempts => {
			"this Pairing offer ended after too many wrong secrets"
		}
		PairingEnd::GateClosed => {
			"this Pairing offer ended when the Pairing gate was closed"
		}
	}
}

/// Refuses an endpoint that is not the address a client can reach the Plane
/// at: bounded text without spaces or the control characters no address
/// carries.
fn require_endpoint(method: &PairingMethod) -> Result<(), CoreError> {
	let PairingMethod::QrPayload { endpoint } = method else {
		return Ok(());
	};
	let supported = !endpoint.is_empty()
		&& endpoint.chars().count() <= MAX_ENDPOINT_CHARS
		&& !endpoint.chars().any(|character| {
			character.is_control() || character.is_whitespace()
		});
	if supported {
		return Ok(());
	}
	Err(CoreError::invalid_input(
		"pairing.endpoint_unsupported",
		format!(
			"an endpoint is at most {MAX_ENDPOINT_CHARS} characters without \
			 spaces or control characters"
		),
	))
}
