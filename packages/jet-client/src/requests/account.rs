//! Account-binding queries and mutations.

use super::unexpected;
use crate::connection::{Client, ClientError};
use jet_protocol::{
	AccountBinding, AccountBindingList, CapabilityObservation, CommandRequest,
	CommandResponse, CredentialReference, CredentialSource, QueryRequest,
	QueryResponse,
};
use uuid::Uuid;

impl Client {
	/// Reads every Account binding on the Plane with the journal cursor the
	/// snapshot was read at. A binding carries non-secret metadata and the
	/// opaque reference its Credential resolves through, never the
	/// Credential itself (ADR-0016, ADR-0076).
	///
	/// Beside each binding is whether its Credential resolves right now,
	/// taken from the last observation of the Plane or from a new one, as
	/// `observation` chooses. A client that has just unlocked the credential
	/// store asks for a new one.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn account_bindings(
		&self,
		observation: CapabilityObservation,
	) -> Result<AccountBindingList, ClientError> {
		self.require_minor(jet_protocol::ACCOUNT_BINDINGS_MINOR)?;
		match self
			.query(QueryRequest::AccountBindings { observation })
			.await?
		{
			QueryResponse::AccountBindings(list) => Ok(list),
			other @ (QueryResponse::GitDeliveries { .. }
			| QueryResponse::ExtensionCatalog(_)
			| QueryResponse::ExtensionChange(_)
			| QueryResponse::RemoteToolReview(_)
			| QueryResponse::Usage(_)
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

	/// Binds a Provider account to the Plane under the Command identity
	/// `command_id`, which a retry must reuse (ADR-0093).
	///
	/// The request carries no secret: where the binding resolves through the
	/// platform credential store, the answer names the item the Plane will
	/// look in, and writing the secret there is the caller's to do
	/// (ADR-0076).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the metadata is not the metadata
	/// a binding carries or the Provider account is already bound, or the
	/// transport failure otherwise.
	pub async fn bind_account(
		&self,
		command_id: Uuid,
		provider: &str,
		label: &str,
		provider_account: Option<&str>,
		credential_source: CredentialSource,
	) -> Result<AccountBinding, ClientError> {
		self.require_minor(jet_protocol::ACCOUNT_BINDINGS_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::BindAccount {
					provider: provider.into(),
					label: label.into(),
					provider_account: provider_account.map(Into::into),
					credential_source,
				},
			)
			.await?
		{
			CommandResponse::AccountBound(binding) => Ok(binding),
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

	/// Removes an Account binding under the Command identity `command_id`,
	/// which a retry must reuse (ADR-0093). The answer returns the reference
	/// the Plane forgot, so the caller can remove the secret it owns from
	/// the backend that holds it.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Plane has no such binding,
	/// or the transport failure otherwise.
	pub async fn unbind_account(
		&self,
		command_id: Uuid,
		binding_id: Uuid,
	) -> Result<CredentialReference, ClientError> {
		self.require_minor(jet_protocol::ACCOUNT_BINDINGS_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::UnbindAccount { binding_id },
			)
			.await?
		{
			CommandResponse::AccountUnbound {
				credential_reference,
				..
			} => Ok(credential_reference),
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
