//! Reading the Usage records one Plane holds (ADR-0023).

use crate::{
	connection::{Client, ClientError},
	requests::unexpected,
};
use jet_protocol::{
	PlaneUsage, QueryRequest, QueryResponse, UsageHistory, UsageHistoryRange,
	UsageHistorySelection, UsageResolution, UsageSelection,
};

impl Client {
	/// The Usage records this Plane holds for `selection`, each with the
	/// freshness and estimation it carries.
	///
	/// The answer covers this Plane alone. A client holding several Planes
	/// groups their bindings into one Provider account itself, and only over
	/// the Planes it is connected to (ADR-0016).
	///
	/// # Errors
	///
	/// Returns [`ClientError`] when the negotiated minor does not name the
	/// Usage Query, when the Plane refuses it, or when the transport fails.
	pub async fn usage(
		&self,
		selection: UsageSelection,
	) -> Result<Box<PlaneUsage>, ClientError> {
		self.require_minor(jet_protocol::USAGE_RECORDS_MINOR)?;
		match self.query(QueryRequest::Usage { selection }).await? {
			QueryResponse::Usage(usage) => Ok(usage),
			other @ (QueryResponse::UsageHistory(_)
			| QueryResponse::GitDeliveries { .. }
			| QueryResponse::ExtensionCatalog(_)
			| QueryResponse::ExtensionChange(_)
			| QueryResponse::RemoteToolReview(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::CredentialStoreVerification { .. }
			| QueryResponse::Utility(_)
			| QueryResponse::CraftInstallationPreview(_)
			| QueryResponse::AutoContinue(_)
			| QueryResponse::ScheduledTasks(_)
			| QueryResponse::ConversationTrash(_)
			| QueryResponse::RetentionPreview(_)
			| QueryResponse::AutodeleteRules(_)
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

	/// The Jet-observed consumption this Plane holds as a time series for
	/// `selection` over `range`, in buckets of `resolution` where the Plane
	/// still holds them and in days otherwise; the answer says which
	/// (ADR-0045).
	///
	/// # Errors
	///
	/// Returns [`ClientError`] when the negotiated minor does not name the
	/// Usage history Query, when the Plane refuses it, or when the
	/// transport fails.
	pub async fn usage_history(
		&self,
		selection: UsageHistorySelection,
		range: UsageHistoryRange,
		resolution: UsageResolution,
	) -> Result<Box<UsageHistory>, ClientError> {
		self.require_minor(jet_protocol::USAGE_HISTORY_MINOR)?;
		match self
			.query(QueryRequest::UsageHistory {
				selection,
				range,
				resolution,
			})
			.await?
		{
			QueryResponse::UsageHistory(history) => Ok(history),
			other @ (QueryResponse::Usage(_)
			| QueryResponse::GitDeliveries { .. }
			| QueryResponse::ExtensionCatalog(_)
			| QueryResponse::ExtensionChange(_)
			| QueryResponse::RemoteToolReview(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::CredentialStoreVerification { .. }
			| QueryResponse::Utility(_)
			| QueryResponse::CraftInstallationPreview(_)
			| QueryResponse::AutoContinue(_)
			| QueryResponse::ScheduledTasks(_)
			| QueryResponse::ConversationTrash(_)
			| QueryResponse::RetentionPreview(_)
			| QueryResponse::AutodeleteRules(_)
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
