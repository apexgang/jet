//! Terminal instances use the existing interactive Orphaned-execution decisions.
use crate::{
	Core, CoreError, ExecutionAction, ExecutionResolution, ExecutionRole,
	OrphanedExecution, RunId, TerminalId, TerminalOperation,
	effect::EffectResult, terminal,
};

impl Core {
	pub(crate) async fn is_terminal_orphan(
		&self,
		id: RunId,
	) -> Result<bool, CoreError> {
		let json = self
			.store
			.read(async |tx| tx.orphaned_execution(id.0).await)
			.await?;
		Ok(json
			.map(|json| crate::run::state::decode::<OrphanedExecution>(&json))
			.transpose()?
			.is_some_and(|orphan| orphan.role == ExecutionRole::Terminal))
	}
	pub(crate) async fn mark_terminal_orphan(
		&self,
		id: TerminalId,
	) -> Result<(), CoreError> {
		let metadata = match &self.terminal_host {
			Some(host) => match host.inspect(self.run_home(), id).await {
				Ok(None) => {
					self.store
						.write(async |tx| {
							tx.remove_orphaned_execution(id.0).await
						})
						.await?;
					return Ok(());
				}
				Ok(metadata) => metadata,
				Err(_) => None,
			},
			None => None,
		};
		let json = serde_json::to_string(&OrphanedExecution {
			role: ExecutionRole::Terminal,
			execution_id: RunId(id.0),
			metadata,
		})
		.map_err(|_| terminal::unavailable())?;
		self.store
			.write(async |tx| tx.insert_orphaned_execution(id.0, &json).await)
			.await?;
		Ok(())
	}
}
pub(crate) async fn prepare(
	core: &Core,
	request: &ExecutionResolution,
) -> Result<(), CoreError> {
	let host = core
		.terminal_host
		.as_ref()
		.ok_or_else(terminal::unavailable)?;
	let id = TerminalId(request.execution_id.0);
	let metadata = host
		.inspect(core.run_home(), id)
		.await?
		.ok_or_else(terminal::unavailable)?;
	if Some(metadata.instance) != request.instance {
		return Err(terminal::unavailable());
	}
	if request.action == ExecutionAction::Adopt {
		let record = core
			.store
			.read(async |tx| tx.terminal(id.0).await)
			.await?
			.ok_or_else(terminal::missing)?;
		if record.state == "closed" || record.state == "closing" {
			return Err(terminal::unavailable());
		}
		let plan = terminal::decode_plan(&record)?;
		core.validate_terminal_workspace(&plan).await?;
		if host
			.operate(
				core.run_home(),
				plan,
				TerminalOperation::Read { after: 0, limit: 0 },
			)
			.await?
			.closed
		{
			return Err(terminal::closed());
		}
	}
	Ok(())
}
pub(crate) async fn execute(
	core: &Core,
	request: &ExecutionResolution,
) -> EffectResult {
	match request.action {
		ExecutionAction::Adopt | ExecutionAction::Leave => {
			EffectResult::Completed
		}
		ExecutionAction::Terminate => match &core.terminal_host {
			Some(host) => {
				match host.terminate(core.run_home(), request.clone()).await {
					Ok(()) => EffectResult::Completed,
					Err(_) => EffectResult::Unknown,
				}
			}
			None => EffectResult::Unknown,
		},
	}
}
pub(crate) async fn settle(
	tx: &mut jet_store::WriteTransaction,
	request: &ExecutionResolution,
	now: i64,
) -> Result<(), CoreError> {
	let Some(json) = tx.orphaned_execution(request.execution_id.0).await?
	else {
		return Ok(());
	};
	let orphan: OrphanedExecution = crate::run::state::decode(&json)?;
	if orphan.role != ExecutionRole::Terminal
		|| tx.terminal(request.execution_id.0).await?.is_none()
	{
		return Ok(());
	}
	let state = match request.action {
		ExecutionAction::Adopt => crate::TerminalState::Open,
		ExecutionAction::Terminate => crate::TerminalState::Closed,
		ExecutionAction::Leave => return Ok(()),
	};
	crate::terminal::command::set_state(
		tx,
		TerminalId(request.execution_id.0),
		state,
		now,
	)
	.await
}
