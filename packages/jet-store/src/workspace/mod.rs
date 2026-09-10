//! Managed Workspaces: the isolated Git worktrees Conversations work in
//! (ADR-0025).
//!
//! A row keeps what the Workspace was made from and where it is: the
//! Project, the base the user selected, the commit that selection resolved
//! to when the Workspace was created, the Jet-owned root, and the seed of
//! Local-checkout changes it was given, if any. The worktree itself is
//! filesystem state the core creates beside the row.

pub(crate) mod promotion;
pub(crate) mod user_edit_intent;

use crate::{
	StoreError,
	records::{column_error, parse_uuid},
	transaction::{ReadTransaction, WriteTransaction},
};
use uuid::Uuid;

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
	/// What it was seeded with from the Local checkout, if anything.
	pub seed: Option<WorkspaceSeedRecord>,
	/// When the caller recorded the Workspace.
	pub created_at_unix_ms: i64,
}

/// The Local-checkout changes a Workspace was seeded with, as an immutable
/// Git tree the core captured and applied over the base (ADR-0025).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSeedRecord {
	/// The tree object the changes were captured as, as Git spells it.
	pub tree: String,
	/// How many paths that tree changes against the base commit.
	pub changed_paths: u32,
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
	/// What it was seeded with from the Local checkout, if anything.
	pub seed: Option<WorkspaceSeedRecord>,
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
	seed_tree: Option<String>,
	seed_changed_paths: Option<i64>,
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
				project_id, root, base_selection, base_commit, seed_tree,
				seed_changed_paths, created_at_unix_ms
			 FROM workspaces
			 WHERE conversation_id = ?1"#,
			conversation_id
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(read_row).transpose()
	}

	/// The Workspace identified by `workspace_id`, if recorded.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn workspace(
		&mut self,
		workspace_id: Uuid,
	) -> Result<Option<WorkspaceRecord>, StoreError> {
		let workspace_id = workspace_id.to_string();
		let row = sqlx::query_as!(
			Row,
			r#"SELECT workspace_id AS "workspace_id!", conversation_id,
				project_id, root, base_selection, base_commit, seed_tree,
				seed_changed_paths, created_at_unix_ms
			 FROM workspaces
			 WHERE workspace_id = ?1"#,
			workspace_id
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
			seed,
			created_at_unix_ms,
		} = workspace;
		let id = workspace_id.to_string();
		let conversation = conversation_id.to_string();
		let project = project_id.to_string();
		let seed_tree = seed.as_ref().map(|seed| seed.tree.as_str());
		let seed_changed_paths =
			seed.as_ref().map(|seed| i64::from(seed.changed_paths));
		sqlx::query!(
			"INSERT INTO workspaces
				(workspace_id, conversation_id, project_id, root,
				base_selection, base_commit, seed_tree, seed_changed_paths,
				created_at_unix_ms)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
			id,
			conversation,
			project,
			root,
			base_selection,
			base_commit,
			seed_tree,
			seed_changed_paths,
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
			seed,
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
		seed: read_seed(row.seed_tree, row.seed_changed_paths)?,
		created_at_unix_ms: row.created_at_unix_ms,
	})
}

/// The two seed columns are NULL together or set together; the schema
/// says so, and a row that says otherwise is an integrity failure.
fn read_seed(
	tree: Option<String>,
	changed_paths: Option<i64>,
) -> Result<Option<WorkspaceSeedRecord>, StoreError> {
	match (tree, changed_paths) {
		(None, None) => Ok(None),
		(Some(tree), Some(changed_paths)) => {
			let changed_paths = u32::try_from(changed_paths).map_err(|_| {
				column_error(
					"seed_changed_paths",
					format!("{changed_paths} is not a path count"),
				)
			})?;
			Ok(Some(WorkspaceSeedRecord {
				tree,
				changed_paths,
			}))
		}
		(tree, changed_paths) => Err(column_error(
			"seed_tree",
			format!(
				"seed tree {tree:?} with {changed_paths:?} changed paths is \
				 not a recorded combination"
			),
		)),
	}
}

#[cfg(test)]
mod tests {
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use super::{NewWorkspace, WorkspaceRecord, WorkspaceSeedRecord};
	use crate::{
		ActorRecord, ConversationOriginRecord, NameRecord, NameSourceRecord,
		NewConversation, NewProject, NewRun, RetentionPolicy, RunLifecycle,
		RunRecord, Store, StoreError, WorkingTreeRecord,
	};

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

	fn conversation(working_tree: WorkingTreeRecord) -> NewConversation {
		NewConversation {
			conversation_id: Uuid::now_v7(),
			retention: RetentionPolicy::Retain,
			working_tree,
			origin: ConversationOriginRecord::New,
			created_at_unix_ms: NOW_UNIX_MS,
		}
	}

	fn workspace(
		conversation: &NewConversation,
		project_id: Uuid,
		root: &str,
	) -> NewWorkspace {
		NewWorkspace {
			workspace_id: Uuid::now_v7(),
			conversation_id: conversation.conversation_id,
			project_id,
			root: root.into(),
			base_selection: "main".into(),
			base_commit: "0123456789abcdef0123456789abcdef01234567".into(),
			seed: None,
			created_at_unix_ms: NOW_UNIX_MS,
		}
	}

	fn seeded(workspace: NewWorkspace) -> NewWorkspace {
		NewWorkspace {
			seed: Some(WorkspaceSeedRecord {
				tree: "89abcdef0123456789abcdef0123456789abcdef".into(),
				changed_paths: 3,
			}),
			..workspace
		}
	}

	fn recorded(workspace: &NewWorkspace) -> WorkspaceRecord {
		WorkspaceRecord {
			workspace_id: workspace.workspace_id,
			conversation_id: workspace.conversation_id,
			project_id: workspace.project_id,
			root: workspace.root.clone(),
			base_selection: workspace.base_selection.clone(),
			base_commit: workspace.base_commit.clone(),
			seed: workspace.seed.clone(),
			created_at_unix_ms: workspace.created_at_unix_ms,
		}
	}

	fn run(conversation: &NewConversation) -> NewRun {
		NewRun {
			run_id: Uuid::now_v7(),
			conversation_id: conversation.conversation_id,
			created_at_unix_ms: NOW_UNIX_MS,
		}
	}

	/// A Workspace belongs to one Conversation and one root, both the
	/// Conversation and its Workspace remember the Project, and a Workspace
	/// keeps the seed it was given or the absence of one (ADR-0025).
	#[tokio::test]
	async fn a_workspace_is_owned_by_one_conversation_at_one_root() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let project = project("/home/jet/repo");
		let owner = conversation(WorkingTreeRecord::Workspace {
			project_id: project.project_id,
		});
		let other = conversation(WorkingTreeRecord::Workspace {
			project_id: project.project_id,
		});
		let unplaced = conversation(WorkingTreeRecord::NoProject);
		let owned =
			workspace(&owner, project.project_id, "/home/jet/.jet/ws/a");
		let grown = seeded(workspace(
			&other,
			project.project_id,
			"/home/jet/.jet/ws/b",
		));

		let (inserted, second_for_owner, same_root, read_back, seeded, none) =
			store
				.write(async |tx| {
					tx.insert_project(project.clone()).await?;
					for conversation in [owner, other, unplaced] {
						tx.insert_conversation(conversation).await?;
					}
					let inserted = tx.insert_workspace(owned.clone()).await?;
					let second_for_owner = tx
						.insert_workspace(workspace(
							&owner,
							project.project_id,
							"/home/jet/.jet/ws/b",
						))
						.await
						.is_err();
					let same_root = tx
						.insert_workspace(workspace(
							&other,
							project.project_id,
							"/home/jet/.jet/ws/a",
						))
						.await
						.is_err();
					let read_back =
						tx.workspace_of(owner.conversation_id).await?;
					tx.insert_workspace(grown.clone()).await?;
					let seeded = tx.workspace_of(other.conversation_id).await?;
					let none =
						tx.workspace_of(unplaced.conversation_id).await?;
					Ok::<_, StoreError>((
						inserted,
						second_for_owner,
						same_root,
						read_back,
						seeded,
						none,
					))
				})
				.await
				.unwrap();

		assert_eq!(
			(
				inserted,
				second_for_owner,
				same_root,
				read_back,
				seeded,
				none
			),
			(
				recorded(&owned),
				true,
				true,
				Some(recorded(&owned)),
				Some(recorded(&grown)),
				None
			)
		);
	}

	/// A Conversation keeps where it works across a reopen, and the Runs in a
	/// Project's Local checkout are found through it (ADR-0025).
	#[tokio::test]
	async fn local_checkout_runs_are_found_through_their_conversations() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let project = project("/home/jet/repo");
		let elsewhere = self::project("/home/jet/other");
		let local = conversation(WorkingTreeRecord::LocalCheckout {
			project_id: project.project_id,
		});
		let isolated = conversation(WorkingTreeRecord::Workspace {
			project_id: project.project_id,
		});
		let other_local = conversation(WorkingTreeRecord::LocalCheckout {
			project_id: elsewhere.project_id,
		});
		let local_run = run(&local);

		let store = Store::open(&path).await.unwrap();
		store
			.write(async |tx| {
				tx.insert_project(project.clone()).await?;
				tx.insert_project(elsewhere.clone()).await?;
				for conversation in [local, isolated, other_local] {
					tx.insert_conversation(conversation).await?;
				}
				tx.insert_run(local_run).await?;
				tx.insert_run(run(&isolated)).await?;
				tx.insert_run(run(&other_local)).await
			})
			.await
			.unwrap();
		store.close().await;

		let store = Store::open(&path).await.unwrap();
		let (placed, runs) = store
			.read(async |tx| {
				Ok::<_, StoreError>((
					tx.conversation(local.conversation_id).await?.unwrap(),
					tx.local_checkout_runs(project.project_id).await?,
				))
			})
			.await
			.unwrap();

		assert_eq!(
			(placed.working_tree, runs),
			(
				WorkingTreeRecord::LocalCheckout {
					project_id: project.project_id,
				},
				vec![RunRecord {
					run_id: local_run.run_id,
					conversation_id: local.conversation_id,
					revision: 1,
					lifecycle: RunLifecycle::Created,
					name: NameRecord {
						value: format!(
							"Run {}",
							&local_run.run_id.simple().to_string()[..8]
						),
						source: NameSourceRecord::Deterministic,
					},
					created_at_unix_ms: NOW_UNIX_MS,
					ended_at_unix_ms: None,
				}]
			)
		);
	}
}
