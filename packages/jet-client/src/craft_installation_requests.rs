//! Verified third-party Craft discovery and installation requests.

use jet_protocol::{
	CommandRequest, CommandResponse, CraftInstallationConfirmation,
	CraftInstallationPreview, CraftInstallationQueued, CraftSource,
	QueryRequest, QueryResponse,
};
use uuid::Uuid;

use crate::connection::{Client, ClientError};
use crate::requests::unexpected;

impl Client {
	/// Disables new Runs and optionally stops the Craft, preserving its helpers.
	/// # Errors
	/// Returns a compatibility, admission, or transport error.
	pub async fn disable_craft(
		&self,
		command_id: Uuid,
		craft_id: String,
		mode: jet_protocol::CraftDisableMode,
	) -> Result<(), ClientError> {
		self.require_minor(jet_protocol::CRAFT_LIFECYCLE_MINOR)?;
		let reply = self
			.execute_command(
				command_id,
				CommandRequest::DisableCraft { craft_id, mode },
			)
			.await?;
		if matches!(reply, CommandResponse::CraftDisabled { .. }) {
			Ok(())
		} else {
			Err(unexpected(&reply))
		}
	}

	/// Verifies `source` and returns the exact consent surface without
	/// changing Plane state.
	///
	/// # Errors
	/// Returns [`ClientError::FeatureUnavailable`] when the negotiated minor
	/// predates third-party installation, [`ClientError::Remote`] when the
	/// source is invalid, or the transport failure otherwise.
	pub async fn discover_craft(
		&self,
		source: CraftSource,
	) -> Result<CraftInstallationPreview, ClientError> {
		self.require_minor(jet_protocol::CRAFT_INSTALLATION_MINOR)?;
		match self.query(QueryRequest::DiscoverCraft { source }).await? {
			QueryResponse::CraftInstallationPreview(preview) => Ok(preview),
			other @ (QueryResponse::ExtensionCatalog(_)
			| QueryResponse::ExtensionChange(_)
			| QueryResponse::RemoteToolReview(_)
			| QueryResponse::Utility(_)
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
			| QueryResponse::Search(_)
			| QueryResponse::ExternalConversations(_)) => Err(unexpected(&other)),
		}
	}

	/// Accepts every value in a discovery preview under `command_id`. The
	/// daemon re-verifies the source and exact Artifact before committing.
	///
	/// # Errors
	/// Returns [`ClientError::FeatureUnavailable`] when the negotiated minor
	/// predates third-party installation, [`ClientError::Remote`] when the
	/// confirmation is stale or invalid, or the transport failure otherwise.
	pub async fn install_craft(
		&self,
		command_id: Uuid,
		confirmation: CraftInstallationConfirmation,
	) -> Result<CraftInstallationQueued, ClientError> {
		self.require_minor(jet_protocol::CRAFT_INSTALLATION_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::InstallCraft { confirmation },
			)
			.await?
		{
			CommandResponse::CraftInstallationQueued(queued) => Ok(queued),
			other @ (CommandResponse::ApprovalRetryAuthorized { .. }
			| CommandResponse::RemoteToolReviewed { .. }
			| CommandResponse::ExtensionChangeQueued { .. }
			| CommandResponse::CraftDisabled { .. }
			| CommandResponse::UtilityQueued { .. }
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
