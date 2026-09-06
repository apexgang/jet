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
	Store, WriteTransaction,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::account::AccountBindingId;
use crate::command::Command;
use crate::conversation::ConversationId;
use crate::error::CoreError;
use crate::paired_client;
use crate::pairing::{self, PairingOfferId};
use crate::setting::{self, SettingKey, SettingScope, SettingValue};
use crate::{Actor, ClientId, PlaneId, ProjectId, system_time};

/// Most records one `Query::SecurityAudit` page returns.
pub(crate) const AUDIT_PAGE_LIMIT: usize = jet_store::AUDIT_PAGE_LIMIT;

/// Milliseconds in one day, which is the unit the retention window is set
/// in.
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// A position in this Plane's Security audit. Positions are never reused,
/// including by the records retention has removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AuditSequence(pub u64);

/// One authority epoch of the audit chain. It changes only when an owner
/// explicitly carries on past an integrity failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AuditEpoch(pub u64);

/// Durable identity of one audit record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuditRecordId(pub Uuid);

/// A decision worth recording. Each variant is one thing that can be
/// decided, spelled so a person reading the audit a year later still knows
/// what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditDecision {
	/// A remote connection attempted to prove its Paired Client identity.
	ConnectionAuthenticated,
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
	/// The Plane changed how long it keeps this audit (ADR-0105).
	AuditRetentionChanged,
	/// The Plane went back to keeping it for the built-in window.
	AuditRetentionCleared,
	/// An owner carried on past an integrity failure, beginning an
	/// authority epoch that records the gap it leaves behind.
	AuditEpochBegun,
	/// The Plane began accepting new Pairings, so a GUI client that holds a
	/// current pairing code may take control of it (ADR-0017).
	PairingGateOpened,
	/// It stopped accepting them. The clients already Paired are unaffected.
	PairingGateClosed,
	/// The Plane issued a Pairing offer, so a client that presents its
	/// secret in the next two minutes can take control of it (ADR-0017).
	PairingOffered,
	/// A client presented that secret and its durable public key.
	PairingClaimed,
	/// An offer was killed after too many wrong secrets, which is what an
	/// attempt to guess one looks like.
	PairingOfferInvalidated,
	/// The person at the target agreed that both screens showed the same
	/// authentication string.
	PairingConfirmed,
	/// A Pairing completed, so a GUI client now controls this Plane with
	/// full trust (ADR-0017).
	PairingCompleted,
	/// A Paired client was allowed to control this Plane again.
	PairedClientEnabled,
	/// A Paired client was stopped from controlling it. The Plane keeps its
	/// key, so this is not the end of the pairing.
	PairedClientDisabled,
	/// A Paired client and its key were forgotten. Nothing in Jet brings
	/// either back: the installation pairs again or it does not control
	/// this Plane.
	PairedClientRevoked,
	/// An interactive user granted Jet a directory as a Project, widening
	/// what it may read and change on this Plane (ADR-0101).
	ProjectRegistered,
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
	/// One Pairing offer.
	PairingOffer(PairingOfferId),
	/// One Paired client.
	PairedClient(ClientId),
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

impl Decision {
	/// A decision that was carried out. It is the only outcome a Command
	/// records from inside its own transaction, because a Command that
	/// failed never reaches the commit that would have kept the record.
	pub(crate) fn succeeded(
		decision: AuditDecision,
		subject: AuditSubject,
	) -> Self {
		Self {
			decision,
			subject,
			outcome: AuditOutcome::Succeeded,
		}
	}
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
			Self::ConnectionAuthenticated => "connection.authenticated",
			Self::AccountBound => "account.bound",
			Self::AccountUnbound => "account.unbound",
			Self::GitAutomationEnabled => "policy.git_automation_enabled",
			Self::GitAutomationDisabled => "policy.git_automation_disabled",
			Self::GitAutomationCleared => "policy.git_automation_cleared",
			Self::AuditRetentionChanged => "policy.audit_retention_changed",
			Self::AuditRetentionCleared => "policy.audit_retention_cleared",
			Self::AuditEpochBegun => "audit.epoch_begun",
			Self::PairingGateOpened => "pairing.gate_opened",
			Self::PairingGateClosed => "pairing.gate_closed",
			Self::PairingOffered => "pairing.offered",
			Self::PairingClaimed => "pairing.claimed",
			Self::PairingOfferInvalidated => "pairing.offer_invalidated",
			Self::PairingConfirmed => "pairing.confirmed",
			Self::PairingCompleted => "pairing.completed",
			Self::PairedClientEnabled => "pairing.client_enabled",
			Self::PairedClientDisabled => "pairing.client_disabled",
			Self::PairedClientRevoked => "pairing.client_revoked",
			Self::ProjectRegistered => "project.registered",
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
			Self::ConnectionAuthenticated => AuditRisk::Routine,
			Self::AccountBound
			| Self::AccountUnbound
			| Self::GitAutomationEnabled
			// Unpinning a policy hands the choice back to the scope above,
			// which may turn it on.
			| Self::GitAutomationCleared
			// Beginning an epoch is how a Plane stops vouching for
			// everything before it.
			| Self::AuditEpochBegun
			// An open gate is the window in which an unknown client can
			// come to control the Plane, and an offer is that window
			// standing open with a secret in it.
			| Self::PairingGateOpened
			| Self::PairingOffered
			| Self::PairingClaimed
			| Self::PairingOfferInvalidated
			| Self::PairingConfirmed
			| Self::PairingCompleted
			| Self::PairedClientEnabled
			// A Path grant is the one way a directory comes under Jet's
			// management, and everything a Run does there follows from it.
			| Self::ProjectRegistered => AuditRisk::Elevated,
			// Revoking destroys the key the pairing was, and no part of Jet
			// can put it back: the installation pairs again or it does not
			// control this Plane.
			Self::PairedClientRevoked => AuditRisk::Destructive,
			// Shortening the window destroys evidence the Plane already
			// holds, which is the one policy change the audit itself is at
			// stake in.
			Self::AuditRetentionChanged
			| Self::AuditRetentionCleared => AuditRisk::Destructive,
			Self::GitAutomationDisabled
			| Self::PairingGateClosed
			// Stopping a client is the safe direction, and its key stays
			// where it is.
			| Self::PairedClientDisabled => AuditRisk::Routine,
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
			Self::PairingOffer(_) => "pairing_offer",
			Self::PairedClient(_) => "paired_client",
		}
	}

	fn identity(self) -> Option<String> {
		match self {
			Self::Plane => None,
			Self::Project(ProjectId(id))
			| Self::Conversation(ConversationId(id))
			| Self::AccountBinding(AccountBindingId(id))
			| Self::PairingOffer(PairingOfferId(id))
			| Self::PairedClient(ClientId(id)) => Some(id.to_string()),
		}
	}
}

/// What `command` would record, when it is one the audit records at all.
///
/// This is the single place that decides whether a Command is the audit's
/// business, so what the audit writes and what Security-degraded mode
/// guards can never drift apart.
pub(crate) fn decision_for(command: &Command) -> Option<AuditDecision> {
	match command {
		Command::BindAccount { .. } => Some(AuditDecision::AccountBound),
		Command::UnbindAccount { .. } => Some(AuditDecision::AccountUnbound),
		Command::SetSetting { key, value, .. } => stored_setting(*key, value),
		Command::ClearSetting { key, .. } => cleared_setting(*key),
		Command::SetPairingGate { gate } => Some(pairing::gate_decision(*gate)),
		Command::OpenPairing { .. } => Some(AuditDecision::PairingOffered),
		Command::ClaimPairing { .. } => Some(AuditDecision::PairingClaimed),
		Command::ConfirmPairing { .. } => Some(AuditDecision::PairingConfirmed),
		Command::CompletePairing { .. } => {
			Some(AuditDecision::PairingCompleted)
		}
		Command::SetPairedClientAccess { access, .. } => {
			Some(paired_client::access_decision(*access))
		}
		Command::RevokePairedClient { .. } => {
			Some(AuditDecision::PairedClientRevoked)
		}
		Command::RegisterProject { .. } => {
			Some(AuditDecision::ProjectRegistered)
		}
		Command::BeginAuditEpoch
		| Command::CreateConversation { .. }
		| Command::CreateRun { .. }
		| Command::PromoteWorkspace { .. }
		| Command::TransitionRun { .. } => None,
	}
}

/// What a Command that never ran was about.
///
/// A binding that was not made has no identity yet, and neither has a
/// Project that was not registered, so a refused one is recorded against
/// the Plane it was refused on. Everything else already names something
/// that exists.
fn refused_subject(command: &Command) -> AuditSubject {
	match command {
		Command::UnbindAccount { binding_id } => {
			AuditSubject::AccountBinding(*binding_id)
		}
		Command::SetPairedClientAccess { client_id, .. }
		| Command::RevokePairedClient { client_id } => {
			AuditSubject::PairedClient(*client_id)
		}
		Command::SetSetting { scope, .. }
		| Command::ClearSetting { scope, .. } => AuditSubject::of_scope(*scope),
		Command::BindAccount { .. }
		| Command::RegisterProject { .. }
		| Command::PromoteWorkspace { .. }
		| Command::BeginAuditEpoch
		| Command::SetPairingGate { .. }
		| Command::OpenPairing { .. }
		| Command::ClaimPairing { .. }
		| Command::ConfirmPairing { .. }
		| Command::CompletePairing { .. }
		| Command::CreateConversation { .. }
		| Command::CreateRun { .. }
		| Command::TransitionRun { .. } => AuditSubject::Plane,
	}
}

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
		(SettingKey::SecurityAuditRetentionDays, SettingValue::Count(_)) => {
			Some(AuditDecision::AuditRetentionChanged)
		}
		(
			SettingKey::GitAutoCommit,
			SettingValue::Text(_) | SettingValue::Count(_),
		)
		| (
			SettingKey::SecurityAuditRetentionDays,
			SettingValue::Flag(_) | SettingValue::Text(_),
		)
		| (SettingKey::UtilityAutomaticNaming, _)
		| (SettingKey::GitMessageInstructions, _) => None,
	}
}

/// What one scope giving up its own value for `key` decides.
pub(crate) fn cleared_setting(key: SettingKey) -> Option<AuditDecision> {
	match key {
		SettingKey::GitAutoCommit => Some(AuditDecision::GitAutomationCleared),
		SettingKey::SecurityAuditRetentionDays => {
			Some(AuditDecision::AuditRetentionCleared)
		}
		SettingKey::UtilityAutomaticNaming
		| SettingKey::GitMessageInstructions => None,
	}
}

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
