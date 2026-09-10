//! Registered Projects and the Path grants that register them (ADR-0025,
//! ADR-0101, ADR-0103).
//!
//! A Project is a registered Git working tree. It enters the core through
//! one explicit Path grant from an interactive user: the granted path is
//! resolved to the canonical directory it names, `git` is asked whether
//! that directory is an ordinary working tree, and only then is the root
//! recorded with the Actor that granted it. Ordinary file Commands never
//! carry a path like this; they name a Project and a relative path.

pub(crate) mod entry;
pub(crate) mod repository;

use crate::{
	Actor, ClientId, Core, ProjectId,
	audit::{self, AuditDecision, AuditSubject, Decision},
	capability::{CapabilityObservation, ExternalTool, ToolAvailability},
	command::CommandOutcome,
	error::CoreError,
	event::{EventKind, EventSequence, EventSubject},
	filesystem::{blocking, canonicalize},
	project::repository::{Inspection, Verdict},
	query::QueryResult,
	system_time,
};
use jet_store::{NewProject, ProjectRecord, WriteTransaction};
use serde::{Deserialize, Serialize};
use std::{
	path::{Path, PathBuf},
	time::SystemTime,
};
use uuid::Uuid;

/// An interactive user's explicit authorization for Jet to register the
/// directory at one absolute path (see `Path grant`). It is the only form
/// in which an absolute path reaches the core, and it is resolved and
/// checked before anything is recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PathGrant(pub PathBuf);

/// One registered Project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
	/// Durable identity.
	pub project_id: ProjectId,
	/// The canonical absolute root of its working tree.
	pub root: PathBuf,
	/// The Client identity of the interactive user whose Path grant
	/// registered it.
	pub registered_by: ClientId,
	/// When it was registered.
	pub registered_at: SystemTime,
}

/// Every registered Project, fenced by the journal position the snapshot
/// was read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectList {
	/// Newest Event sequence visible when the snapshot was read.
	pub cursor: EventSequence,
	/// The Projects in the order they were registered.
	pub projects: Vec<Project>,
}

/// What a Path grant would register, shown before anything is recorded
/// (ADR-0101).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPreview {
	/// The canonical directory the grant resolves to.
	pub root: PathBuf,
	/// Whether that directory can be a Project, and what it is if so.
	pub registrability: Registrability,
}

/// Whether a granted directory can be a Project (ADR-0103). What keeps it
/// from registering is answered as data, so a GUI acts on it without
/// parsing a message (ADR-0068).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registrability {
	/// An ordinary working tree, described.
	Registrable(Repository),
	/// Git finds no repository at the directory or above it.
	NotARepository,
	/// The directory carries a `.git` entry that Git cannot open, such as
	/// a linked worktree whose repository is gone.
	BrokenRepository,
	/// A bare repository, which has no working tree for Runs, diffs, and
	/// Change checkpoints to use.
	BareRepository,
	/// The directory lies inside a repository's own `.git` directory.
	InsideGitDir,
	/// The directory lies inside a working tree without being its top. The
	/// grant is for the directory named, so the user may grant the top
	/// instead.
	InsideWorkingTree {
		/// The top of the working tree the directory lies in.
		toplevel: PathBuf,
	},
}

/// A registrable working tree as `git` describes it (ADR-0103).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
	/// Whether the working tree is the repository's own or a linked one.
	pub worktree: Worktree,
	/// Whether a sparse checkout narrows it. Jet reports the configuration
	/// and never changes it.
	pub checkout: Checkout,
	/// The submodules its index holds, each as its Git link alone. Nothing
	/// beneath a link is managed or listed.
	pub submodules: Vec<GitLink>,
	/// Whether the Plane has Git LFS, from the Capability observation the
	/// preview asked for. Jet never bundles it.
	pub lfs: ToolAvailability,
}

/// Which working tree of its repository a Project is. This is Git's
/// worktree, the thing `git worktree` manages; Jet's Workspace is built on
/// one but is a different concept (ADR-0025).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Worktree {
	/// The repository's own working tree.
	Main,
	/// A linked worktree, sharing the repository at `common_dir`.
	Linked {
		/// The `.git` directory (or bare repository) the worktree shares.
		common_dir: PathBuf,
	},
}

/// Whether a working tree is checked out in full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checkout {
	/// Every tracked path is present.
	Full,
	/// Sparse checkout narrows which paths are present.
	Sparse,
}

/// One submodule as the index records it: a path that holds a commit of
/// another repository (ADR-0103).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLink {
	/// The path inside the working tree, as Git spells it.
	pub path: String,
	/// The commit the link points at, as Git spells it.
	pub commit: String,
}

/// A root a Path grant resolved to and `git` accepted, ready to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Registrable {
	root: PathBuf,
}

impl PathGrant {
	/// Resolves the grant to the canonical directory it names.
	///
	/// # Errors
	///
	/// Returns `path_grant.not_absolute`, `path_grant.nul`,
	/// `path_grant.not_directory`, or `path_grant.not_unicode` when the
	/// grant is not one Jet can record, and a `not_found` or `unavailable`
	/// `path_grant.unreachable` when the path cannot be resolved.
	async fn canonical_root(&self) -> Result<PathBuf, CoreError> {
		let path = self.0.clone();
		if path.as_os_str().as_encoded_bytes().contains(&0) {
			return Err(CoreError::invalid_input(
				"path_grant.nul",
				"a Path grant holds no NUL character",
			));
		}
		if !path.is_absolute() {
			return Err(CoreError::invalid_input(
				"path_grant.not_absolute",
				"a Path grant names an absolute path",
			));
		}
		let root = canonicalize(path).await.map_err(|error| {
			// A path that leads through a file does not exist any more than
			// one that leads nowhere, and neither is worth retrying.
			if matches!(
				error.kind(),
				std::io::ErrorKind::NotFound
					| std::io::ErrorKind::NotADirectory
			) {
				CoreError::not_found(
					"path_grant.unreachable",
					"the granted path does not exist on this Plane",
				)
			} else {
				CoreError::unavailable(
					"path_grant.unreachable",
					"the granted path cannot be reached on this Plane",
					error.to_string(),
				)
			}
		})?;
		let is_dir = {
			let root = root.clone();
			blocking(move || root.is_dir()).await?
		};
		if !is_dir {
			return Err(CoreError::invalid_input(
				"path_grant.not_directory",
				"a Path grant names a directory",
			));
		}
		if root.to_str().is_none() {
			return Err(CoreError::invalid_input(
				"path_grant.not_unicode",
				"a Project root is spelled in Unicode",
			));
		}
		Ok(root)
	}
}

/// Resolves and inspects a grant before the registering transaction opens,
/// so no external process runs while the store is locked and a refusal
/// leaves no receipt behind: it describes the filesystem as it was, not
/// the Command (ADR-0093).
///
/// # Errors
///
/// Returns the grant's own refusals, `project.not_a_repository`,
/// `project.repository_broken`, `project.bare_repository`,
/// `project.inside_git_dir`, or `project.root_not_toplevel` when the
/// directory is not an ordinary working tree (ADR-0103), and what the
/// inspection itself reports when it cannot answer.
pub(crate) async fn prepare_registration(
	actor: &Actor,
	grant: &PathGrant,
) -> Result<Registrable, CoreError> {
	require_interactive(actor);
	let root = grant.canonical_root().await?;
	match repository::verdict(&root).await? {
		Verdict::Registrable => Ok(Registrable { root }),
		Verdict::NotARepository => Err(CoreError::invalid_input(
			"project.not_a_repository",
			"the granted directory is not inside a Git repository",
		)),
		Verdict::BrokenRepository => Err(CoreError::invalid_input(
			"project.repository_broken",
			"the granted directory has a .git entry that Git cannot open",
		)),
		Verdict::BareRepository => Err(CoreError::invalid_input(
			"project.bare_repository",
			"a bare repository has no working tree for Runs to use",
		)),
		Verdict::InsideGitDir => Err(CoreError::invalid_input(
			"project.inside_git_dir",
			"the granted directory lies inside a repository's .git directory",
		)),
		Verdict::InsideWorkingTree { .. } => Err(CoreError::invalid_input(
			"project.root_not_toplevel",
			"the granted directory lies inside a working tree; grant the top \
			 of that working tree instead",
		)),
	}
}

/// ADR-0101: a Path grant is an interactive user's to make, and so is the
/// look before it. Both Actors this core knows are interactive; a Harness,
/// Craft, Scheduled-task, or automatic Actor added later is refused here.
fn require_interactive(actor: &Actor) {
	match actor {
		Actor::InteractiveClient { .. } | Actor::RemoteClient { .. } => {}
	}
}

/// Shows what `grant` would register: the directory it resolves to and
/// what `git` says about it, without recording anything.
///
/// Git LFS is reported from the Capability observation `observation`
/// selects, the way Account bindings report their credential store
/// (ADR-0086).
///
/// # Errors
///
/// Returns the grant's own refusals, or what the inspection reports when
/// it cannot answer.
pub(crate) async fn preview(
	core: &Core,
	actor: &Actor,
	grant: &PathGrant,
	observation: CapabilityObservation,
) -> Result<QueryResult, CoreError> {
	require_interactive(actor);
	let root = grant.canonical_root().await?;
	let registrability = match repository::verdict(&root).await? {
		Verdict::Registrable => {
			let Inspection {
				worktree,
				checkout,
				submodules,
			} = repository::inspect(&root).await?;
			let capabilities = match observation {
				CapabilityObservation::LastObserved => {
					core.capabilities().await
				}
				CapabilityObservation::Fresh => {
					core.observe_capabilities().await
				}
			};
			Registrability::Registrable(Repository {
				worktree,
				checkout,
				submodules,
				lfs: capabilities.availability(ExternalTool::GitLfs),
			})
		}
		Verdict::NotARepository => Registrability::NotARepository,
		Verdict::BrokenRepository => Registrability::BrokenRepository,
		Verdict::BareRepository => Registrability::BareRepository,
		Verdict::InsideGitDir => Registrability::InsideGitDir,
		Verdict::InsideWorkingTree { toplevel } => {
			Registrability::InsideWorkingTree { toplevel }
		}
	};
	Ok(QueryResult::ProjectPreview(ProjectPreview {
		root,
		registrability,
	}))
}

/// Records a prepared root as a Project, journals it, and records the
/// widened access in the Security audit, all in the transaction that
/// commits it (ADR-0105).
///
/// One directory is one Project. A working tree inside another Project's
/// root, such as a submodule checkout or a nested repository, is a Project
/// of its own all the same: the parent treats it as a Git link or an
/// opaque directory (ADR-0103), and ADR-0025's one Run in a Local checkout
/// is a rule of each Project.
///
/// # Errors
///
/// Returns a `conflict` `project.already_registered` when the root is
/// already a Project, or a store category when the row cannot be written.
pub(crate) async fn register(
	tx: &mut WriteTransaction,
	actor: &Actor,
	registrable: Registrable,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let Registrable { root } = registrable;
	let root_text = root_text(&root)?;
	if tx.project_by_root(&root_text).await?.is_some() {
		return Err(CoreError::conflict(
			"project.already_registered",
			"that directory is already a registered Project",
		));
	}
	let project: Project = tx
		.insert_project(NewProject {
			project_id: Uuid::now_v7(),
			root: root_text,
			registered_by: actor.record(),
			registered_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	let event = EventKind::ProjectRegistered {
		project_id: project.project_id,
		root: project.root.clone(),
	};
	tx.append_event(event.to_record(
		actor,
		EventSubject::Plane,
		now_unix_ms,
	)?)
	.await?;
	// ASVS 16.2.1: a Path grant widens what Jet may read and change on
	// this Plane, which the Security audit exists to record (ADR-0105).
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::ProjectRegistered,
			AuditSubject::Project(project.project_id),
		),
		now_unix_ms,
	)
	.await?;
	Ok(CommandOutcome::ProjectRegistered(project))
}

/// The root as the store keeps it. A root that reaches here is Unicode,
/// because the grant was refused otherwise.
fn root_text(root: &Path) -> Result<String, CoreError> {
	root.to_str().map(str::to_owned).ok_or_else(|| {
		CoreError::internal(
			"project.root_not_unicode",
			"a registered root was not Unicode",
		)
	})
}

impl From<ProjectRecord> for Project {
	fn from(record: ProjectRecord) -> Self {
		Self {
			project_id: ProjectId(record.project_id),
			root: PathBuf::from(record.root),
			registered_by: Actor::from_record(record.registered_by).client_id(),
			registered_at: system_time(record.registered_at_unix_ms),
		}
	}
}

#[cfg(test)]
mod tests {
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
		AuditOutcome, AuditRisk, AuditSequence, CapabilityObservation,
		Checkout, ClientId, Command, CommandId, CommandOutcome, Core,
		CoreError, EntryKind, ErrorCategory, EventKind, EventSequence, GitLink,
		PathGrant, Project, ProjectEntry, ProjectId, ProjectPreview, Query,
		QueryResult, Registrability, RelativePath, Repository,
		ToolAvailability, Worktree,
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
			panic!("expected CommandOutcome::ProjectRegistered");
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
			panic!("expected QueryResult::Projects");
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
			panic!("expected QueryResult::Events");
		};
		page.events.into_iter().map(|event| event.kind).collect()
	}

	async fn audit(
		core: &Core,
	) -> Vec<(String, String, AuditRisk, AuditOutcome)> {
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
			panic!("expected QueryResult::SecurityAudit");
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
			refusal(&core, &repository.join("README.md/repo")).await,
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
					(
						ErrorCategory::InvalidInput,
						"project.inside_git_dir".into()
					),
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

		let registered =
			register_as(&core, command_id, &repository).await.unwrap();
		let again = register(&core, &alias).await.unwrap_err();
		let retried =
			register_as(&core, command_id, &repository).await.unwrap();

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

	async fn preview(
		core: &Core,
		path: &Path,
		observation: CapabilityObservation,
	) -> Result<ProjectPreview, CoreError> {
		let result = core
			.query(
				&actor(),
				Query::PreviewProject {
					grant: PathGrant(path.to_path_buf()),
					observation,
				},
			)
			.await?;
		let QueryResult::ProjectPreview(preview) = result else {
			panic!("expected QueryResult::ProjectPreview");
		};
		Ok(preview)
	}

	async fn registrability(core: &Core, path: &Path) -> Registrability {
		preview(core, path, CapabilityObservation::LastObserved)
			.await
			.unwrap()
			.registrability
	}

	/// A preview is the look before the grant (ADR-0101): it resolves the
	/// path and describes the working tree without registering anything.
	#[tokio::test]
	async fn a_preview_describes_the_working_tree_without_registering_it() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let repository = init_repository(&dir.path().join("repo"));
		let alias = dir.path().join("alias");
		symlink(&repository, &alias).unwrap();

		let previewed =
			preview(&core, &alias, CapabilityObservation::LastObserved)
				.await
				.unwrap();

		assert_eq!(
			(previewed, projects(&core).await, events(&core).await),
			(
				ProjectPreview {
					root: repository,
					registrability: Registrability::Registrable(Repository {
						worktree: Worktree::Main,
						checkout: Checkout::Full,
						submodules: vec![],
						lfs: ToolAvailability::Present {
							version: "2.51.0".into()
						},
					}),
				},
				vec![],
				vec![]
			)
		);
	}

	/// What keeps a directory from registering is answered as data the GUI
	/// acts on, not as text it would have to parse (ADR-0068); the one path
	/// it carries is the top of the working tree the user may grant instead.
	#[tokio::test]
	async fn a_preview_names_what_keeps_a_directory_from_registering() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let repository = init_repository(&dir.path().join("repo"));
		std::fs::create_dir_all(repository.join("src")).unwrap();
		let plain = dir.path().join("plain");
		std::fs::create_dir_all(&plain).unwrap();
		git(dir.path(), &["init", "-q", "--bare", "bare.git"]);
		let orphan = dir.path().join("orphan");
		let doomed = init_repository(&dir.path().join("doomed"));
		git(
			&doomed,
			&["worktree", "add", "-q", orphan.to_str().unwrap()],
		);
		std::fs::remove_dir_all(&doomed).unwrap();

		assert_eq!(
			[
				registrability(&core, &plain).await,
				registrability(&core, &dir.path().join("bare.git")).await,
				registrability(&core, &repository.join(".git")).await,
				registrability(&core, &repository.join("src")).await,
				registrability(&core, &orphan).await,
			],
			[
				Registrability::NotARepository,
				Registrability::BareRepository,
				Registrability::InsideGitDir,
				Registrability::InsideWorkingTree {
					toplevel: repository.clone()
				},
				Registrability::BrokenRepository,
			]
		);
	}

	/// ADR-0103: a linked worktree is reported with the repository it shares,
	/// sparse-checkout configuration is reported and left alone, a submodule
	/// is one Git link with its commit and nothing beneath it, and a nested
	/// repository is an opaque directory unless the index holds a Git link
	/// for it. Git LFS is reported from the Plane's Capability observation.
	#[tokio::test]
	async fn a_preview_reports_repository_edges_without_recursing() {
		let dir = tempfile::tempdir().unwrap();
		let probe = FixedProbe::new(equipped());
		let core = start_core_with(
			&dir.path().join("plane.sqlite3"),
			Arc::new(crate::clock::SystemClock),
			Arc::clone(&probe),
		)
		.await;
		let repository = init_repository(&dir.path().join("repo"));
		let linked = dir.path().join("linked");
		git(
			&repository,
			&["worktree", "add", "-q", linked.to_str().unwrap()],
		);
		git(&linked, &["sparse-checkout", "set", "docs"]);
		let child = init_repository(&dir.path().join("child"));
		let child_commit =
			git(&child, &["rev-parse", "HEAD"]).trim().to_owned();
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
		init_repository(&repository.join("vendor/untracked"));
		init_repository(&repository.join("vendor/adopted"));
		git(&repository, &["add", "vendor/adopted"]);
		let adopted_commit =
			git(&repository.join("vendor/adopted"), &["rev-parse", "HEAD"])
				.trim()
				.to_owned();
		probe.answer_with(stripped());

		let main = registrability(&core, &repository).await;
		let linked = preview(&core, &linked, CapabilityObservation::Fresh)
			.await
			.unwrap()
			.registrability;

		assert_eq!(
			(main, linked),
			(
				Registrability::Registrable(Repository {
					worktree: Worktree::Main,
					checkout: Checkout::Full,
					submodules: vec![
						GitLink {
							path: "vendor/adopted".into(),
							commit: adopted_commit,
						},
						GitLink {
							path: "vendor/child".into(),
							commit: child_commit,
						},
					],
					lfs: ToolAvailability::Present {
						version: "2.51.0".into()
					},
				}),
				Registrability::Registrable(Repository {
					worktree: Worktree::Linked {
						common_dir: repository.join(".git"),
					},
					checkout: Checkout::Sparse,
					submodules: vec![],
					lfs: ToolAvailability::Missing,
				}),
			)
		);
	}

	async fn entry(
		core: &Core,
		project_id: ProjectId,
		path: &str,
	) -> Result<ProjectEntry, String> {
		let result = core
			.query(
				&actor(),
				Query::ProjectEntry {
					project_id,
					path: RelativePath::parse(path).unwrap(),
				},
			)
			.await
			.map_err(|error| error.code)?;
		let QueryResult::ProjectEntry(entry) = result else {
			panic!("unexpected result {result:?}");
		};
		Ok(entry)
	}

	/// The entry `path` names in `project_id` on a Plane whose journal holds
	/// the one Event that registered it.
	fn found(
		project_id: ProjectId,
		path: &str,
		kind: EntryKind,
	) -> ProjectEntry {
		ProjectEntry {
			cursor: EventSequence(1),
			project_id,
			path: RelativePath::parse(path).unwrap(),
			kind,
		}
	}

	/// An ordinary file operation names a Project and a relative path, never
	/// an absolute one (ADR-0101). The path resolves under the root the grant
	/// named; a link that leaves it is refused, and so is a root that has
	/// since moved or become a link to somewhere else.
	#[tokio::test]
	async fn a_file_is_addressed_through_its_project_and_a_relative_path() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let repository = init_repository(&dir.path().join("repo"));
		std::fs::create_dir_all(repository.join("docs")).unwrap();
		symlink(dir.path(), repository.join("escape")).unwrap();
		let project_id = register(&core, &repository).await.unwrap().project_id;

		let readme = entry(&core, project_id, "README.md").await;
		let docs = entry(&core, project_id, "docs").await;
		let missing = entry(&core, project_id, "docs/missing.md").await;
		let escaped = entry(&core, project_id, "escape/plane.sqlite3").await;
		let unknown =
			entry(&core, ProjectId(Uuid::now_v7()), "README.md").await;
		std::fs::rename(&repository, dir.path().join("moved")).unwrap();
		let gone = entry(&core, project_id, "README.md").await;
		symlink(dir.path().join("moved"), &repository).unwrap();
		let moved = entry(&core, project_id, "README.md").await;

		assert_eq!(
			[readme, docs, missing, escaped, unknown, gone, moved],
			[
				Ok(found(
					project_id,
					"README.md",
					EntryKind::File { bytes: 6 }
				)),
				Ok(found(project_id, "docs", EntryKind::Directory)),
				Ok(found(project_id, "docs/missing.md", EntryKind::Missing)),
				Err("path.escapes_root".into()),
				Err("project.not_found".into()),
				Err("path.root_unreachable".into()),
				Err("path.root_moved".into()),
			]
		);
	}
}
