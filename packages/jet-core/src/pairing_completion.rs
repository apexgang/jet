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

use jet_store::{NewPairedClient, WriteTransaction};

use crate::audit::{self, AuditDecision, AuditSubject, Decision};
use crate::command::CommandOutcome;
use crate::error::CoreError;
use crate::event::{EventKind, EventSubject};
use crate::pairing::{
	self, AuthenticationString, ClientPublicKey, PAIRING_PROTOCOL,
	PAIRING_WINDOW_MS, PairingChallenge, PairingOfferId, PairingProgress,
	PairingSignature,
};
use crate::pairing_offer;
use crate::pairing_secret as secret;
use crate::{Actor, ClientId, PlaneId, pairing_identity};

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
		return Err(not_awaiting_confirmation(&progress));
	};
	if client_id == actor.client_id() {
		return Err(CoreError::conflict(
			"pairing.confirmation_by_claimant",
			"the client being Paired cannot confirm its own Pairing; \
			 confirm it on the Plane being Paired with",
		));
	}
	if !secret::same_authentication_string(&authentication_string, &presented) {
		return Err(CoreError::invalid_input(
			"pairing.authentication_string_mismatch",
			"that is not the authentication string this Pairing is showing",
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
		return Err(not_confirmed(&progress));
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

fn not_awaiting_confirmation(progress: &PairingProgress) -> CoreError {
	match progress {
		PairingProgress::Offered => CoreError::conflict(
			"pairing.not_claimed",
			"nobody has presented this Pairing offer's secret yet",
		),
		PairingProgress::Confirmed { .. } => CoreError::conflict(
			"pairing.already_confirmed",
			"this Pairing was already confirmed",
		),
		PairingProgress::AwaitingConfirmation { .. } => CoreError::internal(
			"pairing.progress_unexpected",
			"an offer awaiting confirmation reported it could not be \
			 confirmed",
		),
		PairingProgress::Ended { reason } => ended(*reason),
	}
}

fn not_confirmed(progress: &PairingProgress) -> CoreError {
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
		PairingProgress::Confirmed { .. } => CoreError::internal(
			"pairing.progress_unexpected",
			"a confirmed offer reported it was not confirmed",
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
