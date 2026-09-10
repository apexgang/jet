//! Records accepted and refused actions in the Security audit.

use super::{Decision, decision_for, policy::refused_subject};
use crate::{Actor, command::Command, error::CoreError};
use jet_store::{
	AuditOutcome, AuditRisk, NewAuditRecord, Store, WriteTransaction,
};
use uuid::Uuid;

/// Records that `command` was refused before it changed anything, when it
/// is one the audit records.
///
/// A Command turned away because the Plane can no longer do what it needs
/// is an outcome worth keeping: an Account binding refused for want of a
/// credential store is how an authentication setup fails on this Plane, and
/// ADR-0105 asks for the failures as much as the successes.
///
/// A Command refused because the audit itself is in doubt is not recorded.
/// There would be nothing to rely on in the record, and writing one would
/// let a client grow an audit the Plane has already stopped vouching for.
///
/// # Errors
///
/// Returns a store category [`CoreError`] when the record cannot be
/// written.
pub(crate) async fn record_refusal(
	store: &Store,
	actor: &Actor,
	command: &Command,
	now_unix_ms: i64,
) -> Result<(), CoreError> {
	let Some(decision) = decision_for(command) else {
		return Ok(());
	};
	let subject = refused_subject(command);
	store
		.write(async |tx| {
			record(
				tx,
				actor,
				Decision {
					decision,
					subject,
					outcome: AuditOutcome::Denied,
				},
				now_unix_ms,
			)
			.await
		})
		.await
}

/// Records `decision` in the Security audit inside the transaction that
/// carries it out, so the decision and its record commit together or not at
/// all.
///
/// # Errors
///
/// Returns a store category [`CoreError`] when the record cannot be
/// written.
pub(crate) async fn record(
	tx: &mut WriteTransaction,
	actor: &Actor,
	decision: Decision,
	now_unix_ms: i64,
) -> Result<(), CoreError> {
	tx.append_audit_record(NewAuditRecord {
		record_id: Uuid::now_v7(),
		recorded_at_unix_ms: now_unix_ms,
		actor: actor.record().into(),
		target_kind: decision.subject.kind().into(),
		target_id: decision.subject.identity(),
		decision: decision.decision.as_str().into(),
		risk: decision.decision.risk(),
		outcome: decision.outcome,
	})
	.await?;
	Ok(())
}

/// Accepts a verified digest revocation and its audit evidence atomically.
pub(crate) async fn record_craft_revocation(
	tx: &mut WriteTransaction,
	digest: &str,
	now_unix_ms: i64,
) -> Result<(), CoreError> {
	if tx.craft_revoked(digest).await? {
		return Ok(());
	}
	// ASVS 16.2.1, 16.3.3: internal attribution and the exact revoked digest
	// commit with the admission barrier, once even when metadata is replayed.
	tx.append_audit_record(NewAuditRecord {
		record_id: Uuid::now_v7(),
		recorded_at_unix_ms: now_unix_ms,
		actor: jet_store::AuditActorRecord::CraftRevocation,
		target_kind: "craft_digest".into(),
		target_id: Some(digest.into()),
		decision: "craft.digest_revoked".into(),
		risk: AuditRisk::Elevated,
		outcome: AuditOutcome::Succeeded,
	})
	.await?;
	tx.revoke_craft_digest(digest).await?;
	Ok(())
}
