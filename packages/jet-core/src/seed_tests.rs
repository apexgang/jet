use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use crate::test_support::{
	actor, git, init_repository, register_repository, request, start_core,
};
use crate::{
	BaseSelection, Command, CommandOutcome, Conversation, ConversationId,
	ConversationSnapshot, Core, CoreError, ErrorCategory, EventKind,
	EventSequence, ProjectId, Query, QueryResult, RelativePath,
	RetentionPolicy, SeedSelection, WorkingTreeRequest, WorkspaceSeed,
};

/// A Project whose Local checkout holds every kind of change a seed meets:
/// a modified, a deleted, a staged-only, and a new file; an ignored file, a
/// directory mixing unignored and ignored files, an ignored directory, and
/// an unignored directory of ignored files only; a dangling symbolic link;
/// a submodule moved to a later commit and dirtied; and a repository
/// nested inside the working tree.
struct BusyCheckout {
	core: Core,
	project_id: ProjectId,
	repository: PathBuf,
	/// The commit the submodule was recorded at in the base.
	submodule_base: String,
	/// The commit the submodule has checked out now.
	submodule_now: String,
}

async fn busy_checkout(dir: &Path) -> BusyCheckout {
	let core = start_core(&dir.join("plane.sqlite3")).await;
	let project_id = register_repository(&core, &dir.join("repo")).await;
	let repository = dir.join("repo").canonicalize().unwrap();
	let source = init_repository(&dir.join("sub-src"));
	let submodule_base = git(&source, &["rev-parse", "HEAD"]).trim().to_owned();
	std::fs::write(source.join("second.txt"), "second\n").unwrap();
	git(&source, &["add", "-A"]);
	git(&source, &["commit", "-q", "-m", "Second"]);
	let submodule_now = git(&source, &["rev-parse", "HEAD"]).trim().to_owned();

	git(
		&repository,
		&["submodule", "add", "-q", source.to_str().unwrap(), "sub"],
	);
	git(
		&repository.join("sub"),
		&["checkout", "-q", &submodule_base],
	);
	std::fs::write(repository.join(".gitignore"), "*.log\nlogs/\n").unwrap();
	std::fs::write(repository.join("tracked.txt"), "hello\n").unwrap();
	std::fs::write(repository.join("gone.txt"), "gone\n").unwrap();
	std::fs::create_dir(repository.join("d")).unwrap();
	std::fs::write(repository.join("d/keep.txt"), "keep\n").unwrap();
	git(&repository, &["add", "-A"]);
	git(&repository, &["commit", "-q", "-m", "Base"]);

	std::fs::write(repository.join("tracked.txt"), "changed\n").unwrap();
	std::fs::remove_file(repository.join("gone.txt")).unwrap();
	std::fs::write(repository.join("new.txt"), "new\n").unwrap();
	std::fs::write(repository.join("debug.log"), "log\n").unwrap();
	std::fs::write(repository.join("d/scratch.txt"), "scratch\n").unwrap();
	std::fs::write(repository.join("d/out.log"), "out\n").unwrap();
	std::fs::create_dir(repository.join("logs")).unwrap();
	std::fs::write(repository.join("logs/run.log"), "run\n").unwrap();
	std::fs::create_dir(repository.join("d2")).unwrap();
	std::fs::write(repository.join("d2/only.log"), "only\n").unwrap();
	std::os::unix::fs::symlink("/jet/nowhere", repository.join("link"))
		.unwrap();
	std::fs::write(repository.join("staged.txt"), "staged\n").unwrap();
	git(&repository, &["add", "staged.txt"]);
	git(&repository.join("sub"), &["checkout", "-q", &submodule_now]);
	std::fs::write(repository.join("sub/dirty.txt"), "dirty\n").unwrap();
	init_repository(&repository.join("nested"));

	BusyCheckout {
		core,
		project_id,
		repository,
		submodule_base,
		submodule_now,
	}
}

async fn create(
	core: &Core,
	project_id: ProjectId,
	base: BaseSelection,
	seed: SeedSelection,
) -> Result<Conversation, CoreError> {
	let outcome = core
		.execute(
			&actor(),
			request(Command::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRequest::Workspace {
					project_id,
					base,
					seed,
				},
			}),
		)
		.await?;
	let CommandOutcome::ConversationCreated(conversation) = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	Ok(conversation)
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

fn paths(paths: &[&str]) -> SeedSelection {
	SeedSelection::Paths(
		paths
			.iter()
			.map(|path| RelativePath::parse(path).unwrap())
			.collect(),
	)
}

fn read(path: PathBuf) -> Option<String> {
	std::fs::read_to_string(path).ok()
}

/// The paths `git status` reports in `root`, staged and unstaged alike,
/// so a checkout can be compared with itself before and after a capture.
fn status(root: &Path) -> String {
	git(root, &["status", "--porcelain", "--untracked-files=all"])
}

/// The commit a Workspace's index records for its submodule.
fn submodule_commit(root: &Path) -> String {
	git(root, &["rev-parse", ":sub"]).trim().to_owned()
}

/// Seeding with every eligible change brings tracked modifications and
/// deletions, staged and unstaged alike, and unignored new files, staged
/// into the Workspace; leaves ignored files out; keeps a symbolic link as
/// a link; moves the submodule's Git link without entering it; drops the
/// nested repository; records and journals what was applied; and leaves
/// the Local checkout exactly as it was (ADR-0025, ADR-0103).
#[tokio::test]
async fn all_eligible_changes_seed_the_workspace_and_leave_the_checkout_alone()
{
	let dir = tempfile::tempdir().unwrap();
	let checkout = busy_checkout(dir.path()).await;
	let before = status(&checkout.repository);
	let staged_before =
		git(&checkout.repository, &["diff", "--cached", "--name-only"]);

	let conversation = create(
		&checkout.core,
		checkout.project_id,
		BaseSelection::Head,
		SeedSelection::AllEligible,
	)
	.await
	.unwrap();
	let snapshot = snapshot(&checkout.core, conversation.conversation_id).await;
	let workspace = snapshot.workspace.unwrap();
	let root = workspace.root.clone();
	let seed = workspace.seed.clone().unwrap();
	let journal = events(&checkout.core).await;

	assert_eq!(
		(
			(
				read(root.join("tracked.txt")),
				root.join("gone.txt").exists(),
				read(root.join("new.txt")),
				read(root.join("staged.txt")),
				read(root.join("d/scratch.txt")),
				root.join("debug.log").exists(),
				root.join("d/out.log").exists(),
				root.join("logs").exists(),
				root.join("d2").exists(),
			),
			(
				std::fs::read_link(root.join("link")).ok(),
				submodule_commit(&root),
				root.join("sub/dirty.txt").exists(),
				git(&root, &["ls-files", "--", "nested"]),
				root.join("nested").exists(),
				git(&root, &["status", "--porcelain"]),
			),
			(
				&seed,
				journal.last().cloned(),
				status(&checkout.repository) == before,
				git(&checkout.repository, &["diff", "--cached", "--name-only"])
					== staged_before,
			),
		),
		(
			(
				Some("changed\n".into()),
				false,
				Some("new\n".into()),
				Some("staged\n".into()),
				Some("scratch\n".into()),
				false,
				false,
				false,
				false,
			),
			(
				Some(PathBuf::from("/jet/nowhere")),
				checkout.submodule_now.clone(),
				false,
				String::new(),
				false,
				"A  d/scratch.txt\nD  gone.txt\nA  link\nA  new.txt\nA  \
				 staged.txt\nM  sub\nM  tracked.txt\n"
					.to_owned(),
			),
			(
				&WorkspaceSeed {
					tree: seed.tree.clone(),
					changed_paths: 7,
				},
				Some(EventKind::WorkspaceSeeded {
					workspace_id: workspace.workspace_id,
					seed: seed.clone(),
				}),
				true,
				true,
			),
		)
	);
}

/// Seeding named paths brings those paths as the checkout has them and
/// nothing else: an ignored file arrives because it was named, an ignored
/// file inside a named directory does not, a named directory of ignored
/// files alone brings nothing, and unselected changes, staged or not, stay
/// behind (ADR-0025).
#[tokio::test]
async fn named_paths_seed_the_workspace_and_ignored_files_need_naming() {
	let dir = tempfile::tempdir().unwrap();
	let checkout = busy_checkout(dir.path()).await;

	let conversation = create(
		&checkout.core,
		checkout.project_id,
		BaseSelection::Head,
		paths(&["tracked.txt", "debug.log", "d", "logs/run.log", "d2"]),
	)
	.await
	.unwrap();
	let snapshot = snapshot(&checkout.core, conversation.conversation_id).await;
	let workspace = snapshot.workspace.unwrap();
	let root = workspace.root.clone();

	assert_eq!(
		(
			read(root.join("tracked.txt")),
			read(root.join("debug.log")),
			read(root.join("d/scratch.txt")),
			read(root.join("logs/run.log")),
			root.join("d/out.log").exists(),
			root.join("d2").exists(),
			root.join("new.txt").exists(),
			read(root.join("gone.txt")),
			root.join("staged.txt").exists(),
			submodule_commit(&root),
			workspace.seed.map(|seed| seed.changed_paths),
		),
		(
			Some("changed\n".into()),
			Some("log\n".into()),
			Some("scratch\n".into()),
			Some("run\n".into()),
			false,
			false,
			false,
			Some("gone\n".into()),
			false,
			checkout.submodule_base.clone(),
			Some(4),
		)
	);
}

/// A seed is refused, with no Workspace and nothing left in the Workspace
/// home, when the checkout is not at the selected base, when a named path
/// is missing, lies in a submodule, or is a nested repository, and when
/// the selection itself is out of bounds (ADR-0025, ADR-0103).
#[tokio::test]
async fn a_seed_that_cannot_be_taken_is_refused_without_a_trace() {
	let dir = tempfile::tempdir().unwrap();
	let checkout = busy_checkout(dir.path()).await;
	let core = &checkout.core;
	let project_id = checkout.project_id;
	let head = BaseSelection::Head;
	let mut refusals = Vec::new();
	for (base, seed) in [
		(
			BaseSelection::Revision("HEAD~1".into()),
			SeedSelection::AllEligible,
		),
		(head.clone(), paths(&["nothere.txt"])),
		(head.clone(), paths(&["sub/dirty.txt"])),
		(head.clone(), paths(&["nested"])),
		(head.clone(), paths(&[])),
		(
			head.clone(),
			paths(&vec!["tracked.txt"; crate::seed::MAX_SELECTED_PATHS + 1]),
		),
	] {
		let refusal = create(core, project_id, base, seed).await.unwrap_err();
		refusals.push((refusal.category, refusal.code));
	}
	let journal = events(core).await;
	let left_behind = std::fs::read_dir(dir.path().join("workspaces"))
		.map(|entries| entries.count())
		.unwrap_or(0);

	assert_eq!(
		(refusals, journal.len(), left_behind),
		(
			vec![
				(
					ErrorCategory::Conflict,
					"workspace.seed_base_mismatch".into()
				),
				(
					ErrorCategory::NotFound,
					"workspace.seed_path_not_found".into()
				),
				(
					ErrorCategory::InvalidInput,
					"workspace.seed_unsupported".into()
				),
				(
					ErrorCategory::InvalidInput,
					"workspace.seed_unsupported".into()
				),
				(
					ErrorCategory::InvalidInput,
					"workspace.seed_no_paths".into()
				),
				(
					ErrorCategory::InvalidInput,
					"workspace.seed_too_many_paths".into()
				),
			],
			1,
			0,
		)
	);
}
