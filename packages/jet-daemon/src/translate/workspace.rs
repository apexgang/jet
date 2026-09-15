//! Working tree, Base and Seed selection, and Workspace translation.

use super::{promotion, unix_ms};
use jet_core::{
	BaseSelection, CoreError, ProjectId, RelativePath, SeedSelection,
	WorkingTree, WorkingTreeRequest, Workspace, WorkspaceBase,
};
use jet_protocol as wire;

pub(super) fn working_tree(working_tree: WorkingTree) -> wire::WorkingTree {
	match working_tree {
		WorkingTree::NoProject => wire::WorkingTree::NoProject,
		WorkingTree::Workspace { project_id } => wire::WorkingTree::Workspace {
			project_id: project_id.0,
		},
		WorkingTree::LocalCheckout { project_id } => {
			wire::WorkingTree::LocalCheckout {
				project_id: project_id.0,
			}
		}
	}
}

pub(super) fn working_tree_request(
	request: &wire::WorkingTreeRequest,
) -> Result<WorkingTreeRequest, CoreError> {
	Ok(match request {
		wire::WorkingTreeRequest::NoProject => WorkingTreeRequest::NoProject,
		wire::WorkingTreeRequest::Workspace {
			project_id,
			base,
			seed,
		} => WorkingTreeRequest::Workspace {
			project_id: ProjectId(*project_id),
			base: match base {
				wire::BaseSelection::Head => BaseSelection::Head,
				wire::BaseSelection::Revision { revision } => {
					BaseSelection::Revision(revision.clone())
				}
			},
			seed: seed_selection(seed)?,
		},
		wire::WorkingTreeRequest::LocalCheckout { project_id } => {
			WorkingTreeRequest::LocalCheckout {
				project_id: ProjectId(*project_id),
			}
		}
	})
}

fn seed_selection(
	selection: &wire::SeedSelection,
) -> Result<SeedSelection, CoreError> {
	Ok(match selection {
		wire::SeedSelection::None => SeedSelection::None,
		wire::SeedSelection::AllEligible => SeedSelection::AllEligible,
		wire::SeedSelection::Paths { paths } => SeedSelection::Paths(
			paths
				.iter()
				.map(|path| RelativePath::parse(path))
				.collect::<Result<_, _>>()?,
		),
	})
}

pub(super) fn workspace(workspace: &Workspace, minor: u32) -> wire::Workspace {
	let WorkspaceBase { selection, commit } = &workspace.base;
	wire::Workspace {
		workspace_id: workspace.workspace_id.0,
		conversation_id: workspace.conversation_id.0,
		project_id: workspace.project_id.0,
		root: workspace.root.display().to_string(),
		base: wire::WorkspaceBase {
			selection: match selection {
				BaseSelection::Head => wire::BaseSelection::Head,
				BaseSelection::Revision(revision) => {
					wire::BaseSelection::Revision {
						revision: revision.clone(),
					}
				}
			},
			commit: commit.clone(),
		},
		// A client that negotiated an older minor is not told about a seed
		// it cannot decode (ADR-0019).
		seed: (minor >= wire::SEEDED_WORKSPACES_MINOR)
			.then(|| {
				workspace.seed.as_ref().map(|seed| wire::WorkspaceSeed {
					tree: seed.tree.clone(),
					changed_paths: seed.changed_paths,
				})
			})
			.flatten(),
		// Nor about a promotion it cannot decode.
		promotion: (minor >= wire::WORKSPACE_PROMOTION_MINOR)
			.then(|| workspace.promotion.clone().map(promotion::promotion))
			.flatten(),
		created_at_unix_ms: unix_ms(workspace.created_at),
	}
}
