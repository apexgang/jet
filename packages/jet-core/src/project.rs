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
use crate::command::CommandOutcome;
use crate::error::CoreError;
use crate::event::{EventKind, EventSequence, EventSubject};
use crate::repository::{self, Verdict, blocking, canonicalize};
use crate::{Actor, ClientId, ProjectId, system_time};

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
			if error.to_string().contains("No such file") {
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
	// ADR-0101: a Path grant is an interactive user's to make. Both Actors
	// this core knows are interactive; a Harness, Craft, Scheduled-task, or
	// automatic Actor added later is refused here.
	match actor {
		Actor::InteractiveClient { .. } | Actor::RemoteClient { .. } => {}
	}
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

/// Records a prepared root as a Project, journals it, and records the
/// widened access in the Security audit, all in the transaction that
/// commits it (ADR-0105).
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
