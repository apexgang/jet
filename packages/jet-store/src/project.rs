//! Registered Projects: the working trees an interactive user granted Jet
//! access to (ADR-0025, ADR-0101).
//!
//! A row keeps the canonical absolute root the grant resolved to and the
//! Actor that made it. What the repository looked like at registration is
//! observed, not stored: it describes the working tree, which changes
//! without Jet.

use crate::{
	StoreError,
	records::{ActorRecord, parse_uuid},
	transaction::{ReadTransaction, WriteTransaction},
};
use uuid::Uuid;

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
mod tests {
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use super::{NewProject, ProjectRecord};
	use crate::{ActorRecord, Store, StoreError};

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;

	fn project(root: &str) -> NewProject {
		NewProject {
			project_id: Uuid::now_v7(),
			root: root.into(),
			registered_by: ActorRecord::InteractiveClient {
				client_id: Uuid::nil(),
			},
			registered_at_unix_ms: NOW_UNIX_MS,
		}
	}

	fn recorded(project: &NewProject) -> ProjectRecord {
		ProjectRecord {
			project_id: project.project_id,
			root: project.root.clone(),
			registered_by: project.registered_by,
			registered_at_unix_ms: project.registered_at_unix_ms,
		}
	}

	#[tokio::test]
	async fn projects_outlive_the_daemon_that_registered_them() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let first_project = project("/home/jet/first");
		let second_project = project("/home/jet/second");

		let first = Store::open(&path).await.unwrap();
		first
			.write(async |tx| {
				tx.insert_project(first_project.clone()).await?;
				tx.insert_project(second_project.clone()).await
			})
			.await
			.unwrap();
		first.close().await;

		let second = Store::open(&path).await.unwrap();
		let (listed, by_id, by_root, unknown) = second
			.read(async |tx| {
				Ok::<_, StoreError>((
					tx.projects().await?,
					tx.project(second_project.project_id).await?,
					tx.project_by_root("/home/jet/first").await?,
					tx.project_by_root("/home/jet/third").await?,
				))
			})
			.await
			.unwrap();

		assert_eq!(
			(listed, by_id, by_root, unknown),
			(
				vec![recorded(&first_project), recorded(&second_project)],
				Some(recorded(&second_project)),
				Some(recorded(&first_project)),
				None
			)
		);
	}

	/// One root is one Project. The core checks before it inserts; the schema
	/// refuses a second row for the same root even so, so a race or a bug
	/// cannot leave two Projects claiming one directory.
	#[tokio::test]
	async fn a_root_cannot_be_registered_twice() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let first = project("/home/jet/repo");
		let again = project("/home/jet/repo");

		store
			.write(async |tx| tx.insert_project(first.clone()).await)
			.await
			.unwrap();
		let refused = store
			.write(async |tx| tx.insert_project(again).await)
			.await
			.unwrap_err();
		let listed = store.read(async |tx| tx.projects().await).await.unwrap();

		assert_eq!(
			(matches!(refused, StoreError::Integrity(_)), listed),
			(true, vec![recorded(&first)])
		);
	}
}
