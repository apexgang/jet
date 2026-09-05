//! The Project half of the translation seam (ADR-0049, ADR-0101).

use jet_core::{
	Checkout, GitLink, Project, ProjectList, ProjectPreview, Registrability,
	Repository, ToolAvailability, Worktree,
};
use jet_protocol as wire;

use super::{actor_of, unix_ms};

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
