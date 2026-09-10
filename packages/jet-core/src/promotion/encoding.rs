//! Converts promotion records and conflict descriptions.

use super::{
	ChangeKind, ConflictKind, PromotedChange, PromotionBinding,
	PromotionConflict, PromotionDestination, PromotionId, PromotionState,
	WorkspacePromotion,
};
use crate::{
	Actor, system_time,
	workspace::{WorkspaceId, tree_capture::Change},
};
use jet_store::{
	PromotionConflictKindRecord, PromotionConflictRecord,
	PromotionDestinationRecord, PromotionStateRecord, WorkspacePromotionRecord,
};

impl From<Change> for PromotedChange {
	fn from(change: Change) -> Self {
		Self {
			kind: if change.is_addition() {
				ChangeKind::Added
			} else if change.is_deletion() {
				ChangeKind::Deleted
			} else {
				ChangeKind::Modified
			},
			path: change.path,
		}
	}
}

impl From<WorkspacePromotionRecord> for WorkspacePromotion {
	fn from(record: WorkspacePromotionRecord) -> Self {
		Self {
			promotion_id: PromotionId(record.promotion_id),
			binding: PromotionBinding {
				workspace_id: WorkspaceId(record.workspace_id),
				destination: match record.destination {
					PromotionDestinationRecord::LocalCheckout => {
						PromotionDestination::LocalCheckout
					}
					PromotionDestinationRecord::Branch(name) => {
						PromotionDestination::Branch(name)
					}
				},
				base_commit: record.base_commit,
				workspace_tree: record.workspace_tree,
				destination_commit: record.destination_commit,
				destination_tree: record.destination_tree,
				result_tree: record.result_tree,
				destination_dirty: record.destination_dirty,
				conflicts: record
					.conflicts
					.into_iter()
					.map(Into::into)
					.collect(),
				actor: Actor::from_record(record.promoted_by).client_id(),
			},
			changed_paths: record.changed_paths,
			state: match record.state {
				PromotionStateRecord::Applying => PromotionState::Applying,
				PromotionStateRecord::Promoted => PromotionState::Promoted,
				PromotionStateRecord::Conflicted => PromotionState::Conflicted,
				PromotionStateRecord::Failed => PromotionState::Failed,
				PromotionStateRecord::OutcomeUnknown => {
					PromotionState::OutcomeUnknown
				}
			},
			recorded_at: system_time(record.recorded_at_unix_ms),
			settled_at: record.settled_at_unix_ms.map(system_time),
		}
	}
}

impl From<PromotionConflictRecord> for PromotionConflict {
	fn from(record: PromotionConflictRecord) -> Self {
		Self {
			path: record.path,
			kind: match record.kind {
				PromotionConflictKindRecord::Diverged => ConflictKind::Diverged,
				PromotionConflictKindRecord::Untracked => {
					ConflictKind::Untracked
				}
				PromotionConflictKindRecord::Staged => ConflictKind::Staged,
			},
		}
	}
}

impl From<&PromotionConflict> for PromotionConflictRecord {
	fn from(conflict: &PromotionConflict) -> Self {
		Self {
			path: conflict.path.clone(),
			kind: match conflict.kind {
				ConflictKind::Diverged => PromotionConflictKindRecord::Diverged,
				ConflictKind::Untracked => {
					PromotionConflictKindRecord::Untracked
				}
				ConflictKind::Staged => PromotionConflictKindRecord::Staged,
			},
		}
	}
}

impl From<&PromotionDestination> for PromotionDestinationRecord {
	fn from(destination: &PromotionDestination) -> Self {
		match destination {
			PromotionDestination::LocalCheckout => Self::LocalCheckout,
			PromotionDestination::Branch(name) => Self::Branch(name.clone()),
		}
	}
}
