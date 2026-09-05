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
