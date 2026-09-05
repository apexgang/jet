//! Validating the Security audit against the head kept outside it
//! (ADR-0105).
//!
//! Validation walks the newest authority epoch: that epoch is exactly the
//! stretch of audit this Plane still vouches for. Every record in it is
//! folded again from the fields the store holds, and the result has to pass
//! through the durable head. A record whose fields were edited no longer
//! folds to its own link; a store that was restored from a snapshot no
//! longer holds the record the head names.
//!
//! Earlier epochs are read and exported but not revalidated. Their integrity
//! was already decided when the owner began the epoch that replaced them,
//! and the gap that decision left is recorded in the epoch itself.

use uuid::Uuid;

use crate::audit::{
	AUDIT_PAGE_LIMIT, AuditRecord, chain_link, target_matches_reference,
};
use crate::audit_chain::AuditEntryHash;
use crate::audit_epoch::EpochRow;
use crate::audit_head::{self, AuditHead};
use crate::transaction::ReadTransaction;
use crate::{Store, StoreError};

/// What validating the Security audit found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditIntegrity {
	/// The chain folds through the durable head.
	Verified {
		/// The position the audit has actually reached, republished so a
		/// head write lost to a crash catches up. `None` before the Plane
		/// has ever recorded a decision.
		head: Option<AuditHead>,
	},
	/// It does not, with the evidence the owner exports before choosing to
	/// carry on in a new epoch.
	Failed(AuditIntegrityFailure),
}

/// Everything known about one integrity failure. It is the evidence an
/// interactive owner exports; it names positions and hashes and quotes no
/// record content, because the audit holds none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditIntegrityFailure {
	/// What was found to be wrong.
	pub breach: AuditBreach,
	/// The authority epoch that failed to validate.
	pub epoch: u64,
	/// The head published outside the store, if it still has one.
	pub head: Option<AuditHead>,
	/// The newest position the store itself holds.
	pub store_sequence: u64,
}

/// How the Security audit failed to validate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditBreach {
	/// The Plane has recorded decisions, but nothing outside the store says
	/// how far its chain had reached.
	HeadMissing,
	/// The store does not hold the record the head names, so it moved
	/// backwards behind the audit.
	HeadNotInStore,
	/// The store holds that record, and it is not the one the head names,
	/// so the history behind it was rewritten.
	HeadDiverged,
	/// The record at this position no longer folds to its own link, so its
	/// fields were altered after it was written.
	RecordAltered {
		/// Where the fold first disagreed.
		sequence: u64,
	},
	/// The identity at this position is not the one its opaque target
	/// reference was derived from.
	TargetAltered {
		/// Where the target and its reference first disagreed.
		sequence: u64,
	},
}

impl AuditBreach {
	/// The stable code a new epoch records as the reason it succeeds a
	/// chain the Plane stopped vouching for.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::HeadMissing => "audit.head_missing",
			Self::HeadNotInStore => "audit.head_not_in_store",
			Self::HeadDiverged => "audit.head_diverged",
			Self::RecordAltered { .. } => "audit.record_altered",
			Self::TargetAltered { .. } => "audit.target_altered",
		}
	}
}

/// Whether the head has been met while folding the chain.
enum HeadMatch {
	/// Not reached yet, or never present.
	Pending,
	/// The chain passed exactly through it.
	Matched,
	/// The chain reached its position holding a different link.
	Diverged,
}

impl Store {
	/// Validates the Security audit and republishes its head when the chain
	/// is whole.
	///
	/// Republishing is what repairs a head write lost between a commit and
	/// the crash that followed it: the store is ahead of the head, the fold
	/// still passes through the head, and the head catches up. It never
	/// moves a head the chain does not reach.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the store or the head file cannot be
	/// read, or when the head that validated cannot be written back.
	pub async fn validate_audit(&self) -> Result<AuditIntegrity, StoreError> {
		let plane_id = self.plane().await?.plane_id;
		let head = audit_head::read(&self.database, plane_id)?;
		let integrity = self
			.read(async |tx| validate(tx, plane_id, head).await)
			.await?;
		if let AuditIntegrity::Verified { head: Some(head) } = integrity
			&& Some(head) != audit_head::read(&self.database, plane_id)?
		{
			audit_head::write(&self.database, plane_id, head)?;
		}
		Ok(integrity)
	}
}

async fn validate(
	tx: &mut ReadTransaction,
	plane_id: Uuid,
	head: Option<AuditHead>,
) -> Result<AuditIntegrity, StoreError> {
	let store_sequence = tx.audit_cursor().await?;
	let Some(epoch) = tx.newest_audit_epoch().await? else {
		// The Plane has never recorded a decision. A head without an audit
		// behind it is a store that was replaced under one.
		return Ok(match head {
			None => AuditIntegrity::Verified { head: None },
			Some(head) => failed(
				AuditBreach::HeadNotInStore,
				0,
				Some(head),
				store_sequence,
			),
		});
	};
	let mut fold = Fold::start(tx, plane_id, &epoch, head).await?;
	loop {
		let page = tx
			.audit_epoch_page(epoch.epoch, fold.sequence, AUDIT_PAGE_LIMIT)
			.await?;
		if page.is_empty() {
			break;
		}
		for record in page {
			if let Some(breach) = fold.advance(&record) {
				return Ok(failed(breach, epoch.epoch, head, store_sequence));
			}
		}
	}
	Ok(fold.finish(epoch.epoch, store_sequence))
}

/// One pass over the records of an epoch, carrying the link the next record
/// must follow.
struct Fold {
	head: Option<AuditHead>,
	epoch: u64,
	sequence: u64,
	link: AuditEntryHash,
	head_match: HeadMatch,
}

impl Fold {
	/// Starts where this epoch's chain starts: at the record retention last
	/// removed when it removed one from this epoch, and otherwise at the
	/// epoch's own genesis.
	async fn start(
		tx: &mut ReadTransaction,
		plane_id: Uuid,
		epoch: &EpochRow,
		head: Option<AuditHead>,
	) -> Result<Self, StoreError> {
		let anchor = tx.audit_retention_anchor().await?;
		let (sequence, link) = match anchor {
			Some(anchor) if anchor.epoch == epoch.epoch => {
				(anchor.sequence, anchor.entry_hash)
			}
			Some(_) | None => (
				epoch
					.preceding
					.as_ref()
					.map_or(0, |preceding| preceding.sequence),
				epoch.genesis(plane_id),
			),
		};
		let mut fold = Self {
			head,
			epoch: epoch.epoch,
			sequence,
			link,
			head_match: HeadMatch::Pending,
		};
		// An epoch the owner has just begun holds no record yet, so its
		// head names the genesis the next record will follow.
		fold.match_head();
		Ok(fold)
	}

	/// Folds one record in, or reports the first thing that does not add up.
	fn advance(&mut self, record: &AuditRecord) -> Option<AuditBreach> {
		if !target_matches_reference(record) {
			return Some(AuditBreach::TargetAltered {
				sequence: record.sequence,
			});
		}
		if chain_link(self.link, record) != record.entry_hash {
			return Some(AuditBreach::RecordAltered {
				sequence: record.sequence,
			});
		}
		self.sequence = record.sequence;
		self.link = record.entry_hash;
		self.match_head();
		None
	}

	/// Notes whether the head names where the fold now stands.
	fn match_head(&mut self) {
		let Some(head) = self.head else {
			return;
		};
		if head.epoch == self.epoch && head.sequence == self.sequence {
			self.head_match = if head.entry_hash == self.link {
				HeadMatch::Matched
			} else {
				HeadMatch::Diverged
			};
		}
	}

	fn finish(self, epoch: u64, store_sequence: u64) -> AuditIntegrity {
		let reached = AuditHead {
			epoch,
			sequence: self.sequence,
			entry_hash: self.link,
		};
		match (self.head, self.head_match) {
			(None, _) if self.sequence == 0 => {
				AuditIntegrity::Verified { head: None }
			}
			(None, _) => {
				failed(AuditBreach::HeadMissing, epoch, None, store_sequence)
			}
			(Some(_), HeadMatch::Matched) => AuditIntegrity::Verified {
				head: Some(reached),
			},
			(Some(head), HeadMatch::Diverged) => failed(
				AuditBreach::HeadDiverged,
				epoch,
				Some(head),
				store_sequence,
			),
			(Some(head), HeadMatch::Pending) => failed(
				AuditBreach::HeadNotInStore,
				epoch,
				Some(head),
				store_sequence,
			),
		}
	}
}

fn failed(
	breach: AuditBreach,
	epoch: u64,
	head: Option<AuditHead>,
	store_sequence: u64,
) -> AuditIntegrity {
	AuditIntegrity::Failed(AuditIntegrityFailure {
		breach,
		epoch,
		head,
		store_sequence,
	})
}
