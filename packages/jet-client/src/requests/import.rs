//! External Conversations: what a Plane can see outside its management,
//! importing one, and continuing an import as a managed Conversation
//! (ADR-0010).

use crate::{
	connection::{Client, ClientError},
	requests::unexpected,
};
use jet_protocol::{
	CommandRequest, CommandResponse, Conversation, ExternalConversationList,
	ImportedConversation, QueryRequest, QueryResponse, RetentionPolicy,
	WorkingTreeRequest,
};
use uuid::Uuid;

impl Client {
	/// Lists the Harness-native Conversations the Plane can see outside
	/// its management, each placed against the Plane's Projects and marked
	/// with the live process holding it, and the imports the Plane holds.
	/// Live takeover is offered only where the process is reported as
	/// cooperating. Needs protocol minor 15.
	///
	/// # Errors
	///
	/// Returns [`ClientError::FeatureUnavailable`] when the negotiated
	/// minor predates imports, [`ClientError::Remote`] when the daemon
	/// reports a stable error, or the transport failure otherwise.
	pub async fn external_conversations(
		&self,
	) -> Result<ExternalConversationList, ClientError> {
		self.require_minor(jet_protocol::IMPORTED_CONVERSATIONS_MINOR)?;
		match self.query(QueryRequest::ExternalConversations).await? {
			QueryResponse::ExternalConversations(list) => Ok(list),
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
			| QueryResponse::Search(_)) => Err(unexpected(&other)),
		}
	}

	/// Registers the identity `native_conversation` of `harness`, as the
	/// Plane currently reports it, under the Command identity
	/// `command_id`, which a retry must reuse (ADR-0093). The import is
	/// metadata: it is not a Conversation and starts no Run. Needs
	/// protocol minor 15.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] with a stable `import.*` code such
	/// as `import.not_discovered` or `import.already_imported` when the
	/// Plane refuses, or the transport failure otherwise.
	pub async fn import_conversation(
		&self,
		command_id: Uuid,
		harness: &str,
		native_conversation: &str,
	) -> Result<ImportedConversation, ClientError> {
		self.require_minor(jet_protocol::IMPORTED_CONVERSATIONS_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::ImportConversation {
					harness: harness.into(),
					native_conversation: native_conversation.into(),
				},
			)
			.await?
		{
			CommandResponse::ConversationImported(imported) => Ok(imported),
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
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::RecoverySnapshotRestored { .. }
			| CommandResponse::RecoverySnapshotsPurged { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }
			| CommandResponse::PairedClientAccessSet { .. }
			| CommandResponse::PairedClientRevoked { .. }
			| CommandResponse::ProjectRegistered(_)
			| CommandResponse::WorkspacePromotionRecorded(_)) => Err(unexpected(&other)),
		}
	}

	/// Continues the import `import_id` as a new Conversation that works
	/// in `working_tree`: a Workspace or the Local checkout of a registered
	/// Project, which the user registers or maps first. A request with no
	/// Project is refused. Needs protocol minor 15.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] with a stable code such as
	/// `import.working_tree_required`, `import.not_found`,
	/// `import.already_resumed`, or `project.not_found` when the Plane
	/// refuses, or the transport failure otherwise.
	pub async fn resume_imported_conversation(
		&self,
		command_id: Uuid,
		import_id: Uuid,
		retention: RetentionPolicy,
		working_tree: WorkingTreeRequest,
	) -> Result<Conversation, ClientError> {
		self.require_minor(jet_protocol::IMPORTED_CONVERSATIONS_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::ResumeImportedConversation {
					import_id,
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
			| CommandResponse::RecoverySnapshotsPurged { .. }
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
