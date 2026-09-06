use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use crate::test_support::{
	actor, conversation_snapshot as snapshot, git, register_repository,
	request, start_core,
};
use crate::{
	BaseSelection, ChangeKind, ClientId, Command, CommandOutcome, ConflictKind,
	Core, CoreError, ErrorCategory, EventSequence, PromotedChange,
	PromotionBinding, PromotionConflict, PromotionDestination,
	PromotionPreview, Query, QueryResult, RetentionPolicy, SeedSelection,
	WorkingTreeRequest, Workspace, WorkspaceId,
};

/// A Project whose Local checkout and one Workspace have both moved on
/// from the base: the checkout keeps an unstaged edit at the end of
/// `f.txt`, a staged new `o.txt`, and the untracked `notes.txt` the
/// Workspace was seeded with; the Workspace edits the top of `f.txt`,
/// adds `new.txt`, and deletes `k.txt`.
struct Diverged {
	core: Core,
	repository: PathBuf,
	base: String,
	workspace: Workspace,
}

async fn diverged(dir: &Path) -> Diverged {
	let core = start_core(&dir.join("plane.sqlite3")).await;
	let project_id = register_repository(&core, &dir.join("repo")).await;
	let repository = dir.join("repo").canonicalize().unwrap();
	std::fs::write(repository.join("f.txt"), "a\nb\nc\n").unwrap();
	std::fs::write(repository.join("k.txt"), "keep\n").unwrap();
	git(&repository, &["add", "-A"]);
	git(&repository, &["commit", "-q", "-m", "Base"]);
	let base = git(&repository, &["rev-parse", "HEAD"]).trim().to_owned();
	std::fs::write(repository.join("notes.txt"), "draft\n").unwrap();

	let outcome = core
		.execute(
			&actor(),
			request(Command::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRequest::Workspace {
					project_id,
					base: BaseSelection::Head,
					seed: SeedSelection::AllEligible,
				},
			}),
		)
		.await
		.unwrap();
	let CommandOutcome::ConversationCreated(conversation) = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	let workspace = snapshot(&core, conversation.conversation_id)
		.await
		.workspace
		.unwrap();
	std::fs::write(workspace.root.join("f.txt"), "A\nb\nc\n").unwrap();
	std::fs::write(workspace.root.join("new.txt"), "new\n").unwrap();
	std::fs::remove_file(workspace.root.join("k.txt")).unwrap();

	std::fs::write(repository.join("f.txt"), "a\nb\nC\n").unwrap();
	std::fs::write(repository.join("o.txt"), "other\n").unwrap();
	git(&repository, &["add", "o.txt"]);

	Diverged {
		core,
		repository,
		base,
		workspace,
	}
}

async fn preview(
	core: &Core,
	workspace_id: WorkspaceId,
	destination: PromotionDestination,
) -> Result<PromotionPreview, CoreError> {
	let result = core
		.query(
			&actor(),
			Query::PreviewPromotion {
				workspace_id,
				destination,
			},
		)
		.await?;
	let QueryResult::PromotionPreview(preview) = result else {
		panic!("unexpected result {result:?}");
	};
	Ok(preview)
}

/// The paths `git status` reports in `root`, staged and unstaged alike.
fn status(root: &Path) -> String {
	git(root, &["status", "--porcelain", "--untracked-files=all"])
}

/// What `tree` holds at `path`, or nothing.
fn content(root: &Path, tree: &str, path: &str) -> Option<String> {
	let output = std::process::Command::new("git")
		.arg("-C")
		.arg(root)
		.args(["show", &format!("{tree}:{path}")])
		.output()
		.unwrap();
	output
		.status
		.success()
		.then(|| String::from_utf8(output.stdout).unwrap())
}

fn change(path: &str, kind: ChangeKind) -> PromotedChange {
	PromotedChange {
		path: path.into(),
		kind,
	}
}

/// A preview merges the Workspace's changes over the Local checkout's own
/// against the Workspace base, lists exactly what would change, binds
/// what it compared and whom it was shown to, and leaves both working
/// trees exactly as they were. Changes the Workspace was seeded with are
/// on both sides and change nothing (ADR-0025).
#[tokio::test]
async fn a_preview_merges_the_workspace_over_the_checkout_without_touching_it()
{
	let dir = tempfile::tempdir().unwrap();
	let Diverged {
		core,
		repository,
		base,
		workspace,
	} = diverged(dir.path()).await;
	let checkout_before = status(&repository);
	let workspace_before = status(&workspace.root);

	let previewed = preview(
		&core,
		workspace.workspace_id,
		PromotionDestination::LocalCheckout,
	)
	.await
	.unwrap();

	let binding = previewed.binding.clone();
	assert_eq!(
		(
			&previewed,
			content(&repository, &binding.result_tree, "f.txt"),
			content(&repository, &binding.result_tree, "new.txt"),
			content(&repository, &binding.result_tree, "k.txt"),
			content(&repository, &binding.result_tree, "o.txt"),
			content(&repository, &binding.result_tree, "notes.txt"),
			content(&repository, &binding.workspace_tree, "notes.txt"),
			content(&repository, &binding.destination_tree, "f.txt"),
			status(&repository),
			status(&workspace.root),
		),
		(
			&PromotionPreview {
				cursor: previewed.cursor,
				binding: PromotionBinding {
					workspace_id: workspace.workspace_id,
					destination: PromotionDestination::LocalCheckout,
					base_commit: base.clone(),
					workspace_tree: binding.workspace_tree.clone(),
					destination_commit: base.clone(),
					destination_tree: binding.destination_tree.clone(),
					result_tree: binding.result_tree.clone(),
					actor: ClientId(uuid::Uuid::nil()),
				},
				destination_dirty: true,
				changed_paths: 3,
				changes: vec![
					change("f.txt", ChangeKind::Modified),
					change("k.txt", ChangeKind::Deleted),
					change("new.txt", ChangeKind::Added),
				],
				conflicts: vec![],
			},
			Some("A\nb\nC\n".into()),
			Some("new\n".into()),
			None,
			Some("other\n".into()),
			Some("draft\n".into()),
			Some("draft\n".into()),
			Some("a\nb\nC\n".into()),
			checkout_before,
			workspace_before,
		)
	);
}

/// A path both sides changed in ways Git cannot combine, a path both
/// sides added with different content, and a path the Workspace adds
/// where the checkout holds an ignored file are named as conflicts rather
/// than settled (ADR-0025).
#[tokio::test]
async fn a_preview_names_what_it_cannot_settle() {
	let dir = tempfile::tempdir().unwrap();
	let Diverged {
		core,
		repository,
		workspace,
		..
	} = diverged(dir.path()).await;
	std::fs::write(repository.join("f.txt"), "X\nb\nC\n").unwrap();
	std::fs::write(repository.join("new.txt"), "mine\n").unwrap();
	std::fs::write(repository.join(".gitignore"), "local.txt\n").unwrap();
	std::fs::write(repository.join("local.txt"), "mine\n").unwrap();
	std::fs::write(workspace.root.join("local.txt"), "theirs\n").unwrap();

	let previewed = preview(
		&core,
		workspace.workspace_id,
		PromotionDestination::LocalCheckout,
	)
	.await
	.unwrap();

	assert_eq!(
		(previewed.conflicts, previewed.changed_paths),
		(
			vec![
				PromotionConflict {
					path: "f.txt".into(),
					kind: ConflictKind::Diverged,
				},
				PromotionConflict {
					path: "new.txt".into(),
					kind: ConflictKind::Diverged,
				},
				PromotionConflict {
					path: "local.txt".into(),
					kind: ConflictKind::Untracked,
				},
			],
			4,
		)
	);
}

/// A branch no working tree has checked out is previewed against its
/// tip; the checked-out branch, a missing one, a malformed name, and an
/// unknown Workspace are refused with stable errors (ADR-0025).
#[tokio::test]
async fn a_preview_targets_a_branch_no_working_tree_has_checked_out() {
	let dir = tempfile::tempdir().unwrap();
	let Diverged {
		core,
		repository,
		base,
		workspace,
	} = diverged(dir.path()).await;
	git(&repository, &["branch", "release", &base]);
	let mut refusals = Vec::new();
	for (workspace_id, destination) in [
		(
			workspace.workspace_id,
			PromotionDestination::Branch("main".into()),
		),
		(
			workspace.workspace_id,
			PromotionDestination::Branch("nowhere".into()),
		),
		(
			workspace.workspace_id,
			PromotionDestination::Branch("-bad".into()),
		),
		(
			WorkspaceId(uuid::Uuid::nil()),
			PromotionDestination::LocalCheckout,
		),
	] {
		let refused =
			preview(&core, workspace_id, destination).await.unwrap_err();
		refusals.push((refused.category, refused.code));
	}

	let previewed = preview(
		&core,
		workspace.workspace_id,
		PromotionDestination::Branch("release".into()),
	)
	.await
	.unwrap();

	let binding = previewed.binding.clone();
	assert_eq!(
		(
			&previewed,
			content(&repository, &binding.result_tree, "f.txt"),
			content(&repository, &binding.result_tree, "notes.txt"),
			refusals,
		),
		(
			&PromotionPreview {
				cursor: EventSequence(previewed.cursor.0),
				binding: PromotionBinding {
					workspace_id: workspace.workspace_id,
					destination: PromotionDestination::Branch("release".into()),
					base_commit: base.clone(),
					workspace_tree: binding.workspace_tree.clone(),
					destination_commit: base.clone(),
					destination_tree: git(
						&repository,
						&["rev-parse", "HEAD^{tree}"]
					)
					.trim()
					.to_owned(),
					result_tree: binding.result_tree.clone(),
					actor: ClientId(uuid::Uuid::nil()),
				},
				destination_dirty: false,
				changed_paths: 4,
				changes: vec![
					change("f.txt", ChangeKind::Modified),
					change("k.txt", ChangeKind::Deleted),
					change("new.txt", ChangeKind::Added),
					change("notes.txt", ChangeKind::Added),
				],
				conflicts: vec![],
			},
			Some("A\nb\nc\n".into()),
			Some("draft\n".into()),
			vec![
				(
					ErrorCategory::Conflict,
					"workspace.promotion_branch_checked_out".into()
				),
				(
					ErrorCategory::NotFound,
					"workspace.promotion_branch_not_found".into()
				),
				(
					ErrorCategory::InvalidInput,
					"workspace.promotion_destination_invalid".into()
				),
				(ErrorCategory::NotFound, "workspace.not_found".into()),
			],
		)
	);
}

/// A file changed to content of the same size in the same second as the
/// checkout's index was last written is captured as it is: the scratch
/// copy of the index keeps the index file's own time, so Git still
/// distrusts the entry's recorded stat data and reads the file
/// (ADR-0025). The second is staged by hand: the index and the file are
/// given one past time, and the repository is told not to trust change
/// times, which a write moves and a test cannot set.
#[tokio::test]
async fn a_change_made_as_the_index_was_written_is_still_seen() {
	let dir = tempfile::tempdir().unwrap();
	let Diverged {
		core,
		repository,
		workspace,
		..
	} = diverged(dir.path()).await;
	git(&repository, &["config", "core.trustctime", "false"]);
	let file = repository.join("f.txt");
	let index = repository.join(".git").join("index");
	let past =
		std::time::SystemTime::now() - std::time::Duration::from_secs(600);
	let stamp = |path: &Path| {
		std::fs::File::options()
			.write(true)
			.open(path)
			.unwrap()
			.set_times(std::fs::FileTimes::new().set_modified(past))
			.unwrap();
	};
	stamp(&file);
	git(&repository, &["add", "f.txt"]);
	std::fs::write(&file, "a\nb\nX\n").unwrap();
	stamp(&file);
	stamp(&index);

	let previewed = preview(
		&core,
		workspace.workspace_id,
		PromotionDestination::LocalCheckout,
	)
	.await
	.unwrap();

	assert_eq!(
		content(&repository, &previewed.binding.destination_tree, "f.txt"),
		Some("a\nb\nX\n".into())
	);
}
