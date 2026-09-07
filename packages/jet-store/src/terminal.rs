//! Durable Workspace terminal identities; raw PTY bytes never enter SQLite.
use crate::{ReadTransaction, StoreError, WriteTransaction};
use uuid::Uuid;

/// Private core plan and lifecycle for one terminal.
#[derive(Debug, Clone)]
pub struct TerminalRecord {
	/// Terminal identity.
	pub terminal_id: String,
	/// Registered Workspace identity.
	pub workspace_id: String,
	/// Validated lifecycle spelling.
	pub state: String,
	/// Versioned private core launch plan.
	pub plan: String,
}

impl ReadTransaction {
	/// Reads one terminal. Returns a store error if the query fails.
	pub async fn terminal(
		&mut self,
		id: Uuid,
	) -> Result<Option<TerminalRecord>, StoreError> {
		let id = id.to_string();
		Ok(sqlx::query_as!(TerminalRecord,
			"SELECT terminal_id, workspace_id, state, plan FROM workspace_terminals WHERE terminal_id = ?1", id)
			.fetch_optional(self.connection()).await?)
	}
	/// Reads terminals for a Workspace, with live terminals first and bounded recent history. Returns query errors.
	pub async fn workspace_terminals(
		&mut self,
		workspace: Uuid,
	) -> Result<Vec<TerminalRecord>, StoreError> {
		let workspace = workspace.to_string();
		Ok(sqlx::query_as!(TerminalRecord,
			"SELECT terminal_id, workspace_id, state, plan FROM workspace_terminals WHERE workspace_id = ?1 ORDER BY state = 'closed', terminal_id DESC LIMIT 64", workspace)
			.fetch_all(self.connection()).await?)
	}
	/// Pages live terminals for recovery. Returns query errors.
	pub async fn live_terminals(
		&mut self,
		after: &str,
	) -> Result<Vec<TerminalRecord>, StoreError> {
		Ok(sqlx::query_as!(TerminalRecord,
			"SELECT terminal_id, workspace_id, state, plan FROM workspace_terminals WHERE state <> 'closed' AND terminal_id > ?1 ORDER BY terminal_id LIMIT 64", after)
			.fetch_all(self.connection()).await?)
	}
}

impl WriteTransaction {
	/// Records admission alongside its Effect. Returns constraint or query errors.
	pub async fn insert_terminal(
		&mut self,
		terminal: &TerminalRecord,
	) -> Result<(), StoreError> {
		// ASVS 1.2.4: identifiers and plans are bound values, never SQL source.
		sqlx::query!("INSERT INTO workspace_terminals (terminal_id, workspace_id, state, plan) VALUES (?1, ?2, ?3, ?4)",
			terminal.terminal_id, terminal.workspace_id, terminal.state, terminal.plan)
			.execute(self.connection()).await?;
		Ok(())
	}
	/// Records observed lifecycle without resurrecting a closed terminal.
	/// Returns a store error if the update fails.
	pub async fn set_terminal_state(
		&mut self,
		id: Uuid,
		state: &str,
	) -> Result<(), StoreError> {
		let id = id.to_string();
		sqlx::query!("UPDATE workspace_terminals SET state = ?2 WHERE terminal_id = ?1 AND state <> 'closed'", id, state)
			.execute(self.connection()).await?;
		Ok(())
	}
}
