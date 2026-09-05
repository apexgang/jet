//! Registered Projects: the working trees an interactive user granted Jet
//! access to (ADR-0025, ADR-0101).
//!
//! A row keeps the canonical absolute root the grant resolved to and the
//! Actor that made it. What the repository looked like at registration is
//! observed, not stored: it describes the working tree, which changes
//! without Jet.

use uuid::Uuid;

use crate::StoreError;
use crate::records::{ActorRecord, parse_uuid};
use crate::transaction::{ReadTransaction, WriteTransaction};

/// A Project to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProject {
	/// Globally unique identity chosen by the caller.
	pub project_id: Uuid,
	/// The canonical absolute root of the working tree.
	pub root: String,
	/// The authenticated Actor that granted the root.
	pub registered_by: ActorRecord,
	/// When the caller recorded the Project.
	pub registered_at_unix_ms: i64,
}

/// One registered Project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
	/// Globally unique identity.
	pub project_id: Uuid,
	/// The canonical absolute root of the working tree.
	pub root: String,
	/// The authenticated Actor that granted the root.
	pub registered_by: ActorRecord,
	/// When the Project was recorded.
	pub registered_at_unix_ms: i64,
}

/// One `projects` row as SQLite stores it, before its text columns are
/// parsed back into domain types.
struct Row {
	project_id: String,
	root: String,
	actor_kind: String,
	actor_id: String,
	registered_at_unix_ms: i64,
}

impl ReadTransaction {
	/// Every registered Project, in the order they were registered.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn projects(&mut self) -> Result<Vec<ProjectRecord>, StoreError> {
		// ASVS 1.2.4: SQL structure is static; every dynamic value in this
		// module is passed through SQLite parameters.
		let rows = sqlx::query_as!(
			Row,
			r#"SELECT project_id AS "project_id!", root, actor_kind, actor_id,
				registered_at_unix_ms
			 FROM projects
			 ORDER BY rowid"#
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter().map(read_row).collect()
	}

	/// The Project identified by `project_id`, if registered.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn project(
		&mut self,
		project_id: Uuid,
	) -> Result<Option<ProjectRecord>, StoreError> {
		let project_id = project_id.to_string();
		let row = sqlx::query_as!(
			Row,
			r#"SELECT project_id AS "project_id!", root, actor_kind, actor_id,
				registered_at_unix_ms
			 FROM projects
			 WHERE project_id = ?1"#,
			project_id
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(read_row).transpose()
	}

	/// The Project registered at exactly `root`, if any.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn project_by_root(
		&mut self,
		root: &str,
	) -> Result<Option<ProjectRecord>, StoreError> {
		let row = sqlx::query_as!(
			Row,
			r#"SELECT project_id AS "project_id!", root, actor_kind, actor_id,
				registered_at_unix_ms
			 FROM projects
			 WHERE root = ?1"#,
			root
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(read_row).transpose()
	}
}

impl WriteTransaction {
	/// Records a new Project and returns it as stored.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written, including
	/// when another Project already holds the same root.
	pub async fn insert_project(
		&mut self,
		project: NewProject,
	) -> Result<ProjectRecord, StoreError> {
		let NewProject {
			project_id,
			root,
			registered_by,
			registered_at_unix_ms,
		} = project;
		let id = project_id.to_string();
		let (actor_kind, actor_id) = registered_by.columns();
		let actor_id = actor_id.to_string();
		sqlx::query!(
			"INSERT INTO projects
				(project_id, root, actor_kind, actor_id, registered_at_unix_ms)
			 VALUES (?1, ?2, ?3, ?4, ?5)",
			id,
			root,
			actor_kind,
			actor_id,
			registered_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(ProjectRecord {
			project_id,
			root,
			registered_by,
			registered_at_unix_ms,
		})
	}
}

fn read_row(row: Row) -> Result<ProjectRecord, StoreError> {
	Ok(ProjectRecord {
		project_id: parse_uuid("project_id", &row.project_id)?,
		root: row.root,
		registered_by: ActorRecord::parse(&row.actor_kind, &row.actor_id)?,
		registered_at_unix_ms: row.registered_at_unix_ms,
	})
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod tests;
