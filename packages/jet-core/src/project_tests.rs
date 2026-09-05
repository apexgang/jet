use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::test_support::{
	FixedProbe, actor, command_id, equipped, git, init_repository,
	request_with_id, start_core, start_core_with, stripped,
};
use crate::{
	AuditOutcome, AuditRisk, AuditSequence, ClientId, Command, CommandId,
	CommandOutcome, Core, CoreError, ErrorCategory, EventKind, EventSequence,
	PathGrant, Project, Query, QueryResult,
};

async fn start(dir: &tempfile::TempDir) -> Core {
	start_core(&dir.path().join("plane.sqlite3")).await
}

async fn register(core: &Core, path: &Path) -> Result<Project, CoreError> {
	register_as(core, command_id(), path).await
}

async fn register_as(
	core: &Core,
	command_id: CommandId,
	path: &Path,
) -> Result<Project, CoreError> {
	let outcome = core
		.execute(
			&actor(),
			request_with_id(
				command_id,
				Command::RegisterProject {
					grant: PathGrant(path.to_path_buf()),
				},
			),
		)
		.await?;
	let CommandOutcome::ProjectRegistered(project) = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	Ok(project)
}

async fn refusal(core: &Core, path: &Path) -> (ErrorCategory, String) {
	let error = register(core, path).await.unwrap_err();
	(error.category, error.code)
}

async fn projects(core: &Core) -> Vec<Project> {
	let result = core.query(&actor(), Query::Projects).await.unwrap();
	let QueryResult::Projects(list) = result else {
		panic!("unexpected result {result:?}");
	};
	list.projects
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

async fn audit(core: &Core) -> Vec<(String, String, AuditRisk, AuditOutcome)> {
	let result = core
		.query(
			&actor(),
			Query::SecurityAudit {
				after: AuditSequence(0),
			},
		)
		.await
		.unwrap();
	let QueryResult::SecurityAudit(page) = result else {
		panic!("unexpected result {result:?}");
	};
	page.entries
		.into_iter()
		.map(|entry| {
			(entry.decision, entry.target.kind, entry.risk, entry.outcome)
		})
		.collect()
}

/// A Path grant is the one way a canonical absolute path enters the core
/// (ADR-0101). Registration resolves the granted path, records the Actor
/// that granted it, journals it, and records the widened access in the
/// Security audit (ADR-0105).
#[tokio::test]
async fn a_granted_repository_registers_under_its_canonical_root() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let repository = init_repository(&dir.path().join("repo"));
	let alias = dir.path().join("alias");
	symlink(&repository, &alias).unwrap();

	let registered = register(&core, &alias).await.unwrap();
	let listed = projects(&core).await;
	let journal = events(&core).await;
	let audited = audit(&core).await;

	assert_eq!(
		(&registered, listed, journal, audited),
		(
			&Project {
				project_id: registered.project_id,
				root: repository.clone(),
				registered_by: ClientId(Uuid::nil()),
				registered_at: registered.registered_at,
			},
			vec![registered.clone()],
			vec![EventKind::ProjectRegistered {
				project_id: registered.project_id,
				root: repository,
			}],
			vec![(
				"project.registered".into(),
				"project".into(),
				AuditRisk::Elevated,
				AuditOutcome::Succeeded
			)]
		)
	);
}

/// ADR-0103 accepts ordinary non-bare repositories and linked worktrees and
/// nothing else; ADR-0101 accepts a grant only for the canonical directory
/// it names. Each refusal has its own stable code and none of them writes
/// anything.
#[tokio::test]
async fn a_root_that_is_not_an_ordinary_working_tree_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let repository = init_repository(&dir.path().join("repo"));
	std::fs::create_dir_all(repository.join("src")).unwrap();
	let plain = dir.path().join("plain");
	std::fs::create_dir_all(&plain).unwrap();
	let bare = dir.path().join("bare.git");
	git(dir.path(), &["init", "-q", "--bare", "bare.git"]);
	let orphan = dir.path().join("orphan");
	let doomed = init_repository(&dir.path().join("doomed"));
	git(
		&doomed,
		&["worktree", "add", "-q", orphan.to_str().unwrap()],
	);
	std::fs::remove_dir_all(&doomed).unwrap();

	let refused = [
		refusal(&core, &plain).await,
		refusal(&core, &bare).await,
		refusal(&core, &repository.join(".git")).await,
		refusal(&core, &repository.join("src")).await,
		refusal(&core, &orphan).await,
		refusal(&core, &repository.join("README.md")).await,
		refusal(&core, &dir.path().join("missing")).await,
		refusal(&core, Path::new("repo")).await,
		refusal(&core, Path::new("/tmp/jet\0repo")).await,
	];

	assert_eq!(
		(refused, projects(&core).await),
		(
			[
				(
					ErrorCategory::InvalidInput,
					"project.not_a_repository".into()
				),
				(
					ErrorCategory::InvalidInput,
					"project.bare_repository".into()
				),
				(ErrorCategory::InvalidInput, "project.inside_git_dir".into()),
				(
					ErrorCategory::InvalidInput,
					"project.root_not_toplevel".into()
				),
				(
					ErrorCategory::InvalidInput,
					"project.repository_broken".into()
				),
				(
					ErrorCategory::InvalidInput,
					"path_grant.not_directory".into()
				),
				(ErrorCategory::NotFound, "path_grant.unreachable".into()),
				(
					ErrorCategory::InvalidInput,
					"path_grant.not_absolute".into()
				),
				(ErrorCategory::InvalidInput, "path_grant.nul".into()),
			],
			vec![]
		)
	);
}

/// A refusal describes the filesystem as it was, not the Command, so it is
/// not a durable outcome: once the directory is a repository, the same
/// Command identity registers it (ADR-0093). The refusal is still a Path
/// grant that was turned away, which the Security audit keeps (ADR-0105).
#[tokio::test]
async fn a_refused_grant_leaves_no_receipt_behind() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let path = dir.path().join("repo");
	std::fs::create_dir_all(&path).unwrap();
	let command_id = command_id();

	let refused = register_as(&core, command_id, &path).await.unwrap_err();
	let repository = init_repository(&path);
	let registered = register_as(&core, command_id, &path).await.unwrap();

	assert_eq!(
		(refused.code.as_str(), registered.root, audit(&core).await),
		(
			"project.not_a_repository",
			repository,
			vec![
				(
					"project.registered".into(),
					"plane".into(),
					AuditRisk::Elevated,
					AuditOutcome::Denied
				),
				(
					"project.registered".into(),
					"project".into(),
					AuditRisk::Elevated,
					AuditOutcome::Succeeded
				),
			]
		)
	);
}

/// One directory is one Project however it is spelled, and a retried
/// Command is answered with what it decided the first time (ADR-0093).
#[tokio::test]
async fn a_root_is_registered_once() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let repository = init_repository(&dir.path().join("repo"));
	let alias = dir.path().join("alias");
	symlink(&repository, &alias).unwrap();
	let command_id = command_id();

	let registered = register_as(&core, command_id, &repository).await.unwrap();
	let again = register(&core, &alias).await.unwrap_err();
	let retried = register_as(&core, command_id, &repository).await.unwrap();

	assert_eq!(
		(
			again.category,
			again.code.as_str(),
			retried,
			projects(&core).await
		),
		(
			ErrorCategory::Conflict,
			"project.already_registered",
			registered.clone(),
			vec![registered]
		)
	);
}

/// Registration is carried out with the Git the core invokes, so a Plane
/// without it refuses before anything commits (ADR-0056, ADR-0086).
#[tokio::test]
async fn registration_needs_git_on_the_plane() {
	let dir = tempfile::tempdir().unwrap();
	let probe = FixedProbe::new(stripped());
	let core = start_core_with(
		&dir.path().join("plane.sqlite3"),
		Arc::new(crate::clock::SystemClock),
		Arc::clone(&probe),
	)
	.await;
	let repository = init_repository(&dir.path().join("repo"));

	let refused = register(&core, &repository).await.unwrap_err();
	probe.answer_with(equipped());
	let registered = register(&core, &repository).await;

	assert_eq!(
		(
			refused.category,
			refused.code.as_str(),
			refused.message.as_str(),
			registered.is_ok(),
		),
		(
			ErrorCategory::Unavailable,
			"capability.unavailable",
			"this Plane cannot use the git command-line tool right now",
			true,
		)
	);
}

/// ADR-0103 accepts a linked worktree as a Project of its own, and a
/// working tree is a working tree whether its repository is bare, a parent
/// project, or a submodule checkout. Each registers under its own root.
#[tokio::test]
async fn every_working_tree_registers_as_its_own_project() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let repository = init_repository(&dir.path().join("repo"));
	let linked = dir.path().join("linked");
	git(
		&repository,
		&["worktree", "add", "-q", linked.to_str().unwrap()],
	);
	git(dir.path(), &["init", "-q", "--bare", "bare.git"]);
	let of_bare = dir.path().join("of-bare");
	git(
		&dir.path().join("bare.git"),
		&[
			"worktree",
			"add",
			"-q",
			"--orphan",
			of_bare.to_str().unwrap(),
		],
	);
	let child = init_repository(&dir.path().join("child"));
	git(
		&repository,
		&[
			"submodule",
			"add",
			"-q",
			child.to_str().unwrap(),
			"vendor/child",
		],
	);
	let nested = init_repository(&repository.join("vendor/nested"));

	let mut roots: Vec<PathBuf> = Vec::new();
	for path in [
		linked.as_path(),
		of_bare.as_path(),
		&repository.join("vendor/child"),
		&nested,
	] {
		roots.push(register(&core, path).await.unwrap().root);
	}

	assert_eq!(
		roots,
		vec![
			linked.canonicalize().unwrap(),
			of_bare.canonicalize().unwrap(),
			repository.join("vendor/child"),
			nested,
		]
	);
}
