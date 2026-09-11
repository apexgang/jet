//! Run creation and lifecycle requests.

use super::unexpected;
use crate::connection::{Client, ClientError};
use jet_protocol::{CommandRequest, CommandResponse, Run, RunLifecycle};
use uuid::Uuid;

impl Client {
	/// Records a new Run of a Conversation that has no live Run under the
	/// Command identity `command_id`, which a retry must reuse (ADR-0093).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Conversation does not exist
	/// or already has a live Run, or the transport failure otherwise.
	pub async fn create_run(
		&self,
		command_id: Uuid,
		conversation_id: Uuid,
	) -> Result<Run, ClientError> {
		match self
			.execute_command(
				command_id,
				CommandRequest::CreateRun { conversation_id },
			)
			.await?
		{
			CommandResponse::RunCreated(run) => Ok(run),
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

	/// Moves a Run forward to `lifecycle` if its current Revision is
	/// `expected_revision`, under the Command identity `command_id`, which a
	/// retry must reuse (ADR-0093).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Run does not exist or the
	/// transition is not allowed, or the transport failure otherwise.
	pub async fn transition_run(
		&self,
		command_id: Uuid,
		run_id: Uuid,
		expected_revision: u64,
		lifecycle: RunLifecycle,
	) -> Result<Run, ClientError> {
		match self
			.execute_command(
				command_id,
				CommandRequest::TransitionRun {
					run_id,
					expected_revision,
					lifecycle,
				},
			)
			.await?
		{
			CommandResponse::RunTransitioned(run) => Ok(run),
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
