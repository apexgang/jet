//! Pairing: how a GUI client comes to control this Plane (ADR-0017).
//!
//! Pairing is the one authorization that widens who may drive a Plane, so
//! it starts with a switch rather than with a request. The Pairing gate is
//! Plane-level and concerns new clients only: an owner opens it for as long
//! as a pairing takes, and closing it again leaves every client that is
//! already Paired exactly as it was.

use jet_store::{PairingGate, WriteTransaction};

use crate::Actor;
use crate::audit::{self, AuditDecision, AuditSubject, Decision};
use crate::command::CommandOutcome;
use crate::error::CoreError;
use crate::event::{EventKind, EventSequence, EventSubject};

/// The Plane's Pairing as it stands, fenced by the journal position the
/// snapshot was read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingSnapshot {
	/// Newest Event sequence visible when the snapshot was read.
	pub cursor: EventSequence,
	/// Whether a new GUI client may begin Pairing.
	pub gate: PairingGate,
}

/// What opening or closing the gate decides.
pub(crate) fn gate_decision(gate: PairingGate) -> AuditDecision {
	match gate {
		PairingGate::Open => AuditDecision::PairingGateOpened,
		PairingGate::Closed => AuditDecision::PairingGateClosed,
	}
}

/// Records where the owner left the Pairing gate and journals it.
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
	Ok(CommandOutcome::PairingGateSet { gate })
}
