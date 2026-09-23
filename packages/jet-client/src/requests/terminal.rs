//! Workspace terminal lifecycle requests for client protocol minor 18.

use crate::{Client, ClientError, requests::unexpected};
use jet_protocol::{
	CommandRequest, CommandResponse, QueryRequest, QueryResponse,
	WorkspaceTerminal,
};
use uuid::Uuid;

impl Client {
	/// Reads the retained terminal lifecycle states for one Workspace.
	///
	/// # Errors
	/// Returns a compatibility, authorization, or transport error.
	pub async fn workspace_terminals(
		&self,
		workspace_id: Uuid,
	) -> Result<(u64, Vec<WorkspaceTerminal>), ClientError> {
		self.require_minor(jet_protocol::WORKSPACE_TERMINALS_MINOR)?;
		match self
			.query(QueryRequest::WorkspaceTerminals { workspace_id })
			.await?
		{
			QueryResponse::WorkspaceTerminals { cursor, terminals } => {
				Ok((cursor, terminals))
			}
			other => Err(unexpected(&other)),
		}
	}

	/// Opens one terminal under a registered Workspace.
	///
	/// # Errors
	/// Returns a compatibility, validation, authorization, or transport error.
	pub async fn open_terminal(
		&self,
		command_id: Uuid,
		workspace_id: Uuid,
		rows: u16,
		columns: u16,
	) -> Result<WorkspaceTerminal, ClientError> {
		self.require_minor(jet_protocol::WORKSPACE_TERMINALS_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::OpenTerminal {
					workspace_id,
					rows,
					columns,
				},
			)
			.await?
		{
			CommandResponse::Terminal { terminal } => Ok(terminal),
			other => Err(unexpected(&other)),
		}
	}

	/// Explicitly closes one retained Workspace terminal.
	///
	/// # Errors
	/// Returns a compatibility, validation, authorization, or transport error.
	pub async fn close_terminal(
		&self,
		command_id: Uuid,
		terminal_id: Uuid,
	) -> Result<WorkspaceTerminal, ClientError> {
		self.require_minor(jet_protocol::WORKSPACE_TERMINALS_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::CloseTerminal { terminal_id },
			)
			.await?
		{
			CommandResponse::Terminal { terminal } => Ok(terminal),
			other => Err(unexpected(&other)),
		}
	}
}
