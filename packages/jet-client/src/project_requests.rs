//! The Project Query and Command (ADR-0025, ADR-0101).

use jet_protocol::{
	CommandRequest, CommandResponse, Project, ProjectList, QueryRequest,
	QueryResponse,
};
use uuid::Uuid;

use crate::connection::{Client, ClientError};
use crate::requests::unexpected;

impl Client {
	/// Registers the Git working tree at the absolute `path` as a Project
	/// under the Command identity `command_id`, which a retry must reuse
	/// (ADR-0093).
	///
	/// This is a Path grant: the one request that carries an absolute
	/// path, made only from an interactive surface after showing the user
	/// what will be registered. The Plane resolves the path to the
	/// directory it names and refuses a bare repository, a directory that
	/// is not a working tree, or one that is not the top of its working
	/// tree (ADR-0103).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] with a stable `path_grant.*` or
	/// `project.*` code when the Plane refuses the grant, or the transport
	/// failure otherwise.
	pub async fn register_project(
		&self,
		command_id: Uuid,
		path: &str,
	) -> Result<Project, ClientError> {
		self.require_minor(jet_protocol::PROJECTS_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::RegisterProject { path: path.into() },
			)
			.await?
		{
			CommandResponse::ProjectRegistered(project) => Ok(project),
			other @ (CommandResponse::ConversationCreated(_)
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
			| CommandResponse::PairedClientRevoked { .. }) => Err(unexpected(&other)),
		}
	}

	/// Reads every registered Project with the journal cursor the snapshot
	/// was read at.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn projects(&self) -> Result<ProjectList, ClientError> {
		self.require_minor(jet_protocol::PROJECTS_MINOR)?;
		match self.query(QueryRequest::Projects).await? {
			QueryResponse::Projects(list) => Ok(list),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)) => Err(unexpected(&other)),
		}
	}
}
