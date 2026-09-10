//! Applies audit retention and anonymization policy.

use super::AuditSubject;
use crate::{
	error::CoreError,
	setting::{self, SettingKey, SettingValue},
};
use jet_store::{Store, WriteTransaction};

/// Milliseconds in one day, which is the unit the retention window is set
/// in.
pub(super) const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Removes Security audit records the Plane has stopped keeping, and
/// returns how many.
///
/// Retention is enforced when the daemon starts. The audit is written by
/// decisions a person makes rather than by activity, so waking an idle
/// Plane on a timer to sweep it would cost more than it saves (ADR-0055);
/// a Plane left running for longer than its window keeps expired records
/// until its next start.
///
/// # Errors
///
/// Returns a store category [`CoreError`] when the window cannot be read
/// or a batch cannot be removed.
pub(crate) async fn sweep_retention(
	store: &Store,
	now_unix_ms: i64,
) -> Result<usize, CoreError> {
	let window = store
		.read(async |tx| {
			setting::resolve_plane(tx, SettingKey::SecurityAuditRetentionDays)
				.await
		})
		.await?;
	let SettingValue::Count(days) = window else {
		return Err(CoreError::internal(
			"audit.retention_unreadable",
			format!("the retention window resolved to {window:?}"),
		));
	};
	let cutoff =
		now_unix_ms.saturating_sub(i64::from(days).saturating_mul(DAY_MS));
	Ok(store.prune_audit_before(cutoff).await?)
}

/// Forgets what the target of `subject` was called, wherever the Security
/// audit recorded a decision about it, and returns how many records that
/// was.
///
/// The opaque reference each of those records is chained over stays, so
/// the chain is untouched and the audit still says that some Conversation
/// was deleted and which decisions were about the same one (ADR-0105). The
/// count is what a deletion preview discloses.
///
/// # Errors
///
/// Returns a store category [`CoreError`] when the records cannot be
/// updated.
#[allow(
	dead_code,
	reason = "called by Conversation deletion in follow-up issue #53"
)]
pub(crate) async fn anonymize(
	tx: &mut WriteTransaction,
	subject: AuditSubject,
) -> Result<usize, CoreError> {
	let Some(identity) = subject.identity() else {
		return Ok(0);
	};
	Ok(tx.anonymize_audit_target(subject.kind(), &identity).await?)
}
