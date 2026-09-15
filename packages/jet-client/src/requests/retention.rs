//! Forgetting, deleting everywhere, restoring, and reading Jet Trash
//! (ADR-0011, ADR-0015). Everything here needs protocol minor 39.

use super::unexpected;
use crate::connection::{Client, ClientError};
use jet_protocol::{
	CommandRequest, CommandResponse, ConversationTrash, QueryRequest,
	QueryResponse, RetentionPreview, TrashEntry,
};
use uuid::Uuid;

impl Client {
	/// Forgets the Conversation `conversation_id`: stages it in Jet Trash
	/// under the Command identity `command_id`, leaving the Harness's own
	/// history in place. Live work is refused with `retention.live_work`.
	///
	/// # Errors
	///
	/// Returns [`ClientError::FeatureUnavailable`] when the negotiated
	/// minor predates Jet Trash, [`ClientError::Remote`] when the daemon
	/// reports a stable error, or the transport failure otherwise.
	pub async fn forget_conversation(
		&self,
		command_id: Uuid,
		conversation_id: Uuid,
	) -> Result<TrashEntry, ClientError> {
		self.trash(
			command_id,
			CommandRequest::ForgetConversation { conversation_id },
		)
		.await
	}

	/// Deletes the Conversation everywhere: stops its active Run, cancels
	/// its queued turns, and stages it so its native history is requested
	/// from its Harness too when the grace period ends.
	///
	/// # Errors
	///
	/// As [`Client::forget_conversation`]; a Run still launching refuses
	/// with `retention.run_starting`.
	pub async fn delete_conversation_everywhere(
		&self,
		command_id: Uuid,
		conversation_id: Uuid,
	) -> Result<TrashEntry, ClientError> {
		self.trash(
			command_id,
			CommandRequest::DeleteConversationEverywhere { conversation_id },
		)
		.await
	}

	async fn trash(
		&self,
		command_id: Uuid,
		command: CommandRequest,
	) -> Result<TrashEntry, ClientError> {
		self.require_minor(jet_protocol::RETENTION_MINOR)?;
		match self.execute_command(command_id, command).await? {
			CommandResponse::ConversationTrashed { entry } => Ok(entry),
			other @ (CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::RecoverySnapshotRestored { .. }
			| CommandResponse::RecoverySnapshotsPurged { .. }
			| CommandResponse::ConversationRestored { .. }
			| CommandResponse::AutodeleteRuleRecorded { .. }
			| CommandResponse::AutodeleteRuleDeleted { .. }
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

	/// Takes the Conversation back out of Jet Trash.
	///
	/// # Errors
	///
	/// As [`Client::forget_conversation`]; a Conversation that is not
	/// staged refuses with `retention.not_trashed`.
	pub async fn restore_conversation(
		&self,
		command_id: Uuid,
		conversation_id: Uuid,
	) -> Result<(), ClientError> {
		self.require_minor(jet_protocol::RETENTION_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::RestoreConversation { conversation_id },
			)
			.await?
		{
			CommandResponse::ConversationRestored { .. } => Ok(()),
			other @ (CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::AutodeleteRuleRecorded { .. }
			| CommandResponse::AutodeleteRuleDeleted { .. }
			| CommandResponse::RecoverySnapshotRestored { .. }
			| CommandResponse::RecoverySnapshotsPurged { .. }
			| CommandResponse::ConversationTrashed { .. }
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

	/// Reads everything in Jet Trash, soonest expiry first.
	///
	/// # Errors
	///
	/// As [`Client::forget_conversation`].
	pub async fn conversation_trash(
		&self,
	) -> Result<ConversationTrash, ClientError> {
		self.require_minor(jet_protocol::RETENTION_MINOR)?;
		match self.query(QueryRequest::ConversationTrash).await? {
			QueryResponse::ConversationTrash(trash) => Ok(trash),
			other @ (QueryResponse::Conversations(_)
			| QueryResponse::GitDeliveries { .. }
			| QueryResponse::ExtensionCatalog(_)
			| QueryResponse::ExtensionChange(_)
			| QueryResponse::RemoteToolReview(_)
			| QueryResponse::Utility(_)
			| QueryResponse::CraftInstallationPreview(_)
			| QueryResponse::AutoContinue(_)
			| QueryResponse::ScheduledTasks(_)
			| QueryResponse::RetentionPreview(_)
			| QueryResponse::AutodeleteRules(_)
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
			| QueryResponse::UsageHistory(_)
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

	/// Reads what protects the Conversation from forgetting and what its
	/// deletion would leave behind.
	///
	/// # Errors
	///
	/// As [`Client::forget_conversation`].
	pub async fn retention_preview(
		&self,
		conversation_id: Uuid,
	) -> Result<RetentionPreview, ClientError> {
		self.require_minor(jet_protocol::RETENTION_MINOR)?;
		match self
			.query(QueryRequest::RetentionPreview { conversation_id })
			.await?
		{
			QueryResponse::RetentionPreview(preview) => Ok(preview),
			other @ (QueryResponse::Conversations(_)
			| QueryResponse::AutodeleteRules(_)
			| QueryResponse::GitDeliveries { .. }
			| QueryResponse::ExtensionCatalog(_)
			| QueryResponse::ExtensionChange(_)
			| QueryResponse::RemoteToolReview(_)
			| QueryResponse::Utility(_)
			| QueryResponse::CraftInstallationPreview(_)
			| QueryResponse::AutoContinue(_)
			| QueryResponse::ScheduledTasks(_)
			| QueryResponse::ConversationTrash(_)
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
			| QueryResponse::UsageHistory(_)
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
