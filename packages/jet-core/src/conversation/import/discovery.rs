//! Discovers external Conversations and matches registered Projects.

use super::{
	DiscoveredConversation, ExternalConversation, ExternalConversationList,
	ExternalOrigin, ImportedConversation,
};
use crate::{
	Core, ProjectId, error::CoreError, event::EventSequence,
	filesystem::canonicalize, project::Project, query::QueryResult,
};
use std::path::{Path, PathBuf};

/// Lists what the Plane can see and what it holds.
///
/// Discovery runs outside the store's lock; the imports, the Projects the
/// discovered directories are placed against, and the Event fence come
/// from one SQLite snapshot (ASVS 2.3.3).
pub(crate) async fn external_conversations(
	core: &Core,
) -> Result<QueryResult, CoreError> {
	let mut discovered = core.discovery.discover().await;
	for found in &mut discovered {
		found.working_directory = resolve(found.working_directory.take()).await;
	}
	core.store
		.read(async |tx| {
			let cursor = EventSequence(tx.event_cursor().await?);
			let projects: Vec<Project> =
				tx.projects().await?.into_iter().map(Into::into).collect();
			let imported: Vec<ImportedConversation> = tx
				.imported_conversations()
				.await?
				.into_iter()
				.map(Into::into)
				.collect();
			let discovered = discovered
				.into_iter()
				.map(|found| present(found, &projects, &imported))
				.collect();
			Ok(QueryResult::ExternalConversations(
				ExternalConversationList {
					cursor,
					discovered,
					imported,
				},
			))
		})
		.await
}

/// The directory as the filesystem names it, so it compares with Project
/// roots, which are canonical. A directory that is gone, or that was never
/// reported, is kept as reported: what the Harness said is still metadata.
pub(super) async fn resolve(directory: Option<PathBuf>) -> Option<PathBuf> {
	let directory = directory?;
	Some(canonicalize(directory.clone()).await.unwrap_or(directory))
}

/// Places one discovered identity against the Plane's Projects and imports.
pub(super) fn present(
	found: DiscoveredConversation,
	projects: &[Project],
	imported: &[ImportedConversation],
) -> ExternalConversation {
	let DiscoveredConversation {
		harness,
		native_conversation,
		working_directory,
		process,
	} = found;
	let import_id = imported
		.iter()
		.find(|import| {
			import.harness == harness
				&& import.native_conversation == native_conversation
		})
		.map(|import| import.import_id);
	let origin = match working_directory {
		Some(working_directory) => match covering(projects, &working_directory)
		{
			Some(project_id) => ExternalOrigin::Project {
				project_id,
				working_directory,
			},
			None => ExternalOrigin::Unregistered { working_directory },
		},
		None => ExternalOrigin::Unknown,
	};
	ExternalConversation {
		harness,
		native_conversation,
		origin,
		process,
		import_id,
	}
}

/// The Project whose root holds `directory`, preferring the deepest root
/// when a Project sits inside another (ADR-0103).
pub(super) fn covering(
	projects: &[Project],
	directory: &Path,
) -> Option<ProjectId> {
	projects
		.iter()
		.filter(|project| directory.starts_with(&project.root))
		.max_by_key(|project| project.root.components().count())
		.map(|project| project.project_id)
}
