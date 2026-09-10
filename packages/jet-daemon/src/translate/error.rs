//! Stable wire errors and recovery metadata.

use super::{
	conversation::{conversation, run},
	file_revision, file_target,
};
use jet_core::{
	ConflictState, CoreError, ErrorCategory, RecoveryAction, RevisionConflict,
};
use jet_protocol as wire;

pub(crate) fn error(error: CoreError, minor: u32) -> wire::WireError {
	let restart = (minor >= wire::FENCED_READS_MINOR)
		.then(|| error.recovery_actions.iter().find_map(restart_metadata))
		.flatten();
	wire::WireError {
		category: category(error.category),
		code: error.code,
		retryable: error.retryable,
		message: error.message,
		revision_conflict: error
			.revision_conflict
			.map(|conflict| revision_conflict(conflict, minor)),
		restart,
		recovery_actions: error
			.recovery_actions
			.into_iter()
			.filter_map(recovery_action)
			.collect(),
	}
}

pub(super) fn recovery_action(
	action: RecoveryAction,
) -> Option<wire::RecoveryAction> {
	match action {
		RecoveryAction::RefreshFile {
			target,
			path,
			current_revision,
		} => Some(wire::RecoveryAction::RefreshFile {
			target: file_target(target),
			path,
			current_revision: file_revision(current_revision),
		}),
		RecoveryAction::RefreshConversation { conversation_id } => {
			Some(wire::RecoveryAction::RefreshConversation {
				conversation_id: conversation_id.0,
			})
		}
		RecoveryAction::RefreshRun { run_id } => {
			Some(wire::RecoveryAction::RefreshRun { run_id: run_id.0 })
		}
		RecoveryAction::RestartSnapshot { .. } => None,
	}
}

pub(super) fn restart_metadata(
	action: &RecoveryAction,
) -> Option<wire::RestartMetadata> {
	match action {
		RecoveryAction::RefreshFile { .. }
		| RecoveryAction::RefreshConversation { .. }
		| RecoveryAction::RefreshRun { .. } => None,
		RecoveryAction::RestartSnapshot { metadata } => Some(match metadata {
			jet_core::RestartMetadata::CursorExpired {
				minimum_available_cursor,
				current_snapshot_revision,
			} => wire::RestartMetadata::CursorExpired {
				minimum_available_cursor: minimum_available_cursor.0,
				current_snapshot_revision: current_snapshot_revision.0,
			},
			jet_core::RestartMetadata::CursorAhead {
				current_snapshot_revision,
			} => wire::RestartMetadata::CursorAhead {
				current_snapshot_revision: current_snapshot_revision.0,
			},
			jet_core::RestartMetadata::PaginationStale {
				current_snapshot_revision,
			} => wire::RestartMetadata::PaginationStale {
				current_snapshot_revision: current_snapshot_revision.0,
			},
		}),
	}
}

pub(super) fn revision_conflict(
	conflict: RevisionConflict,
	minor: u32,
) -> wire::RevisionConflict {
	wire::RevisionConflict {
		current_revision: conflict.current_revision.0,
		safe_state: match conflict.safe_state {
			ConflictState::Conversation(current) => {
				wire::ConflictState::Conversation {
					conversation: conversation(&current, minor),
				}
			}
			ConflictState::Run(current) => wire::ConflictState::Run {
				run: run(&current, minor),
			},
		},
	}
}

pub(super) fn category(category: ErrorCategory) -> wire::ErrorCategory {
	match category {
		ErrorCategory::InvalidInput => wire::ErrorCategory::InvalidInput,
		ErrorCategory::Unauthorized => wire::ErrorCategory::Unauthorized,
		ErrorCategory::Conflict => wire::ErrorCategory::Conflict,
		ErrorCategory::Unavailable => wire::ErrorCategory::Unavailable,
		ErrorCategory::Incompatible => wire::ErrorCategory::Incompatible,
		ErrorCategory::RateLimited => wire::ErrorCategory::RateLimited,
		ErrorCategory::NotFound => wire::ErrorCategory::NotFound,
		ErrorCategory::OutcomeUnknown => wire::ErrorCategory::OutcomeUnknown,
		ErrorCategory::Internal => wire::ErrorCategory::Internal,
	}
}
