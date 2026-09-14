//! Autodelete rules: compiling, editing, approving, authorizing, and
//! deleting them, and reading them with their candidate matches
//! (ADR-0015). Everything here needs protocol minor 40.

use super::unexpected;
use crate::connection::{Client, ClientError};
use jet_protocol::{
	AutodeleteRule, AutodeleteRules, CommandRequest, CommandResponse,
	QueryRequest, QueryResponse,
};
use uuid::Uuid;

impl Client {
	/// Compiles `prompt` into the rule `rule_id`, or recompiles an
	/// existing rule's source, under the Command identity `command_id`.
	/// The rule comes back compiling; read it again for the draft.
	///
	/// # Errors
	///
	/// Returns [`ClientError::FeatureUnavailable`] when the negotiated
	/// minor predates Autodelete rules, [`ClientError::Remote`] when the
	/// daemon reports a stable error such as `utility.input_limit`, or
	/// the transport failure otherwise.
	pub async fn compile_autodelete_rule(
		&self,
		command_id: Uuid,
		rule_id: Uuid,
		prompt: String,
	) -> Result<AutodeleteRule, ClientError> {
		self.rule(
			command_id,
			CommandRequest::CompileAutodeleteRule { rule_id, prompt },
		)
		.await
	}

	/// Sets the rule's interpretation by hand, returning it to a draft.
	///
	/// # Errors
	///
	/// As [`Client::compile_autodelete_rule`]; a value outside 1 to 36500
	/// refuses with `autodelete.inactive_days_out_of_range`.
	pub async fn set_autodelete_rule_inactive_days(
		&self,
		command_id: Uuid,
		rule_id: Uuid,
		inactive_days: u32,
	) -> Result<AutodeleteRule, ClientError> {
		self.rule(
			command_id,
			CommandRequest::SetAutodeleteRuleInactiveDays {
				rule_id,
				inactive_days,
			},
		)
		.await
	}

	/// Approves `inactive_days`, the interpretation the draft showed.
	///
	/// # Errors
	///
	/// As [`Client::compile_autodelete_rule`]; a draft that reads
	/// differently by now answers `autodelete.interpretation_changed`, and
	/// a rule still compiling, refused, or already approved answers
	/// `autodelete.compiling`, `autodelete.refused`, or
	/// `autodelete.already_approved`.
	pub async fn approve_autodelete_rule(
		&self,
		command_id: Uuid,
		rule_id: Uuid,
		inactive_days: u32,
	) -> Result<AutodeleteRule, ClientError> {
		self.rule(
			command_id,
			CommandRequest::ApproveAutodeleteRule {
				rule_id,
				inactive_days,
			},
		)
		.await
	}

	/// Authorizes an approved rule to request native deletion of its
	/// matches too.
	///
	/// # Errors
	///
	/// As [`Client::compile_autodelete_rule`]; an unapproved rule answers
	/// `autodelete.not_approved`.
	pub async fn authorize_autodelete_everywhere(
		&self,
		command_id: Uuid,
		rule_id: Uuid,
	) -> Result<AutodeleteRule, ClientError> {
		self.rule(
			command_id,
			CommandRequest::AuthorizeAutodeleteEverywhere { rule_id },
		)
		.await
	}

	async fn rule(
		&self,
		command_id: Uuid,
		command: CommandRequest,
	) -> Result<AutodeleteRule, ClientError> {
		self.require_minor(jet_protocol::AUTODELETE_MINOR)?;
		match self.execute_command(command_id, command).await? {
			CommandResponse::AutodeleteRuleRecorded { rule } => Ok(rule),
			other @ (CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::ConversationTrashed { .. }
			| CommandResponse::AutodeleteRuleDeleted { .. }
			| CommandResponse::RecoverySnapshotRestored { .. }
			| CommandResponse::RecoverySnapshotsPurged { .. }
			| CommandResponse::ConversationRestored { .. }
			| CommandResponse::GitDeliveryAcknowledged { .. }
			| CommandResponse::GitDeliveryQueued { .. }
			| CommandResponse::ApprovalRetryAuthorized { .. }
			| CommandResponse::RemoteToolReviewed { .. }
			| CommandResponse::UtilityQueued { .. }
			| CommandResponse::ExtensionChangeQueued { .. }
			| CommandResponse::CraftDisabled { .. }
			| CommandResponse::CraftInstallationQueued(_)
			| CommandResponse::AutoContinueConfigured
			| CommandResponse::ScheduleCreated { .. }
			| CommandResponse::ScheduleCanceled { .. }
			| CommandResponse::UserEditApplied { .. }
			| CommandResponse::ConversationNamed(_)
			| CommandResponse::RunNamed(_)
			| CommandResponse::Terminal { .. }
			| CommandResponse::TurnAdmitted { .. }
			| CommandResponse::TurnWithdrawn { .. }
			| CommandResponse::RunControlAccepted { .. }
			| CommandResponse::ExecutionResolutionRecorded {
				..
			}
			| CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }
			| CommandResponse::PairedClientAccessSet { .. }
			| CommandResponse::PairedClientRevoked { .. }
			| CommandResponse::ProjectRegistered(_)
			| CommandResponse::ProjectRemoved(_)
			| CommandResponse::WorkspacePromotionRecorded(_)
			| CommandResponse::ConversationImported(_)) => Err(unexpected(&other)),
		}
	}

	/// Removes the rule. What it already staged stays in Jet Trash.
	///
	/// # Errors
	///
	/// As [`Client::compile_autodelete_rule`]; an unknown rule answers
	/// `autodelete.not_found`.
	pub async fn delete_autodelete_rule(
		&self,
		command_id: Uuid,
		rule_id: Uuid,
	) -> Result<(), ClientError> {
		self.require_minor(jet_protocol::AUTODELETE_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::DeleteAutodeleteRule { rule_id },
			)
			.await?
		{
			CommandResponse::AutodeleteRuleDeleted { .. } => Ok(()),
			other @ (CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::ConversationTrashed { .. }
			| CommandResponse::AutodeleteRuleRecorded { .. }
			| CommandResponse::RecoverySnapshotRestored { .. }
			| CommandResponse::RecoverySnapshotsPurged { .. }
			| CommandResponse::ConversationRestored { .. }
			| CommandResponse::GitDeliveryAcknowledged { .. }
			| CommandResponse::GitDeliveryQueued { .. }
			| CommandResponse::ApprovalRetryAuthorized { .. }
			| CommandResponse::RemoteToolReviewed { .. }
			| CommandResponse::UtilityQueued { .. }
			| CommandResponse::ExtensionChangeQueued { .. }
			| CommandResponse::CraftDisabled { .. }
			| CommandResponse::CraftInstallationQueued(_)
			| CommandResponse::AutoContinueConfigured
			| CommandResponse::ScheduleCreated { .. }
			| CommandResponse::ScheduleCanceled { .. }
			| CommandResponse::UserEditApplied { .. }
			| CommandResponse::ConversationNamed(_)
			| CommandResponse::RunNamed(_)
			| CommandResponse::Terminal { .. }
			| CommandResponse::TurnAdmitted { .. }
			| CommandResponse::TurnWithdrawn { .. }
			| CommandResponse::RunControlAccepted { .. }
			| CommandResponse::ExecutionResolutionRecorded {
				..
			}
			| CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }
			| CommandResponse::PairedClientAccessSet { .. }
			| CommandResponse::PairedClientRevoked { .. }
			| CommandResponse::ProjectRegistered(_)
			| CommandResponse::ProjectRemoved(_)
			| CommandResponse::WorkspacePromotionRecorded(_)
			| CommandResponse::ConversationImported(_)) => Err(unexpected(&other)),
		}
	}

	/// Reads every rule with the matches its interpretation selects today.
	///
	/// # Errors
	///
	/// As [`Client::compile_autodelete_rule`].
	pub async fn autodelete_rules(
		&self,
	) -> Result<AutodeleteRules, ClientError> {
		self.require_minor(jet_protocol::AUTODELETE_MINOR)?;
		match self.query(QueryRequest::AutodeleteRules).await? {
			QueryResponse::AutodeleteRules(rules) => Ok(rules),
			other @ (QueryResponse::Conversations(_)
			| QueryResponse::ConversationTrash(_)
			| QueryResponse::GitDeliveries { .. }
			| QueryResponse::ExtensionCatalog(_)
			| QueryResponse::ExtensionChange(_)
			| QueryResponse::RemoteToolReview(_)
			| QueryResponse::Utility(_)
			| QueryResponse::CraftInstallationPreview(_)
			| QueryResponse::AutoContinue(_)
			| QueryResponse::ScheduledTasks(_)
			| QueryResponse::RetentionPreview(_)
			| QueryResponse::EditableFile(_)
			| QueryResponse::WorkspaceTerminals { .. }
			| QueryResponse::TurnQueue(_)
			| QueryResponse::OrphanedExecutions(_)
			| QueryResponse::Status(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::Usage(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)
			| QueryResponse::Projects(_)
			| QueryResponse::ProjectPreview(_)
			| QueryResponse::ProjectRemovalPreview(_)
			| QueryResponse::ProjectEntry(_)
			| QueryResponse::PromotionPreview(_)
			| QueryResponse::ChangeArtifact(_)
			| QueryResponse::ChangeDiff(_)
			| QueryResponse::RunExecution(_)
			| QueryResponse::Search(_)
			| QueryResponse::ExternalConversations(_)) => Err(unexpected(&other)),
		}
	}
}
