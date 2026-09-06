//! The Workspace promotion half of the translation seam (ADR-0025,
//! ADR-0049).

use jet_core::{
	ChangeKind, ClientId, ConflictKind, PromotedChange, PromotionBinding,
	PromotionConflict, PromotionDestination, PromotionPreview, PromotionState,
	WorkspaceId, WorkspacePromotion,
};
use jet_protocol as wire;

use super::unix_ms;

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
		destination_dirty: binding.destination_dirty,
		conflicts: binding.conflicts.into_iter().map(conflict).collect(),
		actor: binding.actor.0,
	}
}

pub(super) fn preview(preview: PromotionPreview) -> wire::PromotionPreview {
	wire::PromotionPreview {
		cursor: preview.cursor.0,
		binding: binding(preview.binding),
		changed_paths: preview.changed_paths,
		changes: preview.changes.into_iter().map(change).collect(),
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
			ConflictKind::Staged => wire::ConflictKind::Staged,
		},
	}
}

pub(super) fn binding_from_wire(
	binding: &wire::PromotionBinding,
) -> PromotionBinding {
	PromotionBinding {
		workspace_id: WorkspaceId(binding.workspace_id),
		destination: destination_from_wire(&binding.destination),
		base_commit: binding.base_commit.clone(),
		workspace_tree: binding.workspace_tree.clone(),
		destination_commit: binding.destination_commit.clone(),
		destination_tree: binding.destination_tree.clone(),
		result_tree: binding.result_tree.clone(),
		destination_dirty: binding.destination_dirty,
		conflicts: binding.conflicts.iter().map(conflict_from_wire).collect(),
		actor: ClientId(binding.actor),
	}
}

fn conflict_from_wire(conflict: &wire::PromotionConflict) -> PromotionConflict {
	PromotionConflict {
		path: conflict.path.clone(),
		kind: match conflict.kind {
			wire::ConflictKind::Diverged => ConflictKind::Diverged,
			wire::ConflictKind::Untracked => ConflictKind::Untracked,
			wire::ConflictKind::Staged => ConflictKind::Staged,
		},
	}
}

pub(super) fn promotion(
	promotion: WorkspacePromotion,
) -> wire::WorkspacePromotion {
	wire::WorkspacePromotion {
		promotion_id: promotion.promotion_id.0,
		binding: binding(promotion.binding),
		changed_paths: promotion.changed_paths,
		state: match promotion.state {
			PromotionState::Applying => wire::PromotionState::Applying,
			PromotionState::Promoted => wire::PromotionState::Promoted,
			PromotionState::Conflicted => wire::PromotionState::Conflicted,
			PromotionState::Failed => wire::PromotionState::Failed,
			PromotionState::OutcomeUnknown => {
				wire::PromotionState::OutcomeUnknown
			}
		},
		recorded_at_unix_ms: unix_ms(promotion.recorded_at),
		settled_at_unix_ms: promotion.settled_at.map(unix_ms),
	}
}
