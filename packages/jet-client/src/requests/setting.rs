//! Setting queries and mutations.

use super::unexpected;
use crate::connection::{Client, ClientError};
use jet_protocol::{
	CommandRequest, CommandResponse, QueryRequest, QueryResponse, SettingKey,
	SettingScope, SettingSelection, SettingSnapshot, SettingValue,
};
use uuid::Uuid;

impl Client {
	/// Resolves Settings for `scope`, the scope's own values winning over
	/// the Plane's and built-in defaults beneath both (ADR-0085).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the scope may not store a named
	/// Setting or the daemon reports another stable error, or the transport
	/// failure otherwise.
	pub async fn settings(
		&self,
		scope: SettingScope,
		selection: SettingSelection,
	) -> Result<SettingSnapshot, ClientError> {
		self.require_minor(jet_protocol::SETTINGS_AND_CAPABILITIES_MINOR)?;
		match self
			.query(QueryRequest::Settings { scope, selection })
			.await?
		{
			QueryResponse::Settings(snapshot) => Ok(snapshot),
			other @ (QueryResponse::GitDeliveries { .. }
			| QueryResponse::ExtensionCatalog(_)
			| QueryResponse::ExtensionChange(_)
			| QueryResponse::RemoteToolReview(_)
			| QueryResponse::Utility(_)
			| QueryResponse::CraftInstallationPreview(_)
			| QueryResponse::AutoContinue(_)
			| QueryResponse::ScheduledTasks(_)
			| QueryResponse::EditableFile(_)
			| QueryResponse::WorkspaceTerminals { .. }
			| QueryResponse::TurnQueue(_)
			| QueryResponse::OrphanedExecutions(_)
			| QueryResponse::Status(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::Usage(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)
			| QueryResponse::Projects(_)
			| QueryResponse::ProjectPreview(_)
			| QueryResponse::ProjectEntry(_)
			| QueryResponse::PromotionPreview(_)
			| QueryResponse::ChangeArtifact(_)
			| QueryResponse::ChangeDiff(_)
			| QueryResponse::RunExecution(_)
			| QueryResponse::Search(_)
			| QueryResponse::ExternalConversations(_)) => Err(unexpected(&other)),
		}
	}

	/// Stores `value` for `key` at `scope` under the Command identity
	/// `command_id`, which a retry must reuse (ADR-0093).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the scope may not store the
	/// Setting or the value does not fit it, or the transport failure
	/// otherwise.
	pub async fn set_setting(
		&self,
		command_id: Uuid,
		key: SettingKey,
		scope: SettingScope,
		value: SettingValue,
	) -> Result<SettingValue, ClientError> {
		self.require_minor(jet_protocol::SETTINGS_AND_CAPABILITIES_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::SetSetting { key, scope, value },
			)
			.await?
		{
			CommandResponse::SettingSet { value, .. } => Ok(value),
			other @ (CommandResponse::GitDeliveryAcknowledged { .. }
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
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }
			| CommandResponse::PairedClientAccessSet { .. }
			| CommandResponse::PairedClientRevoked { .. }
			| CommandResponse::ProjectRegistered(_)
			| CommandResponse::WorkspacePromotionRecorded(_)
			| CommandResponse::ConversationImported(_)) => Err(unexpected(&other)),
		}
	}

	/// Removes whatever `scope` stores for `key`, leaving the scopes above
	/// it untouched, under the Command identity `command_id`.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the scope may not store the
	/// Setting, or the transport failure otherwise.
	pub async fn clear_setting(
		&self,
		command_id: Uuid,
		key: SettingKey,
		scope: SettingScope,
	) -> Result<(), ClientError> {
		self.require_minor(jet_protocol::SETTINGS_AND_CAPABILITIES_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::ClearSetting { key, scope },
			)
			.await?
		{
			CommandResponse::SettingCleared { .. } => Ok(()),
			other @ (CommandResponse::GitDeliveryAcknowledged { .. }
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
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }
			| CommandResponse::PairedClientAccessSet { .. }
			| CommandResponse::PairedClientRevoked { .. }
			| CommandResponse::ProjectRegistered(_)
			| CommandResponse::WorkspacePromotionRecorded(_)
			| CommandResponse::ConversationImported(_)) => Err(unexpected(&other)),
		}
	}
}
