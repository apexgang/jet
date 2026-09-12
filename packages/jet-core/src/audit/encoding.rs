//! Converts audit decisions and stored audit records.

use super::{
	AuditDecision, AuditEntry, AuditEpoch, AuditRecordId, AuditSequence,
	AuditSubject, AuditTarget,
};
use crate::{
	ClientId, PlaneId, ProjectId, account::AccountBindingId,
	conversation::ConversationId, pairing::PairingOfferId,
	setting::SettingScope, system_time,
};
use jet_store::{AuditRecord, AuditRisk};

impl AuditDecision {
	/// The durable spelling, also used in the audit and on the wire.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::RemoteToolReviewed => "remote.reviewed",
			Self::ExecutionResolutionRequested => {
				"execution.resolution_requested"
			}
			Self::AutoContinuePolicyChanged => "policy.auto_continue_changed",
			Self::UtilityPolicyChanged => "policy.utility_changed",
			Self::TerminalOpened => "terminal.opened",
			Self::TerminalClosed => "terminal.closed",
			Self::TerminalInput => "terminal.input",
			Self::ConnectionAuthenticated => "connection.authenticated",
			Self::AccountBound => "account.bound",
			Self::AccountUnbound => "account.unbound",
			Self::GitAutomationEnabled => "policy.git_automation_enabled",
			Self::GitAutomationDisabled => "policy.git_automation_disabled",
			Self::GitAutomationCleared => "policy.git_automation_cleared",
			Self::AuditRetentionChanged => "policy.audit_retention_changed",
			Self::AuditRetentionCleared => "policy.audit_retention_cleared",
			Self::AuditEpochBegun => "audit.epoch_begun",
			Self::RecoverySnapshotRestored => "recovery.snapshot_restored",
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
			Self::CraftInstallationApproved => "craft.installation_approved",
			Self::ExtensionInstall => "extension.install",
			Self::ExtensionUpdate => "extension.update",
			Self::ExtensionDisable => "extension.disable",
			Self::ExtensionRemove => "extension.remove",
			Self::CraftDisabled => "craft.disabled",
			Self::DeveloperModeEnabled => "craft.developer_mode_enabled",
			Self::DeveloperModeDisabled => "craft.developer_mode_disabled",
			Self::DeveloperModeCleared => "craft.developer_mode_cleared",
			Self::ApprovalReviewed => "approval.reviewed",
			Self::ApprovalRetryAuthorized => "approval.retry_authorized",
			Self::ReviewPolicyChanged => "policy.review_changed",
		}
	}

	/// How much this decision could cost if it was not the one the owner
	/// intended.
	///
	/// The judgement is made here and then stored, so a record keeps the
	/// risk the Plane assigned when the decision was made rather than the
	/// one a later release would assign.
	pub(super) fn risk(self) -> AuditRisk {
		match self {
			// Answering for a person is exactly what a review does, so
			// every one of them is worth the same attention as the remote
			// action a person reviews by hand.
			Self::RemoteToolReviewed | Self::ApprovalReviewed | Self::ApprovalRetryAuthorized => AuditRisk::Elevated,
            Self::ExecutionResolutionRequested => AuditRisk::Destructive,
			Self::TerminalOpened | Self::TerminalClosed | Self::TerminalInput | Self::ConnectionAuthenticated => AuditRisk::Routine,
			Self::AutoContinuePolicyChanged
			| Self::UtilityPolicyChanged
			| Self::ReviewPolicyChanged
			| Self::AccountBound
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
			| Self::ProjectRegistered
			| Self::CraftDisabled
 | Self::ExtensionInstall | Self::ExtensionUpdate | Self::ExtensionDisable | Self::ExtensionRemove
 | Self::CraftInstallationApproved
			| Self::DeveloperModeEnabled => AuditRisk::Elevated,
			// Revoking destroys the key the pairing was, and no part of Jet
			// can put it back: the installation pairs again or it does not
			// control this Plane.
			Self::PairedClientRevoked => AuditRisk::Destructive,
			// Everything committed after the snapshot was taken is gone
			// from the store, deliberately; the damaged copy stays beside
			// it.
			Self::RecoverySnapshotRestored => AuditRisk::Destructive,
			// Shortening the window destroys evidence the Plane already
			// holds, which is the one policy change the audit itself is at
			// stake in.
			Self::AuditRetentionChanged
			| Self::AuditRetentionCleared => AuditRisk::Destructive,
			Self::GitAutomationDisabled
			| Self::PairingGateClosed
			| Self::DeveloperModeDisabled
			| Self::DeveloperModeCleared
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

	pub(super) fn kind(&self) -> &'static str {
		match self {
			Self::Extension(_) => "extension_change",
			Self::Craft(_) => "craft",
			Self::Terminal(_) => "terminal",
			Self::Execution(_) => "execution",
			Self::Plane => "plane",
			Self::Project(_) => "project",
			Self::Conversation(_) => "conversation",
			Self::AccountBinding(_) => "account_binding",
			Self::PairingOffer(_) => "pairing_offer",
			Self::PairedClient(_) => "paired_client",
		}
	}

	pub(super) fn identity(&self) -> Option<String> {
		match self {
			Self::Extension(id) => Some(id.to_string()),
			Self::Craft(id) => Some(id.clone()),
			Self::Terminal(crate::TerminalId(id)) => Some(id.to_string()),
			Self::Execution(crate::RunId(id)) => Some(id.to_string()),
			Self::Plane => None,
			Self::Project(ProjectId(id))
			| Self::Conversation(ConversationId(id))
			| Self::AccountBinding(AccountBindingId(id))
			| Self::PairingOffer(PairingOfferId(id))
			| Self::PairedClient(ClientId(id)) => Some(id.to_string()),
		}
	}
}

impl From<AuditRecord> for AuditEntry {
	fn from(record: AuditRecord) -> Self {
		Self {
			sequence: AuditSequence(record.sequence),
			epoch: AuditEpoch(record.epoch),
			record_id: AuditRecordId(record.record_id),
			recorded_at: system_time(record.recorded_at_unix_ms),
			plane_id: PlaneId(record.plane_id),
			actor: record.actor.into(),
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
