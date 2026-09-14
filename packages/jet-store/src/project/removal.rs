//! Removing a registered Project and what its removal has to know
//! (ADR-0011, ADR-0102).
//!
//! The removal takes the Project's Workspaces and what hangs off them,
//! detaches every Conversation that worked in the Project, and removes
//! the registration itself. Reapplying the Deletion ledger at open runs
//! the same statements, so a restored snapshot loses exactly what the
//! removal removed. The Conversations stay: their journals, Runs, and
//! names are theirs, not the Project's.

use crate::{
	StoreError,
	deletion::{DeletedIdentityKind, PendingDeletion},
	transaction::{ReadTransaction, WriteTransaction},
};
use sqlx::SqliteConnection;
use uuid::Uuid;

/// What a removal preview counts in the store about one Project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectWork {
	/// Runs of its Conversations that have not ended.
	pub live_runs: u64,
	/// Scheduled tasks of its Conversations.
	pub schedules: u64,
}

impl ReadTransaction {
	/// The live Runs and schedules of the Conversations that work in
	/// `project_id`, in a Workspace or in its Local checkout.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn project_work(
		&mut self,
		project_id: Uuid,
	) -> Result<ProjectWork, StoreError> {
		let project_id = project_id.to_string();
		// The four spellings are the ones `RunLifecycle::is_terminal`
		// answers false for; the macro takes a literal and no other form.
		let live_runs = sqlx::query_scalar!(
			r#"SELECT COUNT(*) AS "count!: i64" FROM runs
			 JOIN conversations USING (conversation_id)
			 WHERE conversations.project_id = ?1
			   AND runs.lifecycle IN ('created', 'starting', 'active', 'stopping')"#,
			project_id
		)
		.fetch_one(self.connection())
		.await?;
		let schedules = sqlx::query_scalar!(
			r#"SELECT COUNT(*) AS "count!: i64" FROM scheduled_tasks
			 JOIN conversations USING (conversation_id)
			 WHERE conversations.project_id = ?1"#,
			project_id
		)
		.fetch_one(self.connection())
		.await?;
		Ok(ProjectWork {
			live_runs: u64::try_from(live_runs).unwrap_or(0),
			schedules: u64::try_from(schedules).unwrap_or(0),
		})
	}
}

impl WriteTransaction {
	/// Removes the Project `project_id`, its Workspaces, and what hangs
	/// off them, detaches its Conversations, and records the identity for
	/// the Deletion ledger, which the commit writes first (ADR-0102).
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when a row cannot be removed.
	pub async fn delete_project(
		&mut self,
		project_id: Uuid,
		deleted_at_unix_ms: i64,
	) -> Result<(), StoreError> {
		let id = project_id.to_string();
		purge_rows(self.connection(), &id).await?;
		self.record_deletion(PendingDeletion {
			kind: DeletedIdentityKind::Project,
			identity: project_id,
			deleted_at_unix_ms,
		});
		Ok(())
	}
}

/// Removes every row of the Project `id`, dependents first, and detaches
/// its Conversations. Reapplying the ledger at open uses the same
/// statements. The Workspace half mirrors `conversation::purge::purge_rows`
/// selected by Project instead of Conversation: the macros take literal
/// SQL, so the two are kept side by side rather than shared.
pub(crate) async fn purge_rows(
	connection: &mut SqliteConnection,
	id: &str,
) -> Result<u64, StoreError> {
	let mut changed = 0;
	changed += sqlx::query!(
		"DELETE FROM workspace_promotion_conflicts WHERE promotion_id IN (
			SELECT p.promotion_id FROM workspace_promotions p
			JOIN workspaces w ON w.workspace_id = p.workspace_id
			WHERE w.project_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM effects WHERE promotion_id IN (
			SELECT p.promotion_id FROM workspace_promotions p
			JOIN workspaces w ON w.workspace_id = p.workspace_id
			WHERE w.project_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM workspace_promotions WHERE workspace_id IN (
			SELECT workspace_id FROM workspaces WHERE project_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM effects WHERE terminal_id IN (
			SELECT t.terminal_id FROM workspace_terminals t
			JOIN workspaces w ON w.workspace_id = t.workspace_id
			WHERE w.project_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM workspace_terminals WHERE workspace_id IN (
			SELECT workspace_id FROM workspaces WHERE project_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!("DELETE FROM workspaces WHERE project_id = ?1", id)
		.execute(&mut *connection)
		.await?
		.rows_affected();
	// 'none' is the spelling `WorkingTreeRecord::columns` gives a
	// Conversation in no Project.
	changed += sqlx::query!(
		"UPDATE conversations SET working_tree = 'none', project_id = NULL
		 WHERE project_id = ?1",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!("DELETE FROM projects WHERE project_id = ?1", id)
		.execute(&mut *connection)
		.await?
		.rows_affected();
	Ok(changed)
}

#[cfg(test)]
mod tests {
	use super::ProjectWork;
	use crate::{
		ActorRecord, ConversationOriginRecord, DeletedIdentityKind,
		DeletionLedger, NewConversation, NewProject, NewRun, NewWorkspace,
		RetentionPolicy, RunLifecycle, Store, StoreError, WorkingTreeRecord,
	};
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

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

	fn workspace(conversation_id: Uuid, project_id: Uuid) -> NewWorkspace {
		NewWorkspace {
			workspace_id: Uuid::now_v7(),
			conversation_id,
			project_id,
			root: format!("/jet/workspaces/{conversation_id}"),
			base_selection: "main".into(),
			base_commit: "0".repeat(40),
			seed: None,
			created_at_unix_ms: NOW_UNIX_MS,
		}
	}

	/// The preview counts the Project's live Runs and schedules across its
	/// Workspaces and Local checkout; the removal takes the Workspaces,
	/// detaches the Conversations, keeps them, and reaches the ledger.
	#[tokio::test]
	async fn removal_counts_work_detaches_conversations_and_reaches_the_ledger()
	{
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let removed = project("/home/jet/removed");
		let kept = project("/home/jet/kept");
		let (removed_id, kept_id) = (removed.project_id, kept.project_id);
		let in_workspace = conversation(WorkingTreeRecord::Workspace {
			project_id: removed_id,
		});
		let in_checkout = conversation(WorkingTreeRecord::LocalCheckout {
			project_id: removed_id,
		});
		let elsewhere = conversation(WorkingTreeRecord::Workspace {
			project_id: kept_id,
		});
		let ids = [
			in_workspace.conversation_id,
			in_checkout.conversation_id,
			elsewhere.conversation_id,
		];
		let removed_workspace = workspace(ids[0], removed_id);
		let kept_workspace = workspace(ids[2], kept_id);
		let (work_before, workspaces_before) = store
			.write(async |tx| {
				tx.insert_project(removed).await?;
				tx.insert_project(kept).await?;
				for new in [in_workspace, in_checkout, elsewhere] {
					tx.insert_conversation(new).await?;
				}
				tx.insert_workspace(removed_workspace.clone()).await?;
				tx.insert_workspace(kept_workspace.clone()).await?;
				for conversation_id in ids {
					let run = tx
						.insert_run(NewRun {
							run_id: Uuid::now_v7(),
							conversation_id,
							created_at_unix_ms: NOW_UNIX_MS,
						})
						.await?;
					if conversation_id == ids[1] {
						tx.update_run_lifecycle(
							run.run_id,
							RunLifecycle::Completed,
							NOW_UNIX_MS,
						)
						.await?;
					}
				}
				tx.save_schedule(Uuid::now_v7(), ids[0], NOW_UNIX_MS, "{}")
					.await?;
				tx.save_schedule(Uuid::now_v7(), ids[2], NOW_UNIX_MS, "{}")
					.await?;
				Ok::<_, StoreError>((
					tx.project_work(removed_id).await?,
					tx.workspaces_of_project(removed_id).await?,
				))
			})
			.await
			.unwrap();
		store
			.write(async |tx| tx.delete_project(removed_id, NOW_UNIX_MS).await)
			.await
			.unwrap();
		let (projects, workspaces, conversations, kept_work) = store
			.read(async |tx| {
				let mut conversations = Vec::new();
				for conversation_id in ids {
					conversations.push(
						tx.conversation(conversation_id)
							.await?
							.map(|record| record.working_tree),
					);
				}
				Ok::<_, StoreError>((
					tx.projects().await?,
					tx.workspaces_of_project(removed_id).await?,
					conversations,
					tx.project_work(kept_id).await?,
				))
			})
			.await
			.unwrap();
		let DeletionLedger::Verified(ledger) = store.deletion_ledger().unwrap()
		else {
			panic!("ledger is corrupt");
		};

		assert_eq!(
			(
				work_before,
				workspaces_before
					.iter()
					.map(|workspace| workspace.workspace_id)
					.collect::<Vec<_>>(),
				projects
					.iter()
					.map(|project| project.project_id)
					.collect::<Vec<_>>(),
				workspaces,
				conversations,
				kept_work,
				ledger
					.iter()
					.map(|record| (record.kind, record.identity))
					.collect::<Vec<_>>(),
			),
			(
				ProjectWork {
					live_runs: 1,
					schedules: 1,
				},
				vec![removed_workspace.workspace_id],
				vec![kept_id],
				vec![],
				vec![
					Some(WorkingTreeRecord::NoProject),
					Some(WorkingTreeRecord::NoProject),
					Some(WorkingTreeRecord::Workspace {
						project_id: kept_id
					}),
				],
				ProjectWork {
					live_runs: 1,
					schedules: 1,
				},
				vec![(DeletedIdentityKind::Project, removed_id)],
			)
		);
	}
}
