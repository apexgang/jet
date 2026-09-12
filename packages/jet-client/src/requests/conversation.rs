//! Conversation queries and creation.

use super::unexpected;
use crate::connection::{Client, ClientError};
use jet_protocol::{
	CommandRequest, CommandResponse, Conversation, ConversationList,
	ConversationSnapshot, PageCursor, QueryRequest, QueryResponse,
	RetentionPolicy, WorkingTreeRequest,
};
use uuid::Uuid;

impl Client {
	/// Reads the first bounded page of Conversations with its journal fence.
	/// A minor-zero daemon instead returns its legacy complete list.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn conversations(&self) -> Result<ConversationList, ClientError> {
		match self.query(QueryRequest::Conversations).await? {
			QueryResponse::Conversations(list) => Ok(list),
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
			| QueryResponse::ProjectEntry(_)
			| QueryResponse::PromotionPreview(_)
			| QueryResponse::ChangeArtifact(_)
			| QueryResponse::ChangeDiff(_)
			| QueryResponse::RunExecution(_)
			| QueryResponse::Search(_)
			| QueryResponse::ExternalConversations(_)) => Err(unexpected(&other)),
		}
	}

	/// Continues a Conversation keyset snapshot from an opaque cursor
	/// returned by [`Client::conversations`] or this method.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] with restart metadata when the cursor
	/// expired or the snapshot changed, or the transport failure otherwise.
	pub async fn next_conversations(
		&self,
		cursor: PageCursor,
	) -> Result<ConversationList, ClientError> {
		self.require_minor(jet_protocol::FENCED_READS_MINOR)?;
		match self
			.query(QueryRequest::NextConversations { cursor })
			.await?
		{
			QueryResponse::Conversations(list) => Ok(list),
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
			| QueryResponse::ProjectEntry(_)
			| QueryResponse::PromotionPreview(_)
			| QueryResponse::ChangeArtifact(_)
			| QueryResponse::ChangeDiff(_)
			| QueryResponse::RunExecution(_)
			| QueryResponse::Search(_)
			| QueryResponse::ExternalConversations(_)) => Err(unexpected(&other)),
		}
	}

	/// Reads one Conversation with all of its Runs and the journal cursor
	/// the snapshot was read at.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Conversation does not exist
	/// or the daemon reports another stable error, or the transport failure
	/// otherwise.
	pub async fn conversation(
		&self,
		conversation_id: Uuid,
	) -> Result<ConversationSnapshot, ClientError> {
		match self
			.query(QueryRequest::Conversation { conversation_id })
			.await?
		{
			QueryResponse::Conversation(snapshot) => Ok(*snapshot),
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
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
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

	/// Creates a Conversation with no Runs under the Command identity
	/// `command_id`, which a retry must reuse (ADR-0093).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn create_conversation(
		&self,
		command_id: Uuid,
		retention: RetentionPolicy,
	) -> Result<Conversation, ClientError> {
		self.create_conversation_in(
			command_id,
			retention,
			WorkingTreeRequest::NoProject,
		)
		.await
	}

	/// Creates a Conversation that works in `working_tree`: a managed
	/// Workspace of a Project, created with it and seeded with the
	/// Local-checkout changes it selects, or the Project's own Local
	/// checkout (ADR-0025). Asking for either needs protocol minor 9, and
	/// seeding the Workspace needs minor 10.
	///
	/// # Errors
	///
	/// Returns [`ClientError::FeatureUnavailable`] when the negotiated
	/// minor predates working trees or seeds, [`ClientError::Remote`] when
	/// the daemon reports a stable error such as `project.not_found`,
	/// `workspace.base_not_found`, or `workspace.seed_base_mismatch`, or
	/// the transport failure otherwise.
	pub async fn create_conversation_in(
		&self,
		command_id: Uuid,
		retention: RetentionPolicy,
		working_tree: WorkingTreeRequest,
	) -> Result<Conversation, ClientError> {
		if working_tree.is_seeded() {
			self.require_minor(jet_protocol::SEEDED_WORKSPACES_MINOR)?;
		} else if !working_tree.is_no_project() {
			self.require_minor(jet_protocol::WORKSPACES_MINOR)?;
		}
		match self
			.execute_command(
				command_id,
				CommandRequest::CreateConversation {
					retention,
					working_tree,
				},
			)
			.await?
		{
			CommandResponse::ConversationCreated(conversation) => {
				Ok(conversation)
			}
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
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::RecoverySnapshotRestored { .. }
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
