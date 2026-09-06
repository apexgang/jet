//! The Workspace promotion half of the translation seam (ADR-0025,
//! ADR-0049).

use jet_core::{
	ChangeKind, ConflictKind, PromotedChange, PromotionBinding,
	PromotionConflict, PromotionDestination, PromotionPreview,
};
use jet_protocol as wire;

pub(super) fn destination_from_wire(
	destination: &wire::PromotionDestination,
) -> PromotionDestination {
	match destination {
		wire::PromotionDestination::LocalCheckout => {
			PromotionDestination::LocalCheckout
		}
		wire::PromotionDestination::Branch { name } => {
			PromotionDestination::Branch(name.clone())
		}
	}
}

fn destination(
	destination: PromotionDestination,
) -> wire::PromotionDestination {
	match destination {
		PromotionDestination::LocalCheckout => {
			wire::PromotionDestination::LocalCheckout
		}
		PromotionDestination::Branch(name) => {
			wire::PromotionDestination::Branch { name }
		}
	}
}

pub(super) fn binding(binding: PromotionBinding) -> wire::PromotionBinding {
	wire::PromotionBinding {
		workspace_id: binding.workspace_id.0,
		destination: destination(binding.destination),
		base_commit: binding.base_commit,
		workspace_tree: binding.workspace_tree,
		destination_commit: binding.destination_commit,
		destination_tree: binding.destination_tree,
		result_tree: binding.result_tree,
		actor: binding.actor.0,
	}
}

pub(super) fn preview(preview: PromotionPreview) -> wire::PromotionPreview {
	wire::PromotionPreview {
		cursor: preview.cursor.0,
		binding: binding(preview.binding),
		destination_dirty: preview.destination_dirty,
		changed_paths: preview.changed_paths,
		changes: preview.changes.into_iter().map(change).collect(),
		conflicts: preview.conflicts.into_iter().map(conflict).collect(),
	}
}

fn change(change: PromotedChange) -> wire::PromotedChange {
	wire::PromotedChange {
		path: change.path,
		kind: match change.kind {
			ChangeKind::Added => wire::ChangeKind::Added,
			ChangeKind::Modified => wire::ChangeKind::Modified,
			ChangeKind::Deleted => wire::ChangeKind::Deleted,
		},
	}
}

fn conflict(conflict: PromotionConflict) -> wire::PromotionConflict {
	wire::PromotionConflict {
		path: conflict.path,
		kind: match conflict.kind {
			ConflictKind::Diverged => wire::ConflictKind::Diverged,
			ConflictKind::Untracked => wire::ConflictKind::Untracked,
		},
	}
}
