//! Terminal launch/close outbox and evidence-based recovery.
use crate::effect::{Effect, EffectAdapter, EffectKind, EffectResult};
use crate::{
	Core, CoreError, TerminalId, TerminalOperation, TerminalState, terminal,
};
use jet_store::{EffectKindRecord, EffectStateRecord, WriteTransaction};

struct Terminals<'a>(&'a Core);
impl Core {
	/// Performs durable terminal Effects and refreshes live helper evidence.
	/// Returns store errors; uncertain transport never proves a terminal closed.
	pub async fn perform_terminals(&self) -> Result<(), CoreError> {
		self.reconcile_effects(
			&mut Terminals(self),
			EffectKindRecord::StartTerminal,
		)
		.await?;
		self.reconcile_effects(
			&mut Terminals(self),
			EffectKindRecord::CloseTerminal,
		)
		.await?;
		self.recover_terminals().await
	}
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "terminal recovery must serialize with launch and close Effects"
	)]
	async fn recover_terminals(&self) -> Result<(), CoreError> {
		let _gate = self.effect_reconciliation.lock().await;
		let Some(host) = &self.terminal_host else {
			return Ok(());
		};
		// SQLite may have been restored independently of live helpers.
		let mut discovered: std::collections::HashSet<_> =
			host.discover(self.run_home()).await?.into_iter().collect();
		let mut cursor = None;
		loop {
			let page = self.orphaned_executions(cursor).await?;
			discovered.extend(
				page.executions
					.into_iter()
					.filter(|o| o.role == crate::ExecutionRole::Terminal)
					.map(|o| TerminalId(o.execution_id.0)),
			);
			cursor = page.next;
			if cursor.is_none() {
				break;
			}
		}
		for id in discovered {
			if self
				.store
				.read(async |tx| tx.terminal(id.0).await)
				.await?
				.is_none_or(|record| record.state == "closed")
			{
				self.mark_terminal_orphan(id).await?;
			}
		}
		let mut after = String::new();
		loop {
			let records = self
				.store
				.read(async |tx| tx.live_terminals(&after).await)
				.await?;
			if records.is_empty() {
				return Ok(());
			}
			for record in records {
				after = record.terminal_id.clone();
				if record.state == "opening" || record.state == "closing" {
					continue;
				}
				let plan = terminal::decode_plan(&record)?;
				let id = plan.terminal_id;
				let unsafe_root =
					self.validate_terminal_workspace(&plan).await.is_err();
				let orphaned =
					self.is_terminal_orphan(crate::RunId(id.0)).await?;
				if unsafe_root || orphaned {
					self.mark_terminal_orphan(id).await?;
				}
				let result = host
					.operate(
						self.run_home(),
						plan,
						TerminalOperation::Read { after: 0, limit: 0 },
					)
					.await;
				if result.is_err() {
					self.mark_terminal_orphan(id).await?;
				}
				let state = match result {
					Ok(output) if output.closed => TerminalState::Closed,
					Ok(_) if unsafe_root || orphaned => {
						TerminalState::Unavailable
					}
					Ok(_) => TerminalState::Open,
					Err(_) => TerminalState::Unavailable,
				};
				if state.as_str() != record.state {
					self.store
						.write(async |tx| {
							crate::terminal_command::set_state(
								tx,
								id,
								state,
								self.now_unix_ms(),
							)
							.await
						})
						.await?;
				}
			}
		}
	}
}
impl EffectAdapter for Terminals<'_> {
	async fn execute(&mut self, effect: &Effect) -> EffectResult {
		let (id, close) = match effect.kind {
			EffectKind::StartTerminal { terminal_id } => (terminal_id, false),
			EffectKind::CloseTerminal { terminal_id } => (terminal_id, true),
			_ => return EffectResult::Unknown,
		};
		let Ok(Some(record)) =
			self.0.store.read(async |tx| tx.terminal(id.0).await).await
		else {
			return EffectResult::Unknown;
		};
		let Ok(plan) = terminal::decode_plan(&record) else {
			return EffectResult::Unknown;
		};
		let Some(host) = &self.0.terminal_host else {
			return EffectResult::Failed;
		};
		if close {
			return match host
				.operate(self.0.run_home(), plan, TerminalOperation::Close)
				.await
			{
				Ok(output) if output.closed => EffectResult::Completed,
				Ok(_) | Err(_) => EffectResult::Unknown,
			};
		}
		if record.state == "closing"
			|| self.0.validate_terminal_workspace(&plan).await.is_err()
		{
			return EffectResult::Failed;
		}
		match host.start(self.0.run_home(), plan).await {
			Ok(()) => EffectResult::Completed,
			Err(_) => EffectResult::Unknown,
		}
	}
	async fn reconcile(&mut self, effect: &Effect) -> EffectResult {
		let id = match effect.kind {
			EffectKind::StartTerminal { terminal_id }
			| EffectKind::CloseTerminal { terminal_id } => terminal_id,
			_ => return EffectResult::Unknown,
		};
		let Ok(Some(record)) =
			self.0.store.read(async |tx| tx.terminal(id.0).await).await
		else {
			return EffectResult::Unknown;
		};
		let Ok(plan) = terminal::decode_plan(&record) else {
			return EffectResult::Unknown;
		};
		let Some(host) = &self.0.terminal_host else {
			return EffectResult::Unknown;
		};
		if matches!(effect.kind, EffectKind::StartTerminal { .. })
			&& self.0.validate_terminal_workspace(&plan).await.is_err()
		{
			return EffectResult::Unknown;
		}
		match host
			.operate(
				self.0.run_home(),
				plan,
				TerminalOperation::Read { after: 0, limit: 0 },
			)
			.await
		{
			Ok(output) => match effect.kind {
				EffectKind::StartTerminal { .. } if output.closed => {
					EffectResult::Failed
				}
				EffectKind::StartTerminal { .. } => EffectResult::Completed,
				EffectKind::CloseTerminal { .. } if output.closed => {
					EffectResult::Completed
				}
				EffectKind::CloseTerminal { .. } => self.execute(effect).await,
				_ => EffectResult::Unknown,
			},
			Err(_) => EffectResult::Unknown,
		}
	}
}
pub(crate) async fn settle(
	tx: &mut WriteTransaction,
	id: TerminalId,
	kind: &EffectKind,
	state: EffectStateRecord,
	now: i64,
) -> Result<(), CoreError> {
	let record = tx.terminal(id.0).await?.ok_or_else(terminal::missing)?;
	// A close admitted during launch always wins over launch's completion.
	if matches!(kind, EffectKind::StartTerminal { .. })
		&& record.state == "closing"
	{
		return Ok(());
	}
	let lifecycle = match state {
		EffectStateRecord::Completed => match kind {
			EffectKind::StartTerminal { .. } => TerminalState::Open,
			_ => TerminalState::Closed,
		},
		EffectStateRecord::Failed => TerminalState::Closed,
		EffectStateRecord::OutcomeUnknown => TerminalState::Unavailable,
		EffectStateRecord::Pending | EffectStateRecord::InFlight => {
			return Ok(());
		}
	};
	crate::terminal_command::set_state(tx, id, lifecycle, now).await
}
