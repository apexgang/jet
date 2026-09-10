//! Workspace terminal authority and the narrow terminal helper host seam.
pub(crate) mod command;
pub(crate) mod effect;
pub(crate) mod orphan;

use crate::{Actor, Core, CoreError, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use uuid::Uuid;

/// Stable identity of one Workspace terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TerminalId(pub Uuid);

/// Observed terminal lifecycle. Reconnect never creates a shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
	/// Launch was admitted.
	Opening,
	/// The helper owns a live PTY.
	Open,
	/// Explicit close is pending.
	Closing,
	/// Execution ended, including after reboot.
	Closed,
	/// A safe connection could not be established.
	Unavailable,
}

impl TerminalState {
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Opening => "opening",
			Self::Open => "open",
			Self::Closing => "closing",
			Self::Closed => "closed",
			Self::Unavailable => "unavailable",
		}
	}
}

/// A terminal belongs to its Workspace, independently of Runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTerminal {
	/// Terminal identity.
	pub terminal_id: TerminalId,
	/// Owning Workspace.
	pub workspace_id: WorkspaceId,
	/// Current lifecycle.
	pub state: TerminalState,
}

/// Immutable server-owned launch identity, persisted before any process starts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalPlan {
	/// Private plan version.
	pub version: u32,
	/// Client that authorized the execution.
	pub client_id: crate::ClientId,
	/// Terminal identity.
	pub terminal_id: TerminalId,
	/// Workspace identity.
	pub workspace_id: WorkspaceId,
	/// Canonical Workspace root.
	pub root: PathBuf,
	/// Registered Project root.
	pub project_root: PathBuf,
	/// Boot that admitted this execution.
	pub boot: String,
	/// Initial height.
	pub rows: u16,
	/// Initial width.
	pub columns: u16,
	/// SHA-256 of the accepted helper.
	pub helper_digest: String,
}

/// Ephemeral terminal operations; input bytes are never persisted as Commands.
#[derive(Debug)]
pub enum TerminalOperation {
	/// Read at most the given byte credit.
	Read {
		/// Requested offset.
		after: u64,
		/// Byte budget.
		limit: u32,
	},
	/// Deliver raw bytes once. A lost response is never retried automatically.
	Input(Vec<u8>),
	/// Resize the live PTY.
	Resize {
		/// Height.
		rows: u16,
		/// Width.
		columns: u16,
	},
	/// Stop the terminal's process.
	Close,
}

/// A bounded replay range. `offset > after` names an explicit byte gap.
#[derive(Debug)]
pub struct TerminalOutput {
	/// First returned byte.
	pub offset: u64,
	/// End of produced output.
	pub produced: u64,
	/// Raw PTY bytes.
	pub bytes: Vec<u8>,
	/// PTY output ended.
	pub closed: bool,
}

/// Trusted host Adapter for instance-validated terminal-role helpers.
/// Implementations must never launch during reconnect or retry uncertain input.
pub trait TerminalHost: std::fmt::Debug + Send + Sync {
	/// Discover bounded terminal execution identities independently of SQLite.
	fn discover(
		&self,
		home: PathBuf,
	) -> crate::RunFuture<'_, Result<Vec<TerminalId>, CoreError>>;
	/// Inspect read-only identity; only proven execution/boot loss returns None.
	fn inspect(
		&self,
		home: PathBuf,
		id: TerminalId,
	) -> crate::RunFuture<'_, Result<Option<crate::ExecutionMetadata>, CoreError>>;
	/// Terminate only the freshly validated instance the user inspected.
	fn terminate(
		&self,
		home: PathBuf,
		request: crate::ExecutionResolution,
	) -> crate::RunFuture<'_, Result<(), CoreError>>;
	/// Start once under a durable create-new execution barrier.
	fn start(
		&self,
		home: PathBuf,
		plan: TerminalPlan,
	) -> crate::RunFuture<'_, Result<(), CoreError>>;
	/// Validate authority and process identity before touching a helper.
	fn operate(
		&self,
		home: PathBuf,
		plan: TerminalPlan,
		operation: TerminalOperation,
	) -> crate::RunFuture<'_, Result<TerminalOutput, CoreError>>;
}

impl Core {
	/// Installs the terminal process Adapter before serving Commands.
	pub fn with_terminal_host(mut self, host: Arc<dyn TerminalHost>) -> Self {
		self.terminal_host = Some(host);
		self
	}
	/// Performs an authenticated, bounded ephemeral terminal operation.
	/// Returns authorization, lifecycle, or transport errors; never retries input.
	pub async fn terminal_operation(
		&self,
		actor: &Actor,
		id: TerminalId,
		operation: TerminalOperation,
	) -> Result<TerminalOutput, CoreError> {
		let _access =
			self.remote_access.acquire().await.expect("authority gate");
		actor.authorize(&self.remote_sessions)?;
		match &operation {
			TerminalOperation::Read { limit, .. } if *limit > 65536 => {
				return Err(invalid());
			}
			TerminalOperation::Input(bytes)
				if bytes.is_empty() || bytes.len() > 65536 =>
			{
				return Err(invalid());
			}
			TerminalOperation::Resize { rows, columns } => {
				dimensions(*rows, *columns)?
			}
			TerminalOperation::Close => return Err(invalid()),
			TerminalOperation::Read { .. } | TerminalOperation::Input(_) => {}
		}
		let record = self
			.store
			.read(async |tx| tx.terminal(id.0).await)
			.await?
			.ok_or_else(missing)?;
		let plan = decode_plan(&record)?;
		if record.state == "closed"
			&& !matches!(operation, TerminalOperation::Read { .. })
		{
			return Err(closed());
		}
		if record.state != "open"
			&& !matches!(operation, TerminalOperation::Read { .. })
		{
			return Err(unavailable());
		}
		if self.is_terminal_orphan(crate::RunId(id.0)).await? {
			return Err(unavailable());
		}
		if record.state != "closed" {
			self.validate_terminal_workspace(&plan).await?;
		}
		// ADR-0038 / ASVS 16.2.1: attribute input intent without retaining its content.
		if matches!(operation, TerminalOperation::Input(_)) {
			self.store
				.write(async |tx| {
					crate::terminal::command::audit(
						tx,
						actor,
						id,
						crate::AuditDecision::TerminalInput,
						self.now_unix_ms(),
					)
					.await
				})
				.await?;
		}
		let output = self
			.terminal_host
			.as_ref()
			.ok_or_else(unavailable)?
			.operate(self.run_home(), plan, operation)
			.await?;
		if record.state == "closed" && !output.closed {
			return Err(unavailable());
		}
		Ok(output)
	}
	pub(crate) async fn validate_terminal_workspace(
		&self,
		plan: &TerminalPlan,
	) -> Result<(), CoreError> {
		let valid = self
			.store
			.read(async |tx| {
				let Some(workspace) = tx.workspace(plan.workspace_id.0).await?
				else {
					return Ok::<_, CoreError>(false);
				};
				let project = tx.project(workspace.project_id).await?;
				Ok(std::path::Path::new(&workspace.root) == plan.root
					&& project.is_some_and(|p| {
						std::path::Path::new(&p.root) == plan.project_root
					}))
			})
			.await?;
		if !valid
			|| tokio::fs::canonicalize(&plan.root).await.ok().as_ref()
				!= Some(&plan.root)
			|| tokio::fs::canonicalize(&plan.project_root)
				.await
				.ok()
				.as_ref() != Some(&plan.project_root)
		{
			return Err(unavailable());
		}
		Ok(())
	}
}
pub(crate) fn dimensions(rows: u16, columns: u16) -> Result<(), CoreError> {
	// ASVS 2.2.1: bound cell dimensions before native allocation/ioctl.
	if !(1..=1000).contains(&rows) || !(1..=1000).contains(&columns) {
		return Err(invalid());
	}
	Ok(())
}
pub(crate) fn snapshot(
	record: &jet_store::TerminalRecord,
) -> Result<WorkspaceTerminal, CoreError> {
	let state = match record.state.as_str() {
		"opening" => TerminalState::Opening,
		"open" => TerminalState::Open,
		"closing" => TerminalState::Closing,
		"closed" => TerminalState::Closed,
		"unavailable" => TerminalState::Unavailable,
		_ => return Err(unavailable()),
	};
	Ok(WorkspaceTerminal {
		terminal_id: TerminalId(
			record.terminal_id.parse().map_err(|_| unavailable())?,
		),
		workspace_id: WorkspaceId(
			record.workspace_id.parse().map_err(|_| unavailable())?,
		),
		state,
	})
}
pub(crate) fn decode_plan(
	record: &jet_store::TerminalRecord,
) -> Result<TerminalPlan, CoreError> {
	let plan: TerminalPlan =
		serde_json::from_str(&record.plan).map_err(|_| unavailable())?;
	if plan.version != 1
		|| plan.terminal_id.0.to_string() != record.terminal_id
		|| plan.workspace_id.0.to_string() != record.workspace_id
	{
		return Err(unavailable());
	}
	Ok(plan)
}
pub(crate) fn invalid() -> CoreError {
	CoreError::invalid_input(
		"terminal.invalid_input",
		"terminal input, credit, or dimensions are outside their bounds",
	)
}
pub(crate) fn missing() -> CoreError {
	CoreError::not_found(
		"terminal.not_found",
		"the Workspace terminal does not exist",
	)
}
pub(crate) fn closed() -> CoreError {
	CoreError::conflict("terminal.closed", "the Workspace terminal is closed")
}
pub(crate) fn unavailable() -> CoreError {
	CoreError::conflict(
		"terminal.unavailable",
		"the Workspace terminal cannot be connected safely",
	)
}
