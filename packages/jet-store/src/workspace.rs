//! Managed Workspaces: the isolated Git worktrees Conversations work in
//! (ADR-0025).
//!
//! A row keeps what the Workspace was made from and where it is: the
//! Project, the base the user selected, the commit that selection resolved
//! to when the Workspace was created, and the Jet-owned root. The worktree
//! itself is filesystem state the core creates beside the row.

use uuid::Uuid;

use crate::StoreError;
use crate::records::parse_uuid;
use crate::transaction::{ReadTransaction, WriteTransaction};

/// A Workspace to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorkspace {
	/// Globally unique identity chosen by the caller.
	pub workspace_id: Uuid,
	/// The one Conversation that owns the Workspace.
	pub conversation_id: Uuid,
	/// The Project the Workspace was created from.
	pub project_id: Uuid,
	/// The canonical absolute root of the worktree.
	pub root: String,
	/// The base as the user selected it, such as a branch name.
	pub base_selection: String,
	/// The commit the selection resolved to, which never changes.
	pub base_commit: String,
	/// When the caller recorded the Workspace.
	pub created_at_unix_ms: i64,
}

/// One recorded Workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRecord {
	/// Globally unique identity.
	pub workspace_id: Uuid,
	/// The one Conversation that owns the Workspace.
	pub conversation_id: Uuid,
	/// The Project the Workspace was created from.
	pub project_id: Uuid,
	/// The canonical absolute root of the worktree.
	pub root: String,
	/// The base as the user selected it.
	pub base_selection: String,
	/// The commit the selection resolved to when the Workspace was made.
	pub base_commit: String,
	/// When the Workspace was recorded.
	pub created_at_unix_ms: i64,
}

/// One `workspaces` row as SQLite stores it.
struct Row {
	workspace_id: String,
	conversation_id: String,
	project_id: String,
	root: String,
	base_selection: String,
	base_commit: String,
	created_at_unix_ms: i64,
}

impl ReadTransaction {
	/// The Workspace owned by `conversation_id`, if it has one.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn workspace_of(
		&mut self,
		conversation_id: Uuid,
	) -> Result<Option<WorkspaceRecord>, StoreError> {
		let conversation_id = conversation_id.to_string();
		// ASVS 1.2.4: SQL structure is static; every dynamic value in this
		// module is passed through SQLite parameters.
		let row = sqlx::query_as!(
			Row,
			r#"SELECT workspace_id AS "workspace_id!", conversation_id,
				project_id, root, base_selection, base_commit,
				created_at_unix_ms
			 FROM workspaces
			 WHERE conversation_id = ?1"#,
			conversation_id
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(read_row).transpose()
	}
}

impl WriteTransaction {
	/// Records a new Workspace and returns it as stored.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written, including
	/// when the Conversation already owns a Workspace or another Workspace
	/// holds the same root.
	pub async fn insert_workspace(
		&mut self,
		workspace: NewWorkspace,
	) -> Result<WorkspaceRecord, StoreError> {
		let NewWorkspace {
			workspace_id,
			conversation_id,
			project_id,
			root,
			base_selection,
			base_commit,
			created_at_unix_ms,
		} = workspace;
		let id = workspace_id.to_string();
		let conversation = conversation_id.to_string();
		let project = project_id.to_string();
		sqlx::query!(
			"INSERT INTO workspaces
				(workspace_id, conversation_id, project_id, root,
				base_selection, base_commit, created_at_unix_ms)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
			id,
			conversation,
			project,
			root,
			base_selection,
			base_commit,
			created_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(WorkspaceRecord {
			workspace_id,
			conversation_id,
			project_id,
			root,
			base_selection,
			base_commit,
			created_at_unix_ms,
		})
	}
}

fn read_row(row: Row) -> Result<WorkspaceRecord, StoreError> {
	Ok(WorkspaceRecord {
		workspace_id: parse_uuid("workspace_id", &row.workspace_id)?,
		conversation_id: parse_uuid("conversation_id", &row.conversation_id)?,
		project_id: parse_uuid("project_id", &row.project_id)?,
		root: row.root,
		base_selection: row.base_selection,
		base_commit: row.base_commit,
		created_at_unix_ms: row.created_at_unix_ms,
	})
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
