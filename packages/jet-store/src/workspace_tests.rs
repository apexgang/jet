use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{NewWorkspace, WorkspaceRecord, WorkspaceSeedRecord};
use crate::{
	ActorRecord, NewConversation, NewProject, NewRun, RetentionPolicy,
	RunLifecycle, RunRecord, Store, StoreError, WorkingTreeRecord,
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
	let owned = workspace(&owner, project.project_id, "/home/jet/.jet/ws/a");
	let grown =
		seeded(workspace(&other, project.project_id, "/home/jet/.jet/ws/b"));

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
				let read_back = tx.workspace_of(owner.conversation_id).await?;
				tx.insert_workspace(grown.clone()).await?;
				let seeded = tx.workspace_of(other.conversation_id).await?;
				let none = tx.workspace_of(unplaced.conversation_id).await?;
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
				created_at_unix_ms: NOW_UNIX_MS,
				ended_at_unix_ms: None,
			}]
		)
	);
}
