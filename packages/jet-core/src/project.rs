//! Registered Projects and the Path grants that register them (ADR-0025,
//! ADR-0101, ADR-0103).
//!
//! A Project is a registered Git working tree. It enters the core through
//! one explicit Path grant from an interactive user: the granted path is
//! resolved to the canonical directory it names, `git` is asked whether
//! that directory is an ordinary working tree, and only then is the root
//! recorded with the Actor that granted it. Ordinary file Commands never
//! carry a path like this; they name a Project and a relative path.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jet_store::{NewProject, ProjectRecord, WriteTransaction};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::{self, AuditDecision, AuditSubject, Decision};
use crate::capability::{
	CapabilityObservation, ExternalTool, ToolAvailability,
};
use crate::command::CommandOutcome;
use crate::error::CoreError;
use crate::event::{EventKind, EventSequence, EventSubject};
use crate::filesystem::{blocking, canonicalize};
use crate::query::QueryResult;
use crate::repository::{self, Inspection, Verdict};
use crate::{Actor, ClientId, Core, ProjectId, system_time};

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
			if error.kind() == std::io::ErrorKind::NotFound {
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
#[path = "project_tests.rs"]
mod tests;
