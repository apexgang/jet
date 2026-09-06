use std::path::Path;

use pretty_assertions::assert_eq;

use crate::test_support::{
	actor, git, register_repository, request, start_core,
};
use crate::{
	BaseSelection, Command, CommandOutcome, Conversation, ConversationId,
	ConversationSnapshot, Core, CoreError, ErrorCategory, EventKind,
	EventSequence, ProjectId, Query, QueryResult, RetentionPolicy, Run,
	RunLifecycle, WorkingTree, WorkingTreeRequest, Workspace, WorkspaceBase,
};

async fn start(dir: &tempfile::TempDir) -> Core {
	start_core(&dir.path().join("plane.sqlite3")).await
}

async fn create(
	core: &Core,
	working_tree: WorkingTreeRequest,
) -> Result<Conversation, CoreError> {
	let outcome = core
		.execute(
			&actor(),
			request(Command::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree,
			}),
		)
		.await?;
	let CommandOutcome::ConversationCreated(conversation) = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	Ok(conversation)
}

async fn create_run(
	core: &Core,
	conversation_id: ConversationId,
) -> Result<Run, CoreError> {
	let outcome = core
		.execute(&actor(), request(Command::CreateRun { conversation_id }))
		.await?;
	let CommandOutcome::RunCreated(run) = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	Ok(run)
}

async fn snapshot(
	core: &Core,
	conversation_id: ConversationId,
) -> ConversationSnapshot {
	let result = core
		.query(&actor(), Query::Conversation { conversation_id })
		.await
		.unwrap();
	let QueryResult::Conversation(snapshot) = result else {
		panic!("unexpected result {result:?}");
	};
	snapshot
}

async fn events(core: &Core) -> Vec<EventKind> {
	let result = core
		.query(
			&actor(),
			Query::Events {
				after: EventSequence(0),
			},
		)
		.await
		.unwrap();
	let QueryResult::Events(page) = result else {
		panic!("unexpected result {result:?}");
	};
	page.events.into_iter().map(|event| event.kind).collect()
}

fn in_workspace(
	project_id: ProjectId,
	base: BaseSelection,
) -> WorkingTreeRequest {
	WorkingTreeRequest::Workspace { project_id, base }
}

fn in_local_checkout(project_id: ProjectId) -> WorkingTreeRequest {
	WorkingTreeRequest::LocalCheckout { project_id }
}

/// What `git` says the worktree at `root` has checked out, and whether
/// its HEAD is detached.
fn checked_out(root: &Path) -> (String, bool) {
	let head = git(root, &["rev-parse", "HEAD"]).trim().to_owned();
	let symbolic = std::process::Command::new("git")
		.arg("-C")
		.arg(root)
		.args(["symbolic-ref", "-q", "HEAD"])
		.output()
		.unwrap();
	(head, !symbolic.status.success())
}

/// A managed Conversation receives a Workspace of its own: a worktree at a
/// deterministic Jet-owned root, detached at the commit its selected base
/// resolved to, recorded with its Project and journaled (ADR-0025).
#[tokio::test]
async fn a_managed_conversation_receives_a_detached_workspace_of_its_own() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let project_id = register_repository(&core, &dir.path().join("repo")).await;
	let repository = dir.path().join("repo").canonicalize().unwrap();
	let main = git(&repository, &["rev-parse", "HEAD"]).trim().to_owned();
	git(&repository, &["checkout", "-q", "-b", "topic"]);
	std::fs::write(repository.join("topic.txt"), "topic\n").unwrap();
	git(&repository, &["add", "-A"]);
	git(&repository, &["commit", "-q", "-m", "Topic"]);
	let topic = git(&repository, &["rev-parse", "HEAD"]).trim().to_owned();

	let first = create(
		&core,
		in_workspace(project_id, BaseSelection::Revision("main".into())),
	)
	.await
	.unwrap();
	let second = create(&core, in_workspace(project_id, BaseSelection::Head))
		.await
		.unwrap();
	let first_snapshot = snapshot(&core, first.conversation_id).await;
	let second_snapshot = snapshot(&core, second.conversation_id).await;
	let journal = events(&core).await;

	let workspaces_dir = dir.path().join("workspaces");
	let first_root = workspaces_dir.join(first.conversation_id.0.to_string());
	let second_root = workspaces_dir.join(second.conversation_id.0.to_string());
	let first_workspace = first_snapshot.workspace.clone().unwrap();
	let second_workspace = second_snapshot.workspace.clone().unwrap();
	assert_eq!(
		(
			first.working_tree,
			&first_workspace,
			&second_workspace,
			checked_out(&first_root),
			checked_out(&second_root),
			first_root.join("README.md").is_file(),
			second_root.join("topic.txt").is_file(),
			first_root.join("topic.txt").exists(),
			&journal[1..],
		),
		(
			WorkingTree::Workspace { project_id },
			&Workspace {
				workspace_id: first_workspace.workspace_id,
				conversation_id: first.conversation_id,
				project_id,
				root: first_root.clone(),
				base: WorkspaceBase {
					selection: BaseSelection::Revision("main".into()),
					commit: main.clone(),
				},
				created_at: first.created_at,
			},
			&Workspace {
				workspace_id: second_workspace.workspace_id,
				conversation_id: second.conversation_id,
				project_id,
				root: second_root.clone(),
				base: WorkspaceBase {
					selection: BaseSelection::Head,
					commit: topic.clone(),
				},
				created_at: second.created_at,
			},
			(main.clone(), true),
			(topic.clone(), true),
			true,
			true,
			false,
			&[
				EventKind::ConversationCreated {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTree::Workspace { project_id },
				},
				EventKind::WorkspaceCreated {
					workspace_id: first_workspace.workspace_id,
					project_id,
					root: first_root,
					base: first_workspace.base.clone(),
				},
				EventKind::ConversationCreated {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTree::Workspace { project_id },
				},
				EventKind::WorkspaceCreated {
					workspace_id: second_workspace.workspace_id,
					project_id,
					root: second_root,
					base: second_workspace.base.clone(),
				},
			][..],
		)
	);
}

/// A Workspace is refused, with nothing recorded and nothing on disk, when
/// its Project is unknown or its base names no commit; and a selection
/// Git could read as more than a revision never reaches Git.
#[tokio::test]
async fn a_workspace_that_cannot_start_is_refused_without_a_trace() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let project_id = register_repository(&core, &dir.path().join("repo")).await;

	let unknown = create(
		&core,
		in_workspace(ProjectId(uuid::Uuid::nil()), BaseSelection::Head),
	)
	.await
	.unwrap_err();
	let no_commit = create(
		&core,
		in_workspace(project_id, BaseSelection::Revision("nowhere".into())),
	)
	.await
	.unwrap_err();
	let malformed = create(
		&core,
		in_workspace(project_id, BaseSelection::Revision("main\nHEAD".into())),
	)
	.await
	.unwrap_err();
	let journal = events(&core).await;

	assert_eq!(
		(
			(unknown.category, unknown.code),
			(no_commit.category, no_commit.code),
			(malformed.category, malformed.code),
			journal.len(),
			dir.path().join("workspaces").exists(),
		),
		(
			(ErrorCategory::NotFound, "project.not_found".into()),
			(ErrorCategory::NotFound, "workspace.base_not_found".into()),
			(ErrorCategory::InvalidInput, "workspace.base_invalid".into()),
			1,
			false,
		)
	);
}

/// A Local checkout is explicit and unisolated: it admits one live managed
/// Run at a time, refuses the next while saying why, and admits it again
/// once the first has ended. Workspaces of the same Project are never
/// counted against it (ADR-0025).
#[tokio::test]
async fn a_local_checkout_admits_one_live_managed_run_at_a_time() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let project_id = register_repository(&core, &dir.path().join("repo")).await;
	let other_project =
		register_repository(&core, &dir.path().join("other")).await;
	let first = create(&core, in_local_checkout(project_id)).await.unwrap();
	let second = create(&core, in_local_checkout(project_id)).await.unwrap();
	let isolated = create(&core, in_workspace(project_id, BaseSelection::Head))
		.await
		.unwrap();
	let elsewhere = create(&core, in_local_checkout(other_project))
		.await
		.unwrap();

	let first_run = create_run(&core, first.conversation_id).await.unwrap();
	let refused = create_run(&core, second.conversation_id).await.unwrap_err();
	let isolated_run = create_run(&core, isolated.conversation_id).await;
	let elsewhere_run = create_run(&core, elsewhere.conversation_id).await;
	core.execute(
		&actor(),
		request(Command::TransitionRun {
			run_id: first_run.run_id,
			expected_revision: first_run.revision,
			lifecycle: RunLifecycle::Canceled,
		}),
	)
	.await
	.unwrap();
	let admitted = create_run(&core, second.conversation_id).await;

	assert_eq!(
		(
			first.working_tree,
			snapshot(&core, first.conversation_id).await.workspace,
			(refused.category, refused.code.as_str()),
			refused.message.contains("outside its management"),
			isolated_run.is_ok(),
			elsewhere_run.is_ok(),
			admitted.map(|run| run.conversation_id),
		),
		(
			WorkingTree::LocalCheckout { project_id },
			None,
			(ErrorCategory::Conflict, "run.local_checkout_busy"),
			true,
			true,
			true,
			Ok(second.conversation_id),
		)
	);
}
