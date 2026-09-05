//! The owner-only Security audit: an integrity-chained record of the
//! decisions that widen trust, change policy, or destroy state (ADR-0105).
//!
//! It is not the Event journal. The journal is Conversation history that
//! clients subscribe to; this is a separate, narrower record that answers
//! who decided what about which target, how risky it was, and how it turned
//! out. No column here can hold a credential, a prompt, terminal output, or
//! file content, and the core has no way to put one in.
//!
//! Each record commits inside the same transaction as the decision it
//! describes, and carries the chain link that binds it to every record
//! before it. The newest link is published outside the database as the
//! audit head once that transaction commits (see [`crate::audit_head`]).

use uuid::Uuid;

use crate::StoreError;
use crate::audit_chain::{
	AuditEntryHash, AuditTargetRef, ChainedFields, entry_hash, target_reference,
};
use crate::audit_epoch::{counter_column, parse_counter};
use crate::audit_head::AuditHead;
use crate::records::ActorRecord;
use crate::transaction::WriteTransaction;

/// Most records one audit page returns. The audit records decisions rather
/// than activity, so a page this size covers a long stretch of a Plane.
pub const AUDIT_PAGE_LIMIT: usize = 256;

/// How much a decision could cost if it was not the one the owner intended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditRisk {
	/// Recorded so it can be reviewed; it widens nothing and destroys
	/// nothing.
	Routine,
	/// Widens trust, changes policy, or exposes state.
	Elevated,
	/// May destroy state that cannot be brought back from within Jet.
	Destructive,
}

/// What became of the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
	/// It was carried out.
	Succeeded,
	/// It was refused before anything changed.
	Denied,
	/// It was allowed but did not complete.
	Failed,
}

/// A Security audit record to append in the transaction that carries out
/// the decision it describes. The core owns the `target_kind` and
/// `decision` vocabularies; the store keeps them as bounded text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAuditRecord {
	/// Globally unique identity chosen by the caller.
	pub record_id: Uuid,
	/// When the decision was made.
	pub recorded_at_unix_ms: i64,
	/// The authenticated Actor the decision is attributed to.
	pub actor: ActorRecord,
	/// The durable kind spelling of what the decision was about, such as
	/// `account_binding`.
	pub target_kind: String,
	/// The target's own identity, when it has one.
	pub target_id: Option<String>,
	/// The durable spelling of the decision, such as `account.bound`.
	pub decision: String,
	/// How much the decision could cost.
	pub risk: AuditRisk,
	/// What became of it.
	pub outcome: AuditOutcome,
}

/// One recorded decision, with the chain link that binds it to the record
/// before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
	/// Plane-local position; never reused, even after retention removes the
	/// record that held it.
	pub sequence: u64,
	/// The authority epoch this record belongs to.
	pub epoch: u64,
	/// Globally unique identity.
	pub record_id: Uuid,
	/// When the decision was made.
	pub recorded_at_unix_ms: i64,
	/// The Plane that made it.
	pub plane_id: Uuid,
	/// The authenticated Actor it is attributed to.
	pub actor: ActorRecord,
	/// The durable kind spelling of what it was about.
	pub target_kind: String,
	/// The opaque identifier of that target, which the chain covers and
	/// which outlives the target itself.
	pub target_reference: AuditTargetRef,
	/// The target's own identity, while the Plane still keeps it.
	pub target_id: Option<String>,
	/// The durable spelling of the decision.
	pub decision: String,
	/// How much it could cost.
	pub risk: AuditRisk,
	/// What became of it.
	pub outcome: AuditOutcome,
	/// The chain link this record folded to.
	pub entry_hash: AuditEntryHash,
}

/// Where the audit chain has reached inside the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditTip {
	/// The authority epoch the newest record belongs to.
	pub epoch: u64,
	/// Its position.
	pub sequence: u64,
	/// The chain link it folded to.
	pub entry_hash: AuditEntryHash,
}

/// The record retention last removed, whose link the remaining chain
/// continues from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RetentionAnchor {
	pub(crate) epoch: u64,
	pub(crate) sequence: u64,
	pub(crate) entry_hash: AuditEntryHash,
}

impl WriteTransaction {
	/// Appends `record` to the Security audit and returns it as stored.
	///
	/// The record chains onto the newest one in its epoch, and the head it
	/// produces is published outside the database once this transaction
	/// commits. A caller therefore records a decision by appending it in
	/// the same transaction as the change it describes; there is no way to
	/// commit one without the other.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the chain tip cannot be read or the
	/// row cannot be written, including when a value exceeds its bound.
	pub async fn append_audit_record(
		&mut self,
		record: NewAuditRecord,
	) -> Result<AuditRecord, StoreError> {
		let plane_id = self.plane().await?.plane_id;
		let epoch =
			self.current_audit_epoch(record.recorded_at_unix_ms).await?;
		let previous = match self.audit_tip().await? {
			Some(tip) if tip.epoch == epoch.epoch => tip.entry_hash,
			// The first record of an epoch follows the epoch's own genesis,
			// which covers the gap that epoch recorded.
			Some(_) | None => epoch.genesis(plane_id),
		};
		let sequence = self.take_audit_sequence().await?;
		let mut stored = AuditRecord {
			sequence,
			epoch: epoch.epoch,
			record_id: record.record_id,
			recorded_at_unix_ms: record.recorded_at_unix_ms,
			plane_id,
			actor: record.actor,
			target_reference: target_reference(
				plane_id,
				&record.target_kind,
				record.target_id.as_deref(),
			),
			target_kind: record.target_kind,
			target_id: record.target_id,
			decision: record.decision,
			risk: record.risk,
			outcome: record.outcome,
			// Replaced immediately below; the link covers every field
			// beside it, so it cannot be computed before they are all here.
			entry_hash: AuditEntryHash([0; 32]),
		};
		stored.entry_hash = chain_link(previous, &stored);
		self.insert_audit_row(&stored).await?;
		self.publish_audit_head(AuditHead {
			epoch: stored.epoch,
			sequence: stored.sequence,
			entry_hash: stored.entry_hash,
		});
		Ok(stored)
	}

	async fn insert_audit_row(
		&mut self,
		record: &AuditRecord,
	) -> Result<(), StoreError> {
		let (actor_kind, actor_id) = record.actor.columns();
		let actor_id = actor_id.to_string();
		let sequence = counter_column(record.sequence)?;
		let epoch = counter_column(record.epoch)?;
		let record_id = record.record_id.to_string();
		let plane_id = record.plane_id.to_string();
		let reference = record.target_reference.0.to_vec();
		let hash = record.entry_hash.0.to_vec();
		let risk = record.risk.as_str();
		let outcome = record.outcome.as_str();
		// ASVS 1.2.4: SQL structure is static; every dynamic value in this
		// module is passed through SQLite parameters.
		sqlx::query!(
			"INSERT INTO security_audit (sequence, epoch, record_id,
				recorded_at_unix_ms, plane_id, actor_kind, actor_id,
				target_kind, target_reference, target_id, decision, risk,
				outcome, entry_hash)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
				?14)",
			sequence,
			epoch,
			record_id,
			record.recorded_at_unix_ms,
			plane_id,
			actor_kind,
			actor_id,
			record.target_kind,
			reference,
			record.target_id,
			record.decision,
			risk,
			outcome,
			hash
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Claims the next audit position. Positions come from the audit's own
	/// counter rather than from the rows, so retention removing the oldest
	/// records can never hand a position out twice.
	async fn take_audit_sequence(&mut self) -> Result<u64, StoreError> {
		let assigned = sqlx::query_scalar!(
			r#"UPDATE audit_state SET next_sequence = next_sequence + 1
			 WHERE singleton = 1
			 RETURNING next_sequence - 1 AS "assigned!: i64""#
		)
		.fetch_one(self.connection())
		.await?;
		parse_counter(assigned)
	}
}

/// The chain link a record folds to when it follows `previous`.
///
/// The record's own `entry_hash` takes no part: this is what that field is
/// supposed to hold, computed from everything beside it.
pub(crate) fn chain_link(
	previous: AuditEntryHash,
	record: &AuditRecord,
) -> AuditEntryHash {
	let (actor_kind, actor_id) = record.actor.columns();
	let actor_id = actor_id.to_string();
	entry_hash(
		previous,
		&ChainedFields {
			sequence: record.sequence,
			epoch: record.epoch,
			record_id: record.record_id,
			recorded_at_unix_ms: record.recorded_at_unix_ms,
			plane_id: record.plane_id,
			actor_kind,
			actor_id: &actor_id,
			target_kind: &record.target_kind,
			target_reference: record.target_reference,
			decision: &record.decision,
			risk: record.risk.as_str(),
			outcome: record.outcome.as_str(),
		},
	)
}

/// Whether `record` still carries the identity its opaque target reference
/// was derived from. A record whose identity has been cleared by deletion
/// has nothing left to disagree with (ADR-0105).
pub(crate) fn target_matches_reference(record: &AuditRecord) -> bool {
	match &record.target_id {
		None => true,
		Some(id) => {
			target_reference(record.plane_id, &record.target_kind, Some(id))
				== record.target_reference
		}
	}
}

impl AuditRisk {
	/// The durable spelling, also used in JSON.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Routine => "routine",
			Self::Elevated => "elevated",
			Self::Destructive => "destructive",
		}
	}

	pub(crate) fn parse(text: &str) -> Option<Self> {
		[Self::Routine, Self::Elevated, Self::Destructive]
			.into_iter()
			.find(|risk| risk.as_str() == text)
	}
}

impl AuditOutcome {
	/// The durable spelling, also used in JSON.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Succeeded => "succeeded",
			Self::Denied => "denied",
			Self::Failed => "failed",
		}
	}

	pub(crate) fn parse(text: &str) -> Option<Self> {
		[Self::Succeeded, Self::Denied, Self::Failed]
			.into_iter()
			.find(|outcome| outcome.as_str() == text)
	}
}

#[cfg(test)]
#[path = "audit_tests.rs"]
mod tests;
