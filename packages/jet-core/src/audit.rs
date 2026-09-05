//! The owner-only Security audit as the core records and reports it
//! (ADR-0105).
//!
//! The audit is not the Event journal. The journal is Conversation history
//! that clients subscribe to and that compaction may thin (ADR-0078); this
//! is a separate, narrower record of the decisions that widen trust, change
//! policy, or destroy state, kept so an owner can answer who did what and
//! when. It is owner-only because reaching a Plane at all is owner-only
//! (ADR-0087).
//!
//! Nothing here can carry a credential, a prompt, terminal output, or file
//! content. A record names a subject and a decision from closed
//! vocabularies this core owns, and the identity it stores is a name for
//! something Jet already keeps — never a value somebody typed.

use std::time::SystemTime;

use jet_store::{
	AuditOutcome, AuditRecord, AuditRisk, AuditTargetRef, NewAuditRecord,
	WriteTransaction,
};
use uuid::Uuid;

use crate::account::AccountBindingId;
use crate::conversation::ConversationId;
use crate::error::CoreError;
use crate::setting::{SettingKey, SettingScope, SettingValue};
use crate::{Actor, PlaneId, ProjectId, system_time};

/// Most records one `Query::SecurityAudit` page returns.
pub(crate) const AUDIT_PAGE_LIMIT: usize = jet_store::AUDIT_PAGE_LIMIT;

/// A position in this Plane's Security audit. Positions are never reused,
/// including by the records retention has removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AuditSequence(pub u64);

/// One authority epoch of the audit chain. It changes only when an owner
/// explicitly carries on past an integrity failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuditEpoch(pub u64);

/// Durable identity of one audit record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuditRecordId(pub Uuid);

/// A decision worth recording. Each variant is one thing that can be
/// decided, spelled so a person reading the audit a year later still knows
/// what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditDecision {
	/// A Provider account was bound to this Plane, widening what may
	/// authenticate on it (ADR-0016).
	AccountBound,
	/// An Account binding was removed.
	AccountUnbound,
	/// Automatic Git delivery was turned on for a scope, letting Jet commit
	/// Harness changes there without being asked (ADR-0029).
	GitAutomationEnabled,
	/// It was turned off for that scope.
	GitAutomationDisabled,
	/// A scope stopped pinning it, so the scope above decides again.
	GitAutomationCleared,
}

/// What a decision is about. The core turns each one into the durable kind
/// and identity the store keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuditSubject {
	/// The Plane as a whole.
	Plane,
	/// One registered Project.
	Project(ProjectId),
	/// One Conversation.
	Conversation(ConversationId),
	/// One Account binding.
	AccountBinding(AccountBindingId),
}

/// One decision to record beside the change that carried it out.
pub(crate) struct Decision {
	/// What was decided.
	pub(crate) decision: AuditDecision,
	/// What it was about.
	pub(crate) subject: AuditSubject,
	/// What became of it.
	pub(crate) outcome: AuditOutcome,
}

/// What one recorded decision was about.
///
/// The kind is the durable spelling rather than a closed enum, and the
/// identity is text rather than a parsed one: a core reads an audit that a
/// newer core may have written, and a record it cannot name is still a
/// record it has to show (ADR-0073, ADR-0094).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditTarget {
	/// The durable kind spelling, such as `account_binding`.
	pub kind: String,
	/// The opaque identifier the integrity chain covers. It is derived from
	/// the target, so it outlives the target itself and groups the records
	/// about one thing together after that thing is gone.
	pub reference: AuditTargetRef,
	/// The target's own identity, while the Plane still keeps the target.
	/// Deleting the target clears it and leaves the reference (ADR-0105).
	pub identity: Option<String>,
}

/// One recorded decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
	/// Where it sits in this Plane's audit.
	pub sequence: AuditSequence,
	/// The authority epoch it belongs to.
	pub epoch: AuditEpoch,
	/// Durable identity.
	pub record_id: AuditRecordId,
	/// When the decision was made.
	pub recorded_at: SystemTime,
	/// The Plane that made it.
	pub plane_id: PlaneId,
	/// The authenticated Actor it is attributed to.
	pub actor: Actor,
	/// What it was about.
	pub target: AuditTarget,
	/// The durable spelling of what was decided, such as `account.bound`.
	pub decision: String,
	/// How much it could cost, as judged when it was made.
	pub risk: AuditRisk,
	/// What became of it.
	pub outcome: AuditOutcome,
}

/// One page of the Security audit, fenced by the position the audit had
/// reached when the page was read. The page is the last one when its final
/// record's sequence equals `cursor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditPage {
	/// Newest audit position when the page was read.
	pub cursor: AuditSequence,
	/// The records strictly after the requested position, oldest first.
	pub entries: Vec<AuditEntry>,
}

impl AuditDecision {
	/// The durable spelling, also used in the audit and on the wire.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::AccountBound => "account.bound",
			Self::AccountUnbound => "account.unbound",
			Self::GitAutomationEnabled => "policy.git_automation_enabled",
			Self::GitAutomationDisabled => "policy.git_automation_disabled",
			Self::GitAutomationCleared => "policy.git_automation_cleared",
		}
	}

	/// How much this decision could cost if it was not the one the owner
	/// intended.
	///
	/// The judgement is made here and then stored, so a record keeps the
	/// risk the Plane assigned when the decision was made rather than the
	/// one a later release would assign.
	fn risk(self) -> AuditRisk {
		match self {
			Self::AccountBound
			| Self::AccountUnbound
			| Self::GitAutomationEnabled
			// Unpinning a policy hands the choice back to the scope above,
			// which may turn it on.
			| Self::GitAutomationCleared => AuditRisk::Elevated,
			Self::GitAutomationDisabled => AuditRisk::Routine,
		}
	}
}

impl AuditSubject {
	/// The subject a Setting decision is about: the scope that stores the
	/// value, which is the thing the policy now applies differently to.
	pub(crate) fn of_scope(scope: SettingScope) -> Self {
		match scope {
			SettingScope::Plane => Self::Plane,
			SettingScope::Project { project_id } => Self::Project(project_id),
			SettingScope::Conversation { conversation_id } => {
				Self::Conversation(conversation_id)
			}
		}
	}

	fn kind(self) -> &'static str {
		match self {
			Self::Plane => "plane",
			Self::Project(_) => "project",
			Self::Conversation(_) => "conversation",
			Self::AccountBinding(_) => "account_binding",
		}
	}

	fn identity(self) -> Option<String> {
		match self {
			Self::Plane => None,
			Self::Project(ProjectId(id))
			| Self::Conversation(ConversationId(id))
			| Self::AccountBinding(AccountBindingId(id)) => Some(id.to_string()),
		}
	}
}

/// What storing `value` for `key` decides, when that Setting is one the
/// audit is for. Most Settings are preferences; these are the ones that
/// change what Jet may do on its own.
pub(crate) fn stored_setting(
	key: SettingKey,
	value: &SettingValue,
) -> Option<AuditDecision> {
	match (key, value) {
		(SettingKey::GitAutoCommit, SettingValue::Flag(true)) => {
			Some(AuditDecision::GitAutomationEnabled)
		}
		(SettingKey::GitAutoCommit, SettingValue::Flag(false)) => {
			Some(AuditDecision::GitAutomationDisabled)
		}
		(SettingKey::GitAutoCommit, SettingValue::Text(_))
		| (SettingKey::UtilityAutomaticNaming, _)
		| (SettingKey::GitMessageInstructions, _) => None,
	}
}

/// What one scope giving up its own value for `key` decides.
pub(crate) fn cleared_setting(key: SettingKey) -> Option<AuditDecision> {
	match key {
		SettingKey::GitAutoCommit => Some(AuditDecision::GitAutomationCleared),
		SettingKey::UtilityAutomaticNaming
		| SettingKey::GitMessageInstructions => None,
	}
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
		actor: actor.record(),
		target_kind: decision.subject.kind().into(),
		target_id: decision.subject.identity(),
		decision: decision.decision.as_str().into(),
		risk: decision.decision.risk(),
		outcome: decision.outcome,
	})
	.await?;
	Ok(())
}

impl From<AuditRecord> for AuditEntry {
	fn from(record: AuditRecord) -> Self {
		Self {
			sequence: AuditSequence(record.sequence),
			epoch: AuditEpoch(record.epoch),
			record_id: AuditRecordId(record.record_id),
			recorded_at: system_time(record.recorded_at_unix_ms),
			plane_id: PlaneId(record.plane_id),
			actor: Actor::from_record(record.actor),
			target: AuditTarget {
				kind: record.target_kind,
				reference: record.target_reference,
				identity: record.target_id,
			},
			decision: record.decision,
			risk: record.risk,
			outcome: record.outcome,
		}
	}
}
