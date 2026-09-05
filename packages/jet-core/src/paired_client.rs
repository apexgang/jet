//! What a Plane does with the clients it has already Paired with
//! (ADR-0017).
//!
//! The Pairing gate decides whether a new client may begin; these decide
//! what happens to one that already has. Disabling and revoking both stop
//! a client controlling the Plane, and they differ in what is left
//! afterwards: disabling keeps the key, so enabling it again needs nobody
//! in front of either machine, while revoking forgets the key and the
//! installation has to be Paired again.
//!
//! Both take effect on this Plane's durable state. Closing the streams a
//! stopped client already holds is the transport's part of the same
//! decision and arrives with remote connections in issue #14.

use jet_store::{PairedClientAccess, WriteTransaction};

use crate::audit::{self, AuditDecision, AuditSubject, Decision};
use crate::command::CommandOutcome;
use crate::error::CoreError;
use crate::event::{EventKind, EventSubject};
use crate::pairing::{self, PairedClient};
use crate::{Actor, ClientId};

/// Records whether a Paired client may control this Plane.
///
/// # Errors
///
/// Returns a `not_found` [`CoreError`] when the Plane has no such Paired
/// client, or a store category when the change cannot be written.
pub(crate) async fn set_access(
	tx: &mut WriteTransaction,
	actor: &Actor,
	client_id: ClientId,
	access: PairedClientAccess,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let record = paired(tx, client_id).await?;
	tx.set_paired_client_access(client_id.0, access).await?;
	tx.append_event(
		EventKind::PairedClientAccessChanged { client_id, access }.to_record(
			actor,
			EventSubject::Plane,
			now_unix_ms,
		)?,
	)
	.await?;
	// ASVS 16.2.1: letting a client control the Plane again widens trust,
	// and stopping one is the record of when that trust stopped (ADR-0105).
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			access_decision(access),
			AuditSubject::PairedClient(client_id),
		),
		now_unix_ms,
	)
	.await?;
	Ok(CommandOutcome::PairedClientAccessSet {
		client: PairedClient {
			access,
			..pairing::paired_client(record)
		},
	})
}

/// Forgets a Paired client and the key it was Paired with.
///
/// # Errors
///
/// Returns a `not_found` [`CoreError`] when the Plane has no such Paired
/// client, or a store category when the row cannot be removed.
pub(crate) async fn revoke(
	tx: &mut WriteTransaction,
	actor: &Actor,
	client_id: ClientId,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	paired(tx, client_id).await?;
	tx.delete_paired_client(client_id.0).await?;
	tx.append_event(EventKind::PairedClientRevoked { client_id }.to_record(
		actor,
		EventSubject::Plane,
		now_unix_ms,
	)?)
	.await?;
	// The audit keeps which client this was after the Plane has stopped
	// keeping it, which is the whole point of recording a revocation.
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::PairedClientRevoked,
			AuditSubject::PairedClient(client_id),
		),
		now_unix_ms,
	)
	.await?;
	Ok(CommandOutcome::PairedClientRevoked { client_id })
}

/// What deciding one way or the other about a client's access records.
pub(crate) fn access_decision(access: PairedClientAccess) -> AuditDecision {
	match access {
		PairedClientAccess::Enabled => AuditDecision::PairedClientEnabled,
		PairedClientAccess::Disabled => AuditDecision::PairedClientDisabled,
	}
}

async fn paired(
	tx: &mut WriteTransaction,
	client_id: ClientId,
) -> Result<jet_store::PairedClientRecord, CoreError> {
	tx.paired_client(client_id.0).await?.ok_or_else(|| {
		CoreError::not_found(
			"pairing.client_not_found",
			"this Plane is not Paired with that client",
		)
	})
}
