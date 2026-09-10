//! Reading the Usage records one Plane holds (ADR-0023).

use jet_protocol::{PlaneUsage, QueryRequest, QueryResponse, UsageSelection};

use crate::connection::{Client, ClientError};
use crate::requests::unexpected;

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
			other @ (QueryResponse::GitDeliveries { .. }
			| QueryResponse::ExtensionCatalog(_)
			| QueryResponse::ExtensionChange(_)
			| QueryResponse::RemoteToolReview(_)
			| QueryResponse::AccountBindings(_)
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
}
