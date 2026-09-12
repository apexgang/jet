//! Minimum protocol versions required by Commands and Queries.

use super::wire_error;
use jet_protocol::{
	CommandRequest, ErrorCategory, QueryRequest, RequestId, ServerMessage,
	WorkingTreeRequest,
};

/// The protocol minor one request needs, named as the refusal spells it.
pub(super) struct MinorRequirement {
	pub(super) minor: u32,
	feature: &'static str,
}

pub(super) fn query_minor(query: &QueryRequest) -> Option<MinorRequirement> {
	match query {
		QueryRequest::RemoteToolReview { .. } => Some(MinorRequirement {
			minor: jet_protocol::NO_VISA_MINOR,
			feature: "No-Visa review",
		}),
		QueryRequest::Settings {
			selection:
				jet_protocol::SettingSelection::Key {
					key: jet_protocol::SettingKey::StorageDisposableMiB,
				},
			..
		} => Some(MinorRequirement {
			minor: jet_protocol::DISK_PRESSURE_MINOR,
			feature: "disposable storage policy",
		}),
		QueryRequest::Settings {
			selection:
				jet_protocol::SettingSelection::Key {
					key:
						jet_protocol::SettingKey::GitAutoBranch
						| jet_protocol::SettingKey::GitAutoPush
						| jet_protocol::SettingKey::GitAutoDraftPullRequest
						| jet_protocol::SettingKey::GitBranchPrefix,
				},
			..
		} => Some(MinorRequirement {
			minor: jet_protocol::GIT_DELIVERY_MINOR,
			feature: "Git delivery policy",
		}),
		QueryRequest::GitDeliveries { .. } => Some(MinorRequirement {
			minor: jet_protocol::GIT_DELIVERY_MINOR,
			feature: "Git delivery",
		}),
		QueryRequest::Utility { .. } => Some(MinorRequirement {
			minor: jet_protocol::UTILITY_MINOR,
			feature: "Utility work",
		}),
		QueryRequest::InspectExtension { .. }
		| QueryRequest::ExtensionCatalog { .. }
		| QueryRequest::ExtensionChange { .. } => Some(MinorRequirement {
			minor: jet_protocol::EXTENSIONS_MINOR,
			feature: "Harness extensions",
		}),
		QueryRequest::DiscoverCraft { .. } => Some(MinorRequirement {
			minor: jet_protocol::CRAFT_INSTALLATION_MINOR,
			feature: "third-party Craft discovery",
		}),
		QueryRequest::AutoContinue { .. } => Some(MinorRequirement {
			minor: jet_protocol::AUTO_CONTINUE_MINOR,
			feature: "Auto-continue",
		}),
		QueryRequest::ScheduledTasks { .. } => Some(MinorRequirement {
			minor: jet_protocol::SCHEDULES_MINOR,
			feature: "Scheduled tasks",
		}),
		QueryRequest::EditableFile { .. } => Some(MinorRequirement {
			minor: jet_protocol::USER_INPUT_MINOR,
			feature: "direct user edits",
		}),
		QueryRequest::WorkspaceTerminals { .. } => Some(MinorRequirement {
			minor: jet_protocol::WORKSPACE_TERMINALS_MINOR,
			feature: "Workspace terminals",
		}),
		QueryRequest::TurnQueue { .. } => Some(MinorRequirement {
			minor: jet_protocol::TURN_QUEUE_MINOR,
			feature: "Turn queue",
		}),
		QueryRequest::OrphanedExecutions { .. } => Some(MinorRequirement {
			minor: jet_protocol::EXECUTION_RECOVERY_MINOR,
			feature: "execution recovery",
		}),
		QueryRequest::ChangeDiff { .. }
		| QueryRequest::NextChangeDiff { .. }
		| QueryRequest::ChangeArtifact { .. } => Some(MinorRequirement {
			minor: jet_protocol::CHANGE_CHECKPOINTS_MINOR,
			feature: "Change checkpoints",
		}),
		QueryRequest::RunExecution { .. } => Some(MinorRequirement {
			minor: jet_protocol::MANAGED_RUNS_MINOR,
			feature: "managed Run Queries",
		}),
		QueryRequest::NextConversations { .. } => Some(MinorRequirement {
			minor: jet_protocol::FENCED_READS_MINOR,
			feature: "Conversation pagination",
		}),
		QueryRequest::Settings {
			selection:
				jet_protocol::SettingSelection::Key {
					key:
						jet_protocol::SettingKey::UtilityAccountBinding
						| jet_protocol::SettingKey::UtilityContentConsent
						| jet_protocol::SettingKey::UtilityAutodeleteCompilation
						| jet_protocol::SettingKey::UtilityGitText,
				},
			..
		} => Some(MinorRequirement {
			minor: jet_protocol::UTILITY_MINOR,
			feature: "Utility policy",
		}),
		QueryRequest::Settings {
			selection:
				jet_protocol::SettingSelection::Key {
					key:
						jet_protocol::SettingKey::ArtifactMaxMiB
						| jet_protocol::SettingKey::ArtifactRunMiB,
				},
			..
		} => Some(MinorRequirement {
			minor: jet_protocol::ARTIFACTS_MINOR,
			feature: "Artifact policy",
		}),
		QueryRequest::Settings {
			selection:
				jet_protocol::SettingSelection::Key {
					key:
						jet_protocol::SettingKey::EnergyConcurrency
						| jet_protocol::SettingKey::EnergyLowPowerConcurrency
						| jet_protocol::SettingKey::EnergyConstrained
						| jet_protocol::SettingKey::EnergyForegroundOverride,
				},
			..
		} => Some(MinorRequirement {
			minor: jet_protocol::ENERGY_MINOR,
			feature: "Energy policy",
		}),
		QueryRequest::Settings { .. } => Some(MinorRequirement {
			minor: jet_protocol::SETTINGS_AND_CAPABILITIES_MINOR,
			feature: "Setting Queries",
		}),
		QueryRequest::Capabilities { .. } => Some(MinorRequirement {
			minor: jet_protocol::SETTINGS_AND_CAPABILITIES_MINOR,
			feature: "the Capability Query",
		}),
		QueryRequest::Usage { .. } => Some(MinorRequirement {
			minor: jet_protocol::USAGE_RECORDS_MINOR,
			feature: "the Usage Query",
		}),
		QueryRequest::AccountBindings { .. } => Some(MinorRequirement {
			minor: jet_protocol::ACCOUNT_BINDINGS_MINOR,
			feature: "the Account binding Query",
		}),
		QueryRequest::SecurityAudit { .. } => Some(MinorRequirement {
			minor: jet_protocol::SECURITY_AUDIT_MINOR,
			feature: "the Security audit Query",
		}),
		QueryRequest::Pairing => Some(MinorRequirement {
			minor: jet_protocol::PAIRING_MINOR,
			feature: "the Pairing Query",
		}),
		QueryRequest::Projects => Some(MinorRequirement {
			minor: jet_protocol::PROJECTS_MINOR,
			feature: "the Project Query",
		}),
		QueryRequest::PreviewProject { .. } => Some(MinorRequirement {
			minor: jet_protocol::PROJECTS_MINOR,
			feature: "the Project preview Query",
		}),
		QueryRequest::ProjectEntry { .. } => Some(MinorRequirement {
			minor: jet_protocol::PROJECTS_MINOR,
			feature: "the Project entry Query",
		}),
		QueryRequest::PreviewPromotion { .. } => Some(MinorRequirement {
			minor: jet_protocol::WORKSPACE_PROMOTION_MINOR,
			feature: "the Workspace promotion preview Query",
		}),
		QueryRequest::Search { .. } => Some(MinorRequirement {
			minor: jet_protocol::SEARCH_MINOR,
			feature: "the Search Query",
		}),
		QueryRequest::ExternalConversations => Some(MinorRequirement {
			minor: jet_protocol::IMPORTED_CONVERSATIONS_MINOR,
			feature: "the external Conversation Query",
		}),
		QueryRequest::Status
		| QueryRequest::Conversations
		| QueryRequest::Conversation { .. }
		| QueryRequest::Events { .. } => None,
	}
}

pub(super) fn command_minor(
	command: &CommandRequest,
) -> Option<MinorRequirement> {
	match command {
		CommandRequest::AuthorizeApprovalRetry { .. } => {
			Some(MinorRequirement {
				minor: jet_protocol::APPROVAL_RETRY_MINOR,
				feature: "exact-action approval retries",
			})
		}
		CommandRequest::ReviewRemoteTool { .. } => Some(MinorRequirement {
			minor: jet_protocol::NO_VISA_MINOR,
			feature: "No-Visa review",
		}),
		CommandRequest::SetSetting {
			key: jet_protocol::SettingKey::StorageDisposableMiB,
			..
		}
		| CommandRequest::ClearSetting {
			key: jet_protocol::SettingKey::StorageDisposableMiB,
			..
		} => Some(MinorRequirement {
			minor: jet_protocol::DISK_PRESSURE_MINOR,
			feature: "disposable storage policy",
		}),
		CommandRequest::SetSetting {
			key:
				jet_protocol::SettingKey::GitAutoBranch
				| jet_protocol::SettingKey::GitAutoPush
				| jet_protocol::SettingKey::GitAutoDraftPullRequest
				| jet_protocol::SettingKey::GitBranchPrefix,
			..
		}
		| CommandRequest::ClearSetting {
			key:
				jet_protocol::SettingKey::GitAutoBranch
				| jet_protocol::SettingKey::GitAutoPush
				| jet_protocol::SettingKey::GitAutoDraftPullRequest
				| jet_protocol::SettingKey::GitBranchPrefix,
			..
		} => Some(MinorRequirement {
			minor: jet_protocol::GIT_DELIVERY_MINOR,
			feature: "Git delivery policy",
		}),
		CommandRequest::AcknowledgeGitDelivery { .. }
		| CommandRequest::DeliverGit { .. } => Some(MinorRequirement {
			minor: jet_protocol::GIT_DELIVERY_MINOR,
			feature: "Git delivery",
		}),
		CommandRequest::RequestUtility { .. } => Some(MinorRequirement {
			minor: jet_protocol::UTILITY_MINOR,
			feature: "Utility work",
		}),
		CommandRequest::DisableCraft { .. } => Some(MinorRequirement {
			minor: jet_protocol::CRAFT_LIFECYCLE_MINOR,
			feature: "Craft lifecycle controls",
		}),
		CommandRequest::ChangeExtension { .. } => Some(MinorRequirement {
			minor: jet_protocol::EXTENSIONS_MINOR,
			feature: "Harness extensions",
		}),
		CommandRequest::InstallCraft { .. } => Some(MinorRequirement {
			minor: jet_protocol::CRAFT_INSTALLATION_MINOR,
			feature: "third-party Craft installation",
		}),
		CommandRequest::SetAutoContinue { .. } => Some(MinorRequirement {
			minor: jet_protocol::AUTO_CONTINUE_MINOR,
			feature: "Auto-continue",
		}),
		CommandRequest::CreateSchedule { .. }
		| CommandRequest::CancelSchedule { .. } => Some(MinorRequirement {
			minor: jet_protocol::SCHEDULES_MINOR,
			feature: "Scheduled tasks",
		}),
		CommandRequest::ApplyUserEdit { .. }
		| CommandRequest::SubmitReview { .. } => Some(MinorRequirement {
			minor: jet_protocol::USER_INPUT_MINOR,
			feature: "direct user input",
		}),
		CommandRequest::SetConversationName { .. }
		| CommandRequest::SetRunName { .. } => Some(MinorRequirement {
			minor: jet_protocol::NAMES_MINOR,
			feature: "Conversation and Run names",
		}),
		CommandRequest::OpenTerminal { .. }
		| CommandRequest::CloseTerminal { .. } => Some(MinorRequirement {
			minor: jet_protocol::WORKSPACE_TERMINALS_MINOR,
			feature: "Workspace terminals",
		}),
		CommandRequest::ResolveExecution { .. } => Some(MinorRequirement {
			minor: jet_protocol::EXECUTION_RECOVERY_MINOR,
			feature: "execution recovery",
		}),
		CommandRequest::SubmitTurn { .. }
		| CommandRequest::WithdrawTurn { .. } => Some(MinorRequirement {
			minor: jet_protocol::TURN_QUEUE_MINOR,
			feature: "Turn queue",
		}),
		CommandRequest::StartNoVisaRun(_) => Some(MinorRequirement {
			minor: jet_protocol::NO_VISA_MINOR,
			feature: "No-Visa Runs",
		}),
		CommandRequest::StartVisaRun(_) => Some(MinorRequirement {
			minor: jet_protocol::VISA_RUNS_MINOR,
			feature: "Visa Runs",
		}),
		CommandRequest::StartRun { .. } => Some(MinorRequirement {
			minor: jet_protocol::MANAGED_RUNS_MINOR,
			feature: "managed Runs",
		}),
		CommandRequest::InterruptTurn { .. }
		| CommandRequest::StopRun { .. } => Some(MinorRequirement {
			minor: jet_protocol::EXECUTION_CONTROL_MINOR,
			feature: "execution control",
		}),
		CommandRequest::SetSetting {
			key:
				jet_protocol::SettingKey::UtilityAccountBinding
				| jet_protocol::SettingKey::UtilityContentConsent
				| jet_protocol::SettingKey::UtilityAutodeleteCompilation
				| jet_protocol::SettingKey::UtilityGitText,
			..
		}
		| CommandRequest::ClearSetting {
			key:
				jet_protocol::SettingKey::UtilityAccountBinding
				| jet_protocol::SettingKey::UtilityContentConsent
				| jet_protocol::SettingKey::UtilityAutodeleteCompilation
				| jet_protocol::SettingKey::UtilityGitText,
			..
		} => Some(MinorRequirement {
			minor: jet_protocol::UTILITY_MINOR,
			feature: "Utility policy",
		}),
		CommandRequest::SetSetting {
			key:
				jet_protocol::SettingKey::ArtifactMaxMiB
				| jet_protocol::SettingKey::ArtifactRunMiB,
			..
		}
		| CommandRequest::ClearSetting {
			key:
				jet_protocol::SettingKey::ArtifactMaxMiB
				| jet_protocol::SettingKey::ArtifactRunMiB,
			..
		} => Some(MinorRequirement {
			minor: jet_protocol::ARTIFACTS_MINOR,
			feature: "Artifact policy",
		}),
		CommandRequest::SetSetting {
			key:
				jet_protocol::SettingKey::EnergyConcurrency
				| jet_protocol::SettingKey::EnergyLowPowerConcurrency
				| jet_protocol::SettingKey::EnergyConstrained
				| jet_protocol::SettingKey::EnergyForegroundOverride,
			..
		}
		| CommandRequest::ClearSetting {
			key:
				jet_protocol::SettingKey::EnergyConcurrency
				| jet_protocol::SettingKey::EnergyLowPowerConcurrency
				| jet_protocol::SettingKey::EnergyConstrained
				| jet_protocol::SettingKey::EnergyForegroundOverride,
			..
		} => Some(MinorRequirement {
			minor: jet_protocol::ENERGY_MINOR,
			feature: "Energy policy",
		}),
		CommandRequest::SetSetting { .. }
		| CommandRequest::ClearSetting { .. } => Some(MinorRequirement {
			minor: jet_protocol::SETTINGS_AND_CAPABILITIES_MINOR,
			feature: "Setting Commands",
		}),
		CommandRequest::BindAccount { .. }
		| CommandRequest::UnbindAccount { .. } => Some(MinorRequirement {
			minor: jet_protocol::ACCOUNT_BINDINGS_MINOR,
			feature: "Account binding Commands",
		}),
		CommandRequest::BeginAuditEpoch => Some(MinorRequirement {
			minor: jet_protocol::SECURITY_AUDIT_MINOR,
			feature: "beginning a Security audit epoch",
		}),
		CommandRequest::RestoreRecoverySnapshot { .. } => {
			Some(MinorRequirement {
				minor: jet_protocol::STORE_RECOVERY_MINOR,
				feature: "restoring a Recovery snapshot",
			})
		}
		CommandRequest::PurgeRecoverySnapshots => Some(MinorRequirement {
			minor: jet_protocol::DELETION_LEDGER_MINOR,
			feature: "purging Recovery snapshots",
		}),
		CommandRequest::SetPairingGate { .. }
		| CommandRequest::OpenPairing { .. }
		| CommandRequest::ClaimPairing { .. }
		| CommandRequest::ConfirmPairing { .. }
		| CommandRequest::CompletePairing { .. }
		| CommandRequest::SetPairedClientAccess { .. }
		| CommandRequest::RevokePairedClient { .. } => Some(MinorRequirement {
			minor: jet_protocol::PAIRING_MINOR,
			feature: "Pairing Commands",
		}),
		CommandRequest::RegisterProject { .. } => Some(MinorRequirement {
			minor: jet_protocol::PROJECTS_MINOR,
			feature: "Project registration",
		}),
		CommandRequest::PromoteWorkspace { .. } => Some(MinorRequirement {
			minor: jet_protocol::WORKSPACE_PROMOTION_MINOR,
			feature: "Workspace promotion",
		}),
		CommandRequest::ImportConversation { .. }
		| CommandRequest::ResumeImportedConversation { .. } => {
			Some(MinorRequirement {
				minor: jet_protocol::IMPORTED_CONVERSATIONS_MINOR,
				feature: "importing external Conversations",
			})
		}
		CommandRequest::HandoffConversation(_) => Some(MinorRequirement {
			minor: jet_protocol::HANDOFFS_MINOR,
			feature: "cross-Harness Handoff",
		}),
		CommandRequest::ForkConversation { .. } => Some(MinorRequirement {
			minor: jet_protocol::CONVERSATION_FORKS_MINOR,
			feature: "Conversation forks",
		}),
		CommandRequest::CreateConversation { working_tree, .. }
			if working_tree.is_seeded() =>
		{
			Some(MinorRequirement {
				minor: jet_protocol::SEEDED_WORKSPACES_MINOR,
				feature: "a Workspace seeded from the Local checkout",
			})
		}
		CommandRequest::CreateConversation {
			working_tree:
				WorkingTreeRequest::Workspace { .. }
				| WorkingTreeRequest::LocalCheckout { .. },
			..
		} => Some(MinorRequirement {
			minor: jet_protocol::WORKSPACES_MINOR,
			feature: "a Conversation with a working tree",
		}),
		CommandRequest::CreateConversation {
			working_tree: WorkingTreeRequest::NoProject,
			..
		}
		| CommandRequest::CreateRun { .. }
		| CommandRequest::TransitionRun { .. } => None,
	}
}

pub(super) fn unsupported_minor(
	id: RequestId,
	requirement: MinorRequirement,
) -> ServerMessage {
	let MinorRequirement { minor, feature } = requirement;
	ServerMessage::Error {
		id: Some(id),
		error: wire_error(
			ErrorCategory::Incompatible,
			"protocol.unsupported_minor",
			format!("{feature} needs protocol minor {minor}"),
		),
	}
}
