//! Changes access for already paired clients.

use crate::{
	connection::{Client, ClientError},
	requests::unexpected,
};
use jet_protocol::{
	CommandRequest, CommandResponse, PairedClient, PairedClientAccess,
};
use uuid::Uuid;

impl Client {
	/// Stops a Paired client controlling the Plane, or lets it control the
	/// Plane again, under the Command identity `command_id`, which a retry
	/// must reuse (ADR-0093).
	///
	/// The Plane keeps the client's key either way, so a disabled client is
	/// enabled again without anybody pairing anything (ADR-0017).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Plane is not Paired with
	/// that client, or the transport failure otherwise.
	pub async fn set_paired_client_access(
		&self,
		command_id: Uuid,
		client_id: Uuid,
		access: PairedClientAccess,
	) -> Result<PairedClient, ClientError> {
		self.require_minor(jet_protocol::PAIRING_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::SetPairedClientAccess { client_id, access },
			)
			.await?
		{
			CommandResponse::PairedClientAccessSet { client } => Ok(client),
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
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }
			| CommandResponse::PairedClientRevoked { .. }
			| CommandResponse::ProjectRegistered(_)
			| CommandResponse::WorkspacePromotionRecorded(_)
			| CommandResponse::ConversationImported(_)) => Err(unexpected(&other)),
		}
	}

	/// Forgets a Paired client and the key it was Paired with, under the
	/// Command identity `command_id`, which a retry must reuse (ADR-0093).
	///
	/// Nothing in Jet brings either back: the installation is Paired again
	/// or it does not control the Plane (ADR-0017).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Plane is not Paired with
	/// that client, or the transport failure otherwise.
	pub async fn revoke_paired_client(
		&self,
		command_id: Uuid,
		client_id: Uuid,
	) -> Result<Uuid, ClientError> {
		self.require_minor(jet_protocol::PAIRING_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::RevokePairedClient { client_id },
			)
			.await?
		{
			CommandResponse::PairedClientRevoked { client_id } => Ok(client_id),
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
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }
			| CommandResponse::PairedClientAccessSet { .. }
			| CommandResponse::ProjectRegistered(_)
			| CommandResponse::WorkspacePromotionRecorded(_)
			| CommandResponse::ConversationImported(_)) => Err(unexpected(&other)),
		}
	}
}
