//! Security-degraded mode: what a Plane still does when it can no longer
//! vouch for its own Security audit (ADR-0105).
//!
//! Validation runs when the daemon starts. When it fails, the Plane does
//! not stop: reads, exports and Runs already under way carry on, because
//! refusing them would destroy the evidence and the work at once. What
//! waits is every change the audit would have recorded — trust, policy,
//! Craft, and anything destructive — since the whole point of recording
//! them is a record that can be relied on.
//!
//! The way out is deliberate and belongs to the person, not the daemon.
//! An owner exports the evidence and begins a new authority epoch, which
//! records where the old chain was last known to have reached and why it
//! stops being vouched for there. Nothing here recovers by itself.

use jet_store::{
	AuditBreach, AuditGap, AuditHead, AuditIntegrity, AuditIntegrityFailure,
	WriteTransaction,
};

use crate::audit::{
	self, AuditDecision, AuditEpoch, AuditSequence, AuditSubject, Decision,
};
use crate::command::CommandOutcome;
use crate::error::CoreError;
use crate::event::{EventKind, EventSubject};
use crate::{Actor, AuditOutcome};

/// Whether the Plane can vouch for its own Security audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityState {
	/// The audit chain folds through the head kept outside the store.
	Trusted,
	/// It does not, and the Plane is in Security-degraded mode.
	Degraded(SecurityDegradation),
}

/// What validation found. It names positions and hashes and quotes no
/// record content, because the audit holds none; this is the evidence an
/// owner exports before deciding to carry on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecurityDegradation {
	/// What was found to be wrong.
	pub breach: AuditBreach,
	/// The authority epoch that failed to validate.
	pub epoch: AuditEpoch,
	/// The head published outside the store, when there still is one.
	pub head: Option<AuditHead>,
	/// The newest position the store itself holds.
	pub store_sequence: AuditSequence,
}

/// Whether a Command may run while the audit cannot be vouched for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecurityClass {
	/// Ordinary work. It neither widens trust nor destroys anything, so a
	/// doubtful audit is no reason to refuse it.
	Ordinary,
	/// A change the Security audit exists to record. It waits for an audit
	/// the Plane can vouch for.
	Guarded,
}

impl SecurityState {
	/// The state validation leaves the Plane in.
	pub(crate) fn of(integrity: AuditIntegrity) -> Self {
		match integrity {
			AuditIntegrity::Verified { .. } => Self::Trusted,
			AuditIntegrity::Failed(AuditIntegrityFailure {
				breach,
				epoch,
				head,
				store_sequence,
			}) => Self::Degraded(SecurityDegradation {
				breach,
				epoch: AuditEpoch(epoch),
				head,
				store_sequence: AuditSequence(store_sequence),
			}),
		}
	}

	/// Lets `class` through, or refuses it while the audit is in doubt.
	///
	/// # Errors
	///
	/// Returns a `conflict` [`CoreError`] naming what the owner has to do,
	/// because nothing the client retries will change the answer.
	pub(crate) fn admit(self, class: SecurityClass) -> Result<(), CoreError> {
		match (self, class) {
			(Self::Trusted, _)
			| (Self::Degraded(_), SecurityClass::Ordinary) => Ok(()),
			(Self::Degraded(_), SecurityClass::Guarded) => {
				Err(CoreError::conflict(
					"security.audit_degraded",
					"this Plane cannot vouch for its Security audit; export \
					 the evidence and begin a new audit epoch before \
					 changing trust, policy, or anything destructive",
				))
			}
		}
	}

	fn degradation(self) -> Option<SecurityDegradation> {
		match self {
			Self::Trusted => None,
			Self::Degraded(degradation) => Some(degradation),
		}
	}
}

/// Begins the authority epoch that succeeds a chain the Plane stopped
/// vouching for, and records that as the first decision in it.
///
/// The gap is taken from what validation found rather than from anything a
/// client sends: an owner decides to carry on, and the Plane decides what
/// carrying on has to admit to.
///
/// # Errors
///
/// Returns a `conflict` [`CoreError`] when the audit is not in doubt, an
/// `internal` one when the failure named no position to succeed, and a
/// store category when the epoch cannot be written.
pub(crate) async fn begin_epoch(
	tx: &mut WriteTransaction,
	actor: &Actor,
	security: SecurityState,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let Some(degradation) = security.degradation() else {
		return Err(CoreError::conflict(
			"security.audit_trusted",
			"this Plane vouches for its Security audit, so there is no gap \
			 to carry on from",
		));
	};
	let gap = degradation.gap(tx).await?;
	let epoch = AuditEpoch(tx.begin_audit_epoch(gap, now_unix_ms).await?);
	tx.append_event(EventKind::AuditEpochBegun { epoch }.to_record(
		actor,
		EventSubject::Plane,
		now_unix_ms,
	)?)
	.await?;
	// The first record of the new epoch says who began it, so the chain the
	// Plane now vouches for starts by admitting why it starts.
	audit::record(
		tx,
		actor,
		Decision {
			decision: AuditDecision::AuditEpochBegun,
			subject: AuditSubject::Plane,
			outcome: AuditOutcome::Succeeded,
		},
		now_unix_ms,
	)
	.await?;
	Ok(CommandOutcome::AuditEpochBegun { epoch })
}

impl SecurityDegradation {
	/// Where the chain being left behind was last known to have reached.
	///
	/// The head is the better witness, because it lives outside the state
	/// that may have moved. When it is gone the store's own newest record
	/// is all there is, and the epoch records that instead.
	async fn gap(
		self,
		tx: &mut WriteTransaction,
	) -> Result<AuditGap, CoreError> {
		let reason = self.breach.as_str().to_owned();
		if let Some(head) = self.head {
			return Ok(AuditGap {
				sequence: head.sequence,
				entry_hash: head.entry_hash,
				reason,
			});
		}
		let Some(tip) = tx.audit_tip().await? else {
			return Err(CoreError::internal(
				"security.gap_unknown",
				"the audit failed to validate with neither a head nor a \
				 record to succeed",
			));
		};
		Ok(AuditGap {
			sequence: tip.sequence,
			entry_hash: tip.entry_hash,
			reason,
		})
	}
}
