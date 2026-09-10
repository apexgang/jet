//! Durable terminal admission and close Effects.
use crate::{
	Actor, AuditDecision, CommandId, CommandOutcome, Core, CoreError,
	TerminalId, TerminalPlan, WorkspaceId, terminal,
};
use jet_store::{
	EffectKindRecord, EffectSafetyRecord, NewEffect, TerminalRecord,
	WriteTransaction,
};
use uuid::Uuid;

pub(crate) async fn prepare(
	core: &Core,
	actor: &Actor,
	workspace_id: WorkspaceId,
	rows: u16,
	columns: u16,
) -> Result<TerminalPlan, CoreError> {
	terminal::dimensions(rows, columns)?;
	if core.terminal_host.is_none() {
		return Err(terminal::unavailable());
	}
	let (root, project_root) = core
		.store
		.read(async |tx| {
			let workspace = tx
				.workspace(workspace_id.0)
				.await?
				.ok_or_else(terminal::missing)?;
			let project = tx
				.project(workspace.project_id)
				.await?
				.ok_or_else(terminal::unavailable)?;
			Ok::<_, CoreError>((workspace.root.into(), project.root.into()))
		})
		.await?;
	let (boot, helper_digest) = crate::filesystem::blocking(|| {
		let boot = jet_runtime::execution_boot_identity()
			.map_err(|_| terminal::unavailable())?;
		let executable = std::env::current_exe()
			.map_err(|_| terminal::unavailable())?
			.with_file_name("jetfueld");
		Ok::<_, CoreError>((
			boot,
			jet_runtime::execution_digest(&executable)
				.map_err(|_| terminal::unavailable())?,
		))
	})
	.await??;
	let plan = TerminalPlan {
		version: 1,
		client_id: actor.client_id(),
		terminal_id: TerminalId(Uuid::now_v7()),
		workspace_id,
		root,
		project_root,
		boot,
		rows,
		columns,
		helper_digest,
	};
	core.validate_terminal_workspace(&plan).await?;
	Ok(plan)
}

pub(crate) async fn open(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	plan: TerminalPlan,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	// ASVS 15.2.2: one Workspace runs at most 64 terminals; closed history never blocks admission.
	if tx
		.workspace_terminals(plan.workspace_id.0)
		.await?
		.iter()
		.filter(|r| r.state != "closed")
		.count()
		>= 64
	{
		return Err(terminal::unavailable());
	}
	let record = TerminalRecord {
		terminal_id: plan.terminal_id.0.to_string(),
		workspace_id: plan.workspace_id.0.to_string(),
		state: "opening".into(),
		plan: serde_json::to_string(&plan)
			.map_err(|_| terminal::unavailable())?,
	};
	tx.insert_terminal(&record).await?;
	state_event(tx, actor, &record, now).await?;
	effect(
		tx,
		command_id,
		plan.terminal_id,
		EffectKindRecord::StartTerminal,
	)
	.await?;
	audit(
		tx,
		actor,
		plan.terminal_id,
		AuditDecision::TerminalOpened,
		now,
	)
	.await?;
	Ok(CommandOutcome::Terminal(terminal::snapshot(&record)?))
}
pub(crate) async fn close(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	id: TerminalId,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let mut record = tx.terminal(id.0).await?.ok_or_else(terminal::missing)?;
	if record.state != "closed" && record.state != "closing" {
		tx.set_terminal_state(id.0, "closing").await?;
		effect(tx, command_id, id, EffectKindRecord::CloseTerminal).await?;
		audit(tx, actor, id, AuditDecision::TerminalClosed, now).await?;
		record.state = "closing".into();
		state_event(tx, actor, &record, now).await?;
	}
	Ok(CommandOutcome::Terminal(terminal::snapshot(&record)?))
}
async fn effect(
	tx: &mut WriteTransaction,
	command_id: CommandId,
	id: TerminalId,
	kind: EffectKindRecord,
) -> Result<(), CoreError> {
	tx.insert_effect(&NewEffect {
		effect_id: Uuid::now_v7(),
		command_id: command_id.0,
		run_id: None,
		promotion_id: None,
		terminal_id: Some(id.0),
		kind,
		safety: EffectSafetyRecord::Ambiguous,
	})
	.await?;
	Ok(())
}
pub(crate) async fn audit(
	tx: &mut WriteTransaction,
	actor: &Actor,
	id: TerminalId,
	decision: AuditDecision,
	now: i64,
) -> Result<(), CoreError> {
	crate::audit::record(
		tx,
		actor,
		crate::audit::Decision::succeeded(
			decision,
			crate::audit::AuditSubject::Terminal(id),
		),
		now,
	)
	.await
}

pub(crate) async fn set_state(
	tx: &mut WriteTransaction,
	id: TerminalId,
	state: crate::TerminalState,
	now: i64,
) -> Result<(), CoreError> {
	let mut record = tx.terminal(id.0).await?.ok_or_else(terminal::missing)?;
	if record.state == state.as_str() || record.state == "closed" {
		return Ok(());
	}
	let plan = terminal::decode_plan(&record)?;
	tx.set_terminal_state(id.0, state.as_str()).await?;
	if state == crate::TerminalState::Closed {
		tx.remove_orphaned_execution(id.0).await?;
	}
	record.state = state.as_str().into();
	state_event(
		tx,
		&Actor::InteractiveClient {
			client_id: plan.client_id,
		},
		&record,
		now,
	)
	.await
}
async fn state_event(
	tx: &mut WriteTransaction,
	actor: &Actor,
	record: &TerminalRecord,
	now: i64,
) -> Result<(), CoreError> {
	let terminal = terminal::snapshot(record)?;
	tx.append_event(
		crate::EventKind::TerminalStateChanged {
			terminal_id: terminal.terminal_id,
			workspace_id: terminal.workspace_id,
			state: terminal.state,
		}
		.to_record(actor, crate::event::EventSubject::Plane, now)?,
	)
	.await?;
	Ok(())
}
