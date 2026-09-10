//! Interactive exact-action Automatic-review retries (ADR-0012).
use jet_protocol::{CommandRequest, CommandResponse};
use uuid::Uuid;

use crate::connection::{Client, ClientError};
use crate::requests::unexpected;

impl Client {
	/// Authorizes one further review of the stored denied action.
	///
	/// # Errors
	/// Returns a stable error for an old protocol, stale denial, consumed
	/// retry, unavailable Run, or transport failure. This never executes work.
	pub async fn authorize_approval_retry(
		&self,
		command_id: Uuid,
		run_id: Uuid,
		review_id: Uuid,
	) -> Result<(), ClientError> {
		self.require_minor(jet_protocol::APPROVAL_RETRY_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::AuthorizeApprovalRetry { run_id, review_id },
			)
			.await?
		{
			CommandResponse::ApprovalRetryAuthorized { review_id: granted }
				if granted == review_id =>
			{
				Ok(())
			}
			other @ (CommandResponse::GitDeliveryAcknowledged { .. }
			| CommandResponse::GitDeliveryQueued { .. }
			| CommandResponse::CraftInstallationQueued { .. }
			| CommandResponse::ApprovalRetryAuthorized { .. }
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
