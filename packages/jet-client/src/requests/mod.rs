//! The Queries and Commands a client can issue once connected.

mod audit;

mod account;

mod setting;

mod run;

mod conversation;

pub(crate) mod auto_continue;
pub(crate) mod checkpoint;
pub(crate) mod craft_installation;
pub(crate) mod extension;
pub(crate) mod fork;
pub(crate) mod git_delivery;
pub(crate) mod handoff;
pub(crate) mod import;
pub(crate) mod name;
pub(crate) mod pairing;
pub(crate) mod project;
pub(crate) mod promotion;
pub(crate) mod review;
pub(crate) mod search;
pub(crate) mod turn;
pub(crate) mod usage;
pub(crate) mod user_input;
pub(crate) mod utility;
pub(crate) mod visa;

use crate::connection::{Client, ClientError};
use jet_protocol::{
	CapabilityObservation, CapabilitySnapshot, EventPage, PlaneStatus,
	QueryRequest, QueryResponse,
};

impl Client {
	/// Runs the status Query and returns the Plane status snapshot.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn status(&self) -> Result<PlaneStatus, ClientError> {
		match self.query(QueryRequest::Status).await? {
			QueryResponse::Status(status)
				if self.negotiated_minor()
					>= jet_protocol::FENCED_READS_MINOR
					&& status.cursor.is_none() =>
			{
				Err(ClientError::Unexpected(
					"jetd omitted the negotiated status fence".into(),
				))
			}
			QueryResponse::Status(status) => Ok(status),
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

	/// Reads one page of journal Events strictly after `sequence`; zero
	/// starts from the beginning of the journal. The page's cursor tells
	/// whether later pages exist (ADR-0092).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn events_after(
		&self,
		sequence: u64,
	) -> Result<EventPage, ClientError> {
		match self.query(QueryRequest::Events { after: sequence }).await? {
			QueryResponse::Events(page) => Ok(page),
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

	/// Reports what the Plane can do: its platform, the external tools it
	/// found, whether credentials resolve, its Crafts and Harnesses, and
	/// what leaves it degraded (ADR-0086).
	///
	/// `jetd` observes the Plane at startup and whenever `observation` asks
	/// for a new look; it never polls in between.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn capabilities(
		&self,
		observation: CapabilityObservation,
	) -> Result<CapabilitySnapshot, ClientError> {
		self.require_minor(jet_protocol::SETTINGS_AND_CAPABILITIES_MINOR)?;
		match self
			.query(QueryRequest::Capabilities { observation })
			.await?
		{
			QueryResponse::Capabilities(snapshot) => Ok(snapshot),
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
}

pub(crate) fn unexpected(reply: &impl std::fmt::Debug) -> ClientError {
	ClientError::Unexpected(format!("{reply:?}"))
}
