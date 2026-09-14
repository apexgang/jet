//! The Project half of the translation seam (ADR-0049, ADR-0101).

use super::{actor_of, unix_ms};
use jet_core::{
	Checkout, ClientId, Disposition, EntryKind, GitLink, PermanentRemoval,
	Project, ProjectDisposal, ProjectEntry, ProjectId, ProjectList,
	ProjectPreview, ProjectRemovalBinding, ProjectRemovalPreview,
	ProjectRemoved, Registrability, RemovalObstacle, Repository,
	ToolAvailability, WorkspaceId, Worktree,
};
use jet_protocol as wire;
use std::path::PathBuf;

pub(super) fn entry(entry: ProjectEntry) -> wire::ProjectEntry {
	wire::ProjectEntry {
		cursor: entry.cursor.0,
		project_id: entry.project_id.0,
		path: entry.path.as_str().into(),
		kind: match entry.kind {
			EntryKind::File { bytes } => wire::EntryKind::File { bytes },
			EntryKind::Directory => wire::EntryKind::Directory,
			EntryKind::Other => wire::EntryKind::Other,
			EntryKind::Missing => wire::EntryKind::Missing,
		},
	}
}

pub(super) fn preview(preview: ProjectPreview) -> wire::ProjectPreview {
	wire::ProjectPreview {
		root: preview.root.display().to_string(),
		registrability: registrability(preview.registrability),
	}
}

fn registrability(registrability: Registrability) -> wire::Registrability {
	match registrability {
		Registrability::Registrable(described) => {
			wire::Registrability::Registrable {
				repository: repository(described),
			}
		}
		Registrability::NotARepository => wire::Registrability::NotARepository,
		Registrability::BrokenRepository => {
			wire::Registrability::BrokenRepository
		}
		Registrability::BareRepository => wire::Registrability::BareRepository,
		Registrability::InsideGitDir => wire::Registrability::InsideGitDir,
		Registrability::InsideWorkingTree { toplevel } => {
			wire::Registrability::InsideWorkingTree {
				toplevel: toplevel.display().to_string(),
			}
		}
	}
}

fn repository(repository: Repository) -> wire::Repository {
	wire::Repository {
		worktree: match repository.worktree {
			Worktree::Main => wire::Worktree::Main,
			Worktree::Linked { common_dir } => wire::Worktree::Linked {
				common_dir: common_dir.display().to_string(),
			},
		},
		checkout: match repository.checkout {
			Checkout::Full => wire::Checkout::Full,
			Checkout::Sparse => wire::Checkout::Sparse,
		},
		submodules: repository
			.submodules
			.into_iter()
			.map(|GitLink { path, commit }| wire::GitLink { path, commit })
			.collect(),
		lfs: match repository.lfs {
			ToolAvailability::Present { version } => {
				wire::ToolAvailability::Present { version }
			}
			ToolAvailability::Missing => wire::ToolAvailability::Missing,
		},
	}
}

pub(super) fn list(list: ProjectList) -> wire::ProjectList {
	wire::ProjectList {
		cursor: list.cursor.0,
		projects: list.projects.into_iter().map(project).collect(),
	}
}

pub(super) fn project(project: Project) -> wire::Project {
	wire::Project {
		project_id: project.project_id.0,
		root: project.root.display().to_string(),
		registered_by: actor_of(project.registered_by),
		registered_at_unix_ms: unix_ms(project.registered_at),
	}
}

pub(super) fn removal_preview(
	preview: ProjectRemovalPreview,
) -> wire::ProjectRemovalPreview {
	wire::ProjectRemovalPreview {
		cursor: preview.cursor.0,
		binding: removal_binding(preview.binding),
		disk_use_bytes: preview.disk_use_bytes,
		obstacles: preview.obstacles.into_iter().map(obstacle).collect(),
		permanent_removal_warning: preview.permanent_removal_warning.into(),
	}
}

fn removal_binding(
	binding: ProjectRemovalBinding,
) -> wire::ProjectRemovalBinding {
	wire::ProjectRemovalBinding {
		project_id: binding.project_id.0,
		root: binding.root.display().to_string(),
		live_runs: binding.live_runs,
		schedules: binding.schedules,
		dirty_files: binding.dirty_files,
		unpushed_commits: binding.unpushed_commits,
		workspaces: binding.workspaces.into_iter().map(|id| id.0).collect(),
		actor: binding.actor.0,
	}
}

pub(super) fn removal_binding_from_wire(
	binding: &wire::ProjectRemovalBinding,
) -> ProjectRemovalBinding {
	ProjectRemovalBinding {
		project_id: ProjectId(binding.project_id),
		root: PathBuf::from(&binding.root),
		live_runs: binding.live_runs,
		schedules: binding.schedules,
		dirty_files: binding.dirty_files,
		unpushed_commits: binding.unpushed_commits,
		workspaces: binding
			.workspaces
			.iter()
			.copied()
			.map(WorkspaceId)
			.collect(),
		actor: ClientId(binding.actor),
	}
}

/// The core form of a disposal. A permanent one is accepted only with the
/// warning acknowledged word for word.
pub(super) fn disposal_from_wire(
	disposal: &wire::ProjectDisposal,
) -> Result<ProjectDisposal, jet_core::CoreError> {
	match disposal {
		wire::ProjectDisposal::SystemTrash => Ok(ProjectDisposal::SystemTrash),
		wire::ProjectDisposal::Permanent {
			acknowledged_warning,
		} => PermanentRemoval::acknowledging(acknowledged_warning)
			.map(ProjectDisposal::Permanent),
	}
}

fn obstacle(obstacle: RemovalObstacle) -> wire::RemovalObstacle {
	match obstacle {
		RemovalObstacle::LiveRuns => wire::RemovalObstacle::LiveRuns,
		RemovalObstacle::Schedules => wire::RemovalObstacle::Schedules,
		RemovalObstacle::FilesystemRoot => {
			wire::RemovalObstacle::FilesystemRoot
		}
		RemovalObstacle::UserHome => wire::RemovalObstacle::UserHome,
		RemovalObstacle::JetHome => wire::RemovalObstacle::JetHome,
		RemovalObstacle::ContainsProject => {
			wire::RemovalObstacle::ContainsProject
		}
	}
}

pub(super) fn removed(removed: ProjectRemoved) -> wire::ProjectRemoved {
	wire::ProjectRemoved {
		project_id: removed.project_id.0,
		root: removed.root.display().to_string(),
		disposition: match removed.disposition {
			Disposition::Trashed => wire::Disposition::Trashed,
			Disposition::Deleted => wire::Disposition::Deleted,
		},
	}
}
