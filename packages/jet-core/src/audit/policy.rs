//! Selects audit subjects and decisions for Commands and Settings.

use super::{AuditDecision, AuditSubject};
use crate::{
	command::Command,
	pairing::{self, paired_client},
	setting::{SettingKey, SettingValue},
};

/// What `command` would record, when it is one the audit records at all.
///
/// This is the single place that decides whether a Command is the audit's
/// business, so what the audit writes and what Security-degraded mode
/// guards can never drift apart.
pub(crate) fn decision_for(command: &Command) -> Option<AuditDecision> {
	match command {
		Command::SetAutoContinue { .. } => {
			Some(AuditDecision::AutoContinuePolicyChanged)
		}
		Command::AuthorizeApprovalRetry { .. } => {
			Some(AuditDecision::ApprovalRetryAuthorized)
		}
		Command::ReviewRemoteTool { .. } => {
			Some(AuditDecision::RemoteToolReviewed)
		}
		Command::ChangeExtension { confirmation } => {
			Some(crate::extension::work::decision(confirmation.action))
		}
		Command::DisableCraft { .. } => Some(AuditDecision::CraftDisabled),
		Command::InstallCraft { .. } => {
			Some(AuditDecision::CraftInstallationApproved)
		}
		Command::OpenTerminal { .. } => Some(AuditDecision::TerminalOpened),
		Command::CloseTerminal { .. } => Some(AuditDecision::TerminalClosed),
		Command::ResolveExecution(_) => {
			Some(AuditDecision::ExecutionResolutionRequested)
		}
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
		| Command::ApplyUserEdit { .. }
		| Command::SetConversationName { .. }
		| Command::SetRunName { .. }
		| Command::CreateConversation { .. }
		| Command::ForkConversation { .. }
		| Command::HandoffConversation(_)
		| Command::CreateRun { .. }
		| Command::StartRun { .. }
		| Command::StartVisaRun(_)
		| Command::StartNoVisaRun(_)
		| Command::AcknowledgeGitDelivery { .. }
		| Command::DeliverGit { .. }
		| Command::RequestUtility { .. }
		| Command::CreateSchedule { .. }
		| Command::CancelSchedule { .. }
		| Command::SubmitTurn { .. }
		| Command::SubmitReview { .. }
		| Command::WithdrawTurn { .. }
		| Command::ControlRun { .. }
		| Command::PromoteWorkspace { .. }
		| Command::ImportConversation { .. }
		| Command::ResumeImportedConversation { .. }
		| Command::TransitionRun { .. } => None,
	}
}

/// What a Command that never ran was about.
///
/// A binding that was not made has no identity yet, and neither has a
/// Project that was not registered, so a refused one is recorded against
/// the Plane it was refused on. Everything else already names something
/// that exists.
pub(super) fn refused_subject(command: &Command) -> AuditSubject {
	match command {
		Command::SetAutoContinue { target, .. } => {
			auto_continue_subject(*target)
		}
		Command::AuthorizeApprovalRetry { run_id, .. } => {
			AuditSubject::Execution(*run_id)
		}
		Command::ReviewRemoteTool { .. } => AuditSubject::Plane,
		Command::ChangeExtension { .. } => AuditSubject::Plane,
		Command::DisableCraft { craft_id, .. } => {
			AuditSubject::Craft(craft_id.clone())
		}
		Command::InstallCraft { confirmation } => {
			AuditSubject::Craft(refused_craft_identity(confirmation))
		}
		Command::OpenTerminal { .. } => AuditSubject::Plane,
		Command::CloseTerminal { terminal_id } => {
			AuditSubject::Terminal(*terminal_id)
		}
		Command::ResolveExecution(request) => {
			AuditSubject::Execution(request.execution_id)
		}
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
		| Command::ApplyUserEdit { .. }
		| Command::RegisterProject { .. }
		| Command::PromoteWorkspace { .. }
		| Command::BeginAuditEpoch
		| Command::SetPairingGate { .. }
		| Command::OpenPairing { .. }
		| Command::ClaimPairing { .. }
		| Command::ConfirmPairing { .. }
		| Command::CompletePairing { .. }
		| Command::SetConversationName { .. }
		| Command::SetRunName { .. }
		| Command::CreateConversation { .. }
		| Command::ForkConversation { .. }
		| Command::HandoffConversation(_)
		| Command::CreateRun { .. }
		| Command::StartRun { .. }
		| Command::StartVisaRun(_)
		| Command::StartNoVisaRun(_)
		| Command::AcknowledgeGitDelivery { .. }
		| Command::DeliverGit { .. }
		| Command::RequestUtility { .. }
		| Command::CreateSchedule { .. }
		| Command::CancelSchedule { .. }
		| Command::SubmitTurn { .. }
		| Command::SubmitReview { .. }
		| Command::WithdrawTurn { .. }
		| Command::ControlRun { .. }
		| Command::ImportConversation { .. }
		| Command::ResumeImportedConversation { .. }
		| Command::TransitionRun { .. } => AuditSubject::Plane,
	}
}

pub(super) fn refused_craft_identity(
	confirmation: &crate::CraftInstallationConfirmation,
) -> String {
	if let crate::CraftSource::GitHubRelease { repository, .. } =
		&confirmation.source
		&& let Some(name) = repository.split_once('/').map(|(_, name)| name)
		&& let Some(id) = name.strip_prefix("jet-craft-")
		&& !id.is_empty()
		&& id.len() <= 80
		&& id.bytes().all(|byte| {
			byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
		}) {
		return id.to_owned();
	}
	if crate::craft::installation::is_sha256(&confirmation.artifact_sha256) {
		confirmation.artifact_sha256.clone()
	} else {
		"unidentified".into()
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
		(
			SettingKey::UtilityGitText
			| SettingKey::UtilityContentConsent
			| SettingKey::UtilityAccountBinding
			| SettingKey::UtilityAutodeleteCompilation,
			_,
		) => Some(AuditDecision::UtilityPolicyChanged),
		(
			SettingKey::AutomaticReview
			| SettingKey::AutomaticReviewBinding
			| SettingKey::AutomaticReviewConsent,
			_,
		) => Some(AuditDecision::ReviewPolicyChanged),
		(
			SettingKey::GitAutoCommit
			| SettingKey::GitAutoBranch
			| SettingKey::GitAutoPush
			| SettingKey::GitAutoDraftPullRequest,
			SettingValue::Flag(true),
		) => Some(AuditDecision::GitAutomationEnabled),
		(
			SettingKey::GitAutoCommit
			| SettingKey::GitAutoBranch
			| SettingKey::GitAutoPush
			| SettingKey::GitAutoDraftPullRequest,
			SettingValue::Flag(false),
		) => Some(AuditDecision::GitAutomationDisabled),
		(SettingKey::SecurityAuditRetentionDays, SettingValue::Count(_)) => {
			Some(AuditDecision::AuditRetentionChanged)
		}
		(SettingKey::DeveloperMode, SettingValue::Flag(true)) => {
			Some(AuditDecision::DeveloperModeEnabled)
		}
		(SettingKey::DeveloperMode, SettingValue::Flag(false)) => {
			Some(AuditDecision::DeveloperModeDisabled)
		}
		(
			SettingKey::GitAutoCommit
			| SettingKey::GitAutoBranch
			| SettingKey::GitAutoPush
			| SettingKey::GitAutoDraftPullRequest,
			SettingValue::Text(_) | SettingValue::Count(_),
		)
		| (
			SettingKey::SecurityAuditRetentionDays,
			SettingValue::Flag(_) | SettingValue::Text(_),
		)
		| (
			SettingKey::DeveloperMode,
			SettingValue::Text(_) | SettingValue::Count(_),
		)
		| (
			SettingKey::EnergyConcurrency
			| SettingKey::EnergyLowPowerConcurrency
			| SettingKey::EnergyConstrained
			| SettingKey::EnergyForegroundOverride,
			_,
		)
		| (SettingKey::UtilityAutomaticNaming, _)
		| (
			SettingKey::StorageDisposableMiB
			| SettingKey::ArtifactMaxMiB
			| SettingKey::ArtifactRunMiB,
			_,
		)
		| (
			SettingKey::GitMessageInstructions | SettingKey::GitBranchPrefix,
			_,
		) => None,
	}
}

/// What one scope giving up its own value for `key` decides.
pub(crate) fn cleared_setting(key: SettingKey) -> Option<AuditDecision> {
	match key {
		SettingKey::UtilityGitText
		| SettingKey::UtilityContentConsent
		| SettingKey::UtilityAccountBinding
		| SettingKey::UtilityAutodeleteCompilation => {
			Some(AuditDecision::UtilityPolicyChanged)
		}
		SettingKey::AutomaticReview
		| SettingKey::AutomaticReviewBinding
		| SettingKey::AutomaticReviewConsent => {
			Some(AuditDecision::ReviewPolicyChanged)
		}
		SettingKey::GitAutoCommit
		| SettingKey::GitAutoBranch
		| SettingKey::GitAutoPush
		| SettingKey::GitAutoDraftPullRequest => {
			Some(AuditDecision::GitAutomationCleared)
		}
		SettingKey::SecurityAuditRetentionDays => {
			Some(AuditDecision::AuditRetentionCleared)
		}
		SettingKey::DeveloperMode => Some(AuditDecision::DeveloperModeCleared),
		SettingKey::EnergyConcurrency
		| SettingKey::EnergyLowPowerConcurrency
		| SettingKey::EnergyConstrained
		| SettingKey::EnergyForegroundOverride
		| SettingKey::UtilityAutomaticNaming
		| SettingKey::ArtifactMaxMiB
		| SettingKey::ArtifactRunMiB
		| SettingKey::StorageDisposableMiB
		| SettingKey::GitMessageInstructions
		| SettingKey::GitBranchPrefix => None,
	}
}

pub(crate) fn auto_continue_subject(
	target: crate::AutoContinueTarget,
) -> AuditSubject {
	match target {
		crate::AutoContinueTarget::AccountBinding(id) => {
			AuditSubject::AccountBinding(id)
		}
		crate::AutoContinueTarget::Conversation(id) => {
			AuditSubject::Conversation(id)
		}
	}
}
