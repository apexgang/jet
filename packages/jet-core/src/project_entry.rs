//! One path inside a registered Project, addressed the way every ordinary
//! file operation addresses a file: by the Project and a validated
//! relative path (ADR-0101). Workspaces address files the same way once
//! they exist (ADR-0025).

use std::path::{Path, PathBuf};

use crate::error::CoreError;
use crate::event::EventSequence;
use crate::query::QueryResult;
use crate::relative_path::{GrantedRoot, RelativePath};
use crate::repository::blocking;
use crate::{Core, ProjectId};

/// One entry inside a registered Project, addressed the way every ordinary
/// file operation addresses a file: by the Project and a validated
/// relative path (ADR-0101). It carries metadata and never content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEntry {
	/// Newest Event sequence visible when the Project was read.
	pub cursor: EventSequence,
	/// The Project the path was resolved in.
	pub project_id: ProjectId,
	/// The path as it was asked for.
	pub path: RelativePath,
	/// What the path names right now.
	pub kind: EntryKind,
}

/// What one path inside a Project names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
	/// A regular file.
	File {
		/// Its length in bytes.
		bytes: u64,
	},
	/// A directory.
	Directory,
	/// Something else the filesystem holds, such as a socket.
	Other,
	/// Nothing yet.
	Missing,
}

/// Describes what `path` names inside `project_id`.
///
/// The Project is read with its Event fence in one store snapshot (ASVS
/// 2.3.3); the filesystem is then examined on a blocking thread: the root
/// must still be the directory that was granted, and the path resolves
/// under it without following a link outside (ADR-0101).
///
/// # Errors
///
/// Returns `project.not_found` for an unknown Project, what
/// [`GrantedRoot::verify`] and [`RelativePath::resolve_within`] refuse,
/// and an `unavailable` `path.unreadable` when the entry cannot be
/// examined.
pub(crate) async fn entry(
	core: &Core,
	project_id: ProjectId,
	path: RelativePath,
) -> Result<QueryResult, CoreError> {
	let (cursor, record) = core
		.store
		.read(async |tx| {
			let cursor = EventSequence(tx.event_cursor().await?);
			let record = tx.project(project_id.0).await?;
			Ok::<_, CoreError>((cursor, record))
		})
		.await?;
	let Some(record) = record else {
		return Err(CoreError::not_found(
			"project.not_found",
			"the Project does not exist",
		));
	};
	let root = PathBuf::from(record.root);
	let kind = {
		let path = path.clone();
		blocking(move || {
			let root = GrantedRoot::verify(&root)?;
			entry_kind(&path.resolve_within(&root)?)
		})
		.await??
	};
	Ok(QueryResult::ProjectEntry(ProjectEntry {
		cursor,
		project_id,
		path,
		kind,
	}))
}

/// What the resolved path names. Resolution has already followed every
/// link that exists, so what is examined here is the thing itself.
fn entry_kind(resolved: &Path) -> Result<EntryKind, CoreError> {
	match std::fs::metadata(resolved) {
		Ok(metadata) if metadata.is_file() => Ok(EntryKind::File {
			bytes: metadata.len(),
		}),
		Ok(metadata) if metadata.is_dir() => Ok(EntryKind::Directory),
		Ok(_) => Ok(EntryKind::Other),
		Err(error)
			if matches!(
				error.kind(),
				std::io::ErrorKind::NotFound
					| std::io::ErrorKind::NotADirectory
			) =>
		{
			Ok(EntryKind::Missing)
		}
		Err(error) => Err(CoreError::unavailable(
			"path.unreadable",
			"the entry cannot be examined",
			error.to_string(),
		)),
	}
}
