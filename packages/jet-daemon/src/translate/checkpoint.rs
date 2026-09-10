//! Explicit domain-to-wire mapping for checkpoint protocol minor 17.
use jet_core as core;
use jet_protocol as wire;
pub(super) fn scope(value: &wire::DiffScope) -> core::DiffScope {
	match *value {
		wire::DiffScope::Turn { turn } => core::DiffScope::Turn { turn },
		wire::DiffScope::Current => core::DiffScope::Current,
		wire::DiffScope::Final => core::DiffScope::Final,
		wire::DiffScope::Historical { from_turn, to_turn } => {
			core::DiffScope::Historical { from_turn, to_turn }
		}
	}
}
pub(super) fn diff(value: core::ChangeDiff, minor: u32) -> wire::ChangeDiff {
	wire::ChangeDiff {
		total_files: value.total_files,
		next_page: value.next_page.map(|cursor| wire::PageCursor(cursor.0)),
		cursor: value.cursor.0,
		plane_id: value.plane_id.0,
		workspace_id: value.workspace_id.map(|id| id.0),
		run_id: value.run_id.0,
		scope: match value.scope {
			core::DiffScope::Turn { turn } => wire::DiffScope::Turn { turn },
			core::DiffScope::Current => wire::DiffScope::Current,
			core::DiffScope::Final => wire::DiffScope::Final,
			core::DiffScope::Historical { from_turn, to_turn } => {
				wire::DiffScope::Historical { from_turn, to_turn }
			}
		},
		latest_turn: value.latest_turn,
		outcome: value.outcome.map(|outcome| match outcome {
			core::TurnOutcome::Completed => wire::TurnOutcome::Completed,
			core::TurnOutcome::Interrupted => wire::TurnOutcome::Interrupted,
		}),
		before: snapshot(value.before, minor),
		after: snapshot(value.after, minor),
		artifact: artifact(value.artifact, minor),
		files: value
			.files
			.into_iter()
			.map(|f| wire::ChangedFile {
				path: f.path,
				before_size: f.before_size,
				after_size: f.after_size,
				before_object: f.before_object,
				after_object: f.after_object,
				before_mode: f.before_mode,
				after_mode: f.after_mode,
				origin: match f.origin {
					core::ChangeOrigin::ExternalOrUnknown => {
						wire::ChangeOrigin::ExternalOrUnknown
					}
					core::ChangeOrigin::Mixed => wire::ChangeOrigin::Mixed,
					core::ChangeOrigin::UserEdit { client_id } => {
						wire::ChangeOrigin::UserEdit {
							client_id: client_id.0,
						}
					}
					core::ChangeOrigin::WorkspaceTerminal { terminal_id } => {
						wire::ChangeOrigin::WorkspaceTerminal { terminal_id }
					}
					core::ChangeOrigin::Harness { run_id } => {
						wire::ChangeOrigin::Harness { run_id: run_id.0 }
					}
				},
			})
			.collect(),
		patch: value.patch,
		patch_truncated: value.patch_truncated,
	}
}
fn snapshot(value: core::ChangeSnapshot, minor: u32) -> wire::ChangeSnapshot {
	wire::ChangeSnapshot {
		content_complete: value.omitted_files.is_empty()
			&& value.uncommitted.availability
				!= core::ArtifactAvailability::DiskPressure,
		commit: value.commit,
		tree: value.tree,
		uncommitted: artifact(value.uncommitted, minor),
	}
}
pub(super) fn artifact(
	value: core::ChangeArtifact,
	minor: u32,
) -> wire::ChangeArtifact {
	wire::ChangeArtifact {
		availability: match value.availability {
			core::ArtifactAvailability::DiskPressure
				if minor >= wire::DISK_PRESSURE_MINOR =>
			{
				wire::ArtifactAvailability::DiskPressure
			}
			core::ArtifactAvailability::DiskPressure => {
				wire::ArtifactAvailability::RunBudgetExceeded
			}
			core::ArtifactAvailability::Stored => {
				wire::ArtifactAvailability::Stored
			}
			core::ArtifactAvailability::RunBudgetExceeded => {
				wire::ArtifactAvailability::RunBudgetExceeded
			}
			core::ArtifactAvailability::ArtifactSizeExceeded => {
				wire::ArtifactAvailability::ArtifactSizeExceeded
			}
		},
		sha256: value.sha256,
		size: value.size,
	}
}
