//! Pairing: how a GUI client comes to control this Plane (ADR-0017).
//!
//! Pairing is the one authorization that widens who may drive a Plane, so
//! it starts with a switch rather than with a request. The Pairing gate is
//! Plane-level and concerns new clients only: an owner opens it for as long
//! as a pairing takes, and closing it again leaves every client that is
//! already Paired exactly as it was.
//!
//! Behind an open gate the Plane issues one offer at a time. The offer
//! carries a one-time secret the owner reads off the target and the person
//! pairing presents back, which proves that somebody with access to the
//! target meant this to happen. The secret is short-lived, single-use, and
//! survives few wrong guesses; the durable credential it leads to is the
//! client installation's own public key.

use std::fmt;
use std::time::SystemTime;

use jet_store::{
	PairingGate, PairingInvalidation, PairingKeyAlgorithm, PairingMethod,
	PairingOfferRecord, PairingOfferState, WriteTransaction,
};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::audit::{self, AuditDecision, AuditSubject, Decision};
use crate::command::CommandOutcome;
use crate::error::CoreError;
use crate::event::{EventKind, EventSequence, EventSubject};
use crate::{Actor, ClientId, system_time};

/// How long one step of a Pairing has. ADR-0017 gives the token two
/// minutes; the claim it produces gets its own two minutes to be confirmed
/// in, because the people comparing an authentication string are not the
/// same speed as the machine that issued it.
pub(crate) const PAIRING_WINDOW_MS: i64 = 2 * 60 * 1000;

/// How many wrong secrets one offer survives before it is dead
/// (ADR-0017: five).
pub(crate) const MAX_PAIRING_ATTEMPTS: u32 = 5;

/// Durable identity of one Pairing offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PairingOfferId(pub Uuid);

/// One offer's one-time secret: the code a person reads out, or the token
/// inside a QR payload.
///
/// Its `Debug` says nothing, so a secret cannot reach a diagnostic log by
/// being part of something else that was printed (ADR-0061), and equality
/// is constant-time so no comparison of one leaks where it differed.
#[derive(Clone, Serialize, Deserialize)]
pub struct PairingSecret(pub String);

/// The one-time secret as the Plane hands it to the owner who opened the
/// offer.
///
/// It is disclosed once. A retry of the Command that opened the offer is
/// answered without it, because the receipt that makes the retry idempotent
/// is durable and a live secret has no business being in it (ADR-0093).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairingDisclosure {
	/// The eight-digit code, grouped as `xxxx-yyyy`.
	ManualCode {
		/// The code to read out.
		code: String,
	},
	/// The versioned payload to render as a QR code, carrying the endpoint
	/// and the offer's one-time token.
	QrPayload {
		/// The payload to render.
		payload: String,
	},
	/// The Plane already disclosed this offer's secret. Open another offer
	/// to be given one.
	AlreadyDisclosed,
}

/// The public half of one Client identity: the durable credential a
/// completed Pairing leaves behind (ADR-0017).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientPublicKey {
	/// The algorithm it signs with, retained so a Plane that later speaks a
	/// second one can tell which key is which.
	pub algorithm: PairingKeyAlgorithm,
	/// The key itself. Every v1 Client identity is Ed25519, whose public
	/// keys are 32 bytes.
	pub key: [u8; 32],
}

/// One fresh challenge, issued when an offer is claimed and signed by the
/// Client identity to complete the Pairing (ADR-0090).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingChallenge(pub [u8; 32]);

/// What both sides display for the people at each end to compare
/// (ADR-0017).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthenticationString(pub String);

/// How far the Plane's one Pairing offer has got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairingProgress {
	/// Issued, and waiting for a client to present its secret.
	Offered,
	/// A client presented the secret. Both sides now display the
	/// authentication string, and the pairing waits for the people at each
	/// end to agree that it is the same string.
	AwaitingConfirmation {
		/// The Client identity that claimed the offer.
		client_id: ClientId,
		/// What both sides display.
		authentication_string: AuthenticationString,
	},
	/// Over. It can only be replaced by a new offer.
	Ended {
		/// Why it is over.
		reason: PairingEnd,
	},
}

/// Why a Pairing offer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairingEnd {
	/// Its window passed. Nothing was written when it did: an offer nobody
	/// used is simply not accepted any more.
	Expired,
	/// Too many wrong secrets were presented against it.
	TooManyAttempts,
	/// The owner closed the Plane's Pairing gate while it was open.
	GateClosed,
}

/// The Plane's one Pairing offer, without the secret it was issued with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingPairing {
	/// Durable identity.
	pub offer_id: PairingOfferId,
	/// How its secret reached the person pairing.
	pub method: PairingMethod,
	/// How far it has got.
	pub progress: PairingProgress,
	/// How many wrong secrets it still survives.
	pub attempts_remaining: u32,
	/// When it was opened.
	pub opened_at: SystemTime,
	/// When its current step stops being accepted.
	pub expires_at: SystemTime,
}

/// The Plane's Pairing as it stands, fenced by the journal position the
/// snapshot was read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingSnapshot {
	/// Newest Event sequence visible when the snapshot was read.
	pub cursor: EventSequence,
	/// Whether a new GUI client may begin Pairing.
	pub gate: PairingGate,
	/// The offer the Plane has open, if any. A Plane pairs with one client
	/// at a time, so opening an offer replaces whatever was open.
	pub pending: Option<PendingPairing>,
}

impl fmt::Debug for PairingSecret {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str("PairingSecret(..)")
	}
}

impl PartialEq for PairingSecret {
	fn eq(&self, other: &Self) -> bool {
		self.0.as_bytes().ct_eq(other.0.as_bytes()).into()
	}
}

impl Eq for PairingSecret {}

/// What opening or closing the gate decides.
pub(crate) fn gate_decision(gate: PairingGate) -> AuditDecision {
	match gate {
		PairingGate::Open => AuditDecision::PairingGateOpened,
		PairingGate::Closed => AuditDecision::PairingGateClosed,
	}
}

/// The offer as a client sees it, with the progress it has at
/// `now_unix_ms`.
pub(crate) fn pending(
	record: &PairingOfferRecord,
	now_unix_ms: i64,
) -> PendingPairing {
	PendingPairing {
		offer_id: PairingOfferId(record.offer_id),
		method: record.method.clone(),
		progress: progress(record, now_unix_ms),
		attempts_remaining: MAX_PAIRING_ATTEMPTS
			.saturating_sub(record.failed_attempts),
		opened_at: system_time(record.opened_at_unix_ms),
		expires_at: system_time(record.expires_at_unix_ms),
	}
}

/// How far `record` has got at `now_unix_ms`.
///
/// Expiry is decided here rather than written: an offer nobody used is
/// simply not accepted any more, and a Plane that had to wake up to record
/// that would be a Plane that answers differently depending on whether it
/// was awake (ADR-0055).
fn progress(record: &PairingOfferRecord, now_unix_ms: i64) -> PairingProgress {
	if let Some(invalidation) = record.invalidation {
		return PairingProgress::Ended {
			reason: match invalidation {
				PairingInvalidation::TooManyAttempts => {
					PairingEnd::TooManyAttempts
				}
				PairingInvalidation::GateClosed => PairingEnd::GateClosed,
			},
		};
	}
	if now_unix_ms > record.expires_at_unix_ms {
		return PairingProgress::Ended {
			reason: PairingEnd::Expired,
		};
	}
	match (record.state, &record.claim) {
		(PairingOfferState::Offered, _)
		| (PairingOfferState::AwaitingConfirmation, None) => PairingProgress::Offered,
		(PairingOfferState::AwaitingConfirmation, Some(claim)) => {
			PairingProgress::AwaitingConfirmation {
				client_id: ClientId(claim.client_id),
				authentication_string: AuthenticationString(
					claim.authentication_string.clone(),
				),
			}
		}
		// An invalidated offer answered above; the store cannot hold one
		// without a reason to be invalidated for.
		(PairingOfferState::Invalidated, _) => PairingProgress::Ended {
			reason: PairingEnd::Expired,
		},
	}
}

/// Records where the owner left the Pairing gate and journals it.
///
/// Closing the gate ends the offer the Plane had open. The gate decides
/// whether a new client may begin Pairing, and a pairing halfway through is
/// one that has begun; leaving it claimable would make the switch mean
/// something narrower than it says.
///
/// The gate is written even when it already stood there, because the
/// journal and the Security audit answer "who decided this, and when",
/// which a Plane that quietly dropped the second decision could not.
///
/// # Errors
///
/// Returns a store category [`CoreError`] when the change cannot be
/// written.
pub(crate) async fn set_gate(
	tx: &mut WriteTransaction,
	actor: &Actor,
	gate: PairingGate,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	tx.set_pairing_gate(gate, now_unix_ms).await?;
	tx.append_event(EventKind::PairingGateChanged { gate }.to_record(
		actor,
		EventSubject::Plane,
		now_unix_ms,
	)?)
	.await?;
	// ASVS 16.2.1: deciding whether this Plane accepts new clients at all
	// is a Security audit decision, not only journal history (ADR-0105).
	audit::record(
		tx,
		actor,
		Decision::succeeded(gate_decision(gate), AuditSubject::Plane),
		now_unix_ms,
	)
	.await?;
	if gate == PairingGate::Closed {
		end_open_offer(tx, actor, now_unix_ms).await?;
	}
	Ok(CommandOutcome::PairingGateSet { gate })
}

/// Ends the offer a closing gate leaves behind, if there is one still
/// waiting to be used.
async fn end_open_offer(
	tx: &mut WriteTransaction,
	actor: &Actor,
	now_unix_ms: i64,
) -> Result<(), CoreError> {
	let Some(record) = tx.pairing_offer().await? else {
		return Ok(());
	};
	if matches!(
		progress(&record, now_unix_ms),
		PairingProgress::Ended { .. }
	) {
		return Ok(());
	}
	tx.invalidate_pairing_offer(PairingInvalidation::GateClosed)
		.await?;
	let offer_id = PairingOfferId(record.offer_id);
	tx.append_event(
		EventKind::PairingOfferEnded {
			offer_id,
			reason: PairingEnd::GateClosed,
		}
		.to_record(actor, EventSubject::Plane, now_unix_ms)?,
	)
	.await?;
	Ok(())
}
