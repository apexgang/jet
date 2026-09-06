use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{NewWorkspace, WorkspaceRecord};
use crate::{
	ActorRecord, NewConversation, NewProject, RetentionPolicy, Store,
	StoreError, WorkingTreeRecord,
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
		created_at_unix_ms: NOW_UNIX_MS,
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
		created_at_unix_ms: workspace.created_at_unix_ms,
	}
}

/// A Workspace belongs to one Conversation and one root, and both the
/// Conversation and its Workspace remember the Project (ADR-0025).
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

	let (inserted, second_for_owner, same_root, read_back, none) = store
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
			let none = tx.workspace_of(unplaced.conversation_id).await?;
			Ok::<_, StoreError>((
				inserted,
				second_for_owner,
				same_root,
				read_back,
				none,
			))
		})
		.await
		.unwrap();

	assert_eq!(
		(inserted, second_for_owner, same_root, read_back, none),
		(recorded(&owned), true, true, Some(recorded(&owned)), None)
	);
}
