//! Jet Trash and retention previews across the wire (ADR-0011, ADR-0015).

use super::unix_ms;
use jet_core::{
	ConversationTrash, Protection, RetentionPreview, TrashEntry, TrashReason,
};
use jet_protocol as wire;

pub(super) fn entry(entry: TrashEntry) -> wire::TrashEntry {
	wire::TrashEntry {
		conversation_id: entry.conversation_id.0,
		reason: reason(entry.reason),
		trashed_at_unix_ms: unix_ms(entry.trashed_at),
		expires_at_unix_ms: unix_ms(entry.expires_at),
	}
}

fn reason(reason: TrashReason) -> wire::TrashReason {
	match reason {
		TrashReason::ManualForget => wire::TrashReason::ManualForget,
		TrashReason::AutomaticForget => wire::TrashReason::AutomaticForget,
		TrashReason::DeleteEverywhere => wire::TrashReason::DeleteEverywhere,
		TrashReason::AutodeleteRule => wire::TrashReason::AutodeleteRule,
		TrashReason::AutodeleteEverywhere => {
			wire::TrashReason::AutodeleteEverywhere
		}
		TrashReason::PlaneTransfer => wire::TrashReason::PlaneTransfer,
	}
}

/// Whether a peer at `minor` can read `entries`. The two Autodelete
/// reasons arrived with minor 40 and the Transfer tombstone with 42; an
/// older peer is refused the page rather than shown a reason it would
/// misread (ADR-0019).
fn readable(
	entries: &[TrashEntry],
	minor: u32,
) -> Result<(), jet_core::CoreError> {
	let needs_autodelete_peer = minor < wire::AUTODELETE_MINOR
		&& entries.iter().any(|entry| {
			matches!(
				entry.reason,
				TrashReason::AutodeleteRule | TrashReason::AutodeleteEverywhere
			)
		});
	if needs_autodelete_peer {
		return Err(jet_core::CoreError::incompatible(
			"retention.reason_incompatible",
			"Jet Trash holds Conversations staged by an Autodelete rule; upgrade the client to read it",
		));
	}
	let needs_transfer_peer = minor < wire::PLANE_TRANSFER_MINOR
		&& entries
			.iter()
			.any(|entry| entry.reason == TrashReason::PlaneTransfer);
	if needs_transfer_peer {
		return Err(jet_core::CoreError::incompatible(
			"retention.reason_incompatible",
			"Jet Trash holds a Transfer tombstone; upgrade the client to read it",
		));
	}
	Ok(())
}

pub(super) fn protection(protection: Protection) -> wire::RetentionProtection {
	match protection {
		Protection::ActiveRun => wire::RetentionProtection::ActiveRun,
		Protection::PendingTurn => wire::RetentionProtection::PendingTurn,
		Protection::EnabledSchedule => {
			wire::RetentionProtection::EnabledSchedule
		}
		Protection::DirtyWorkspace => wire::RetentionProtection::DirtyWorkspace,
		Protection::UnpushedWork => wire::RetentionProtection::UnpushedWork,
		Protection::UnresolvedEffect => {
			wire::RetentionProtection::UnresolvedEffect
		}
	}
}

pub(super) fn trash(
	trash: ConversationTrash,
	minor: u32,
) -> Result<wire::ConversationTrash, jet_core::CoreError> {
	readable(&trash.entries, minor)?;
	Ok(wire::ConversationTrash {
		cursor: trash.cursor.0,
		entries: trash.entries.into_iter().map(entry).collect(),
	})
}

pub(super) fn preview(
	preview: RetentionPreview,
	minor: u32,
) -> Result<wire::RetentionPreview, jet_core::CoreError> {
	readable(preview.trash.as_slice(), minor)?;
	Ok(wire::RetentionPreview {
		conversation_id: preview.conversation_id.0,
		protections: preview.protections.into_iter().map(protection).collect(),
		trash: preview.trash.map(entry),
		audit_records: u64::try_from(preview.audit_records).unwrap_or(u64::MAX),
	})
}
