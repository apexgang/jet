//! The Project half of the translation seam (ADR-0049, ADR-0101).

use jet_core::{Project, ProjectList};
use jet_protocol as wire;

use super::{actor_of, unix_ms};

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
