//! User-edit targets and file revisions.

use jet_core::{FileRevision, FileTarget, ProjectId, WorkspaceId};
use jet_protocol as wire;

pub(super) fn file_target_from_wire(target: wire::FileTarget) -> FileTarget {
	match target {
		wire::FileTarget::Project { project_id } => FileTarget::Project {
			project_id: ProjectId(project_id),
		},
		wire::FileTarget::Workspace { workspace_id } => FileTarget::Workspace {
			workspace_id: WorkspaceId(workspace_id),
		},
	}
}

pub(super) fn file_target(target: FileTarget) -> wire::FileTarget {
	match target {
		FileTarget::Project { project_id } => wire::FileTarget::Project {
			project_id: project_id.0,
		},
		FileTarget::Workspace { workspace_id } => wire::FileTarget::Workspace {
			workspace_id: workspace_id.0,
		},
	}
}

pub(super) fn file_revision_from_wire(
	revision: &wire::FileRevision,
) -> FileRevision {
	FileRevision {
		object: revision.object.clone(),
		mode: revision.mode.clone(),
	}
}

pub(super) fn file_revision(revision: FileRevision) -> wire::FileRevision {
	wire::FileRevision {
		object: revision.object,
		mode: revision.mode,
	}
}
