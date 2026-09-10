//! Read-only metadata when capturing new Git objects would consume the reserve.
use crate::{ArtifactAvailability, ChangeArtifact, ChangeSnapshot, CoreError};
use std::path::Path;

pub(crate) async fn snapshot(root: &Path) -> Result<ChangeSnapshot, CoreError> {
	let commit =
		crate::workspace::worktree::resolve_commit(root, "HEAD").await?;
	let tree = crate::project::repository::git(
		root,
		&["rev-parse", "--verify", "HEAD^{tree}"],
	)
	.await?;
	if !tree.status.success() {
		return Err(crate::checkpoint::change_artifact::failed(tree.stderr));
	}
	// This is explicitly incomplete, never a captured current working tree or a
	// fork source. Keep files in place and report metadata without staging bytes.
	Ok(ChangeSnapshot {
		commit,
		tree: tree.stdout.trim().to_owned(),
		omitted_files: crate::checkpoint::omissions::inspect(root, 0).await?,
		uncommitted: artifact(),
	})
}

pub(crate) fn artifact() -> ChangeArtifact {
	ChangeArtifact {
		sha256:
			"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
				.into(),
		size: 0,
		availability: ArtifactAvailability::DiskPressure,
	}
}

pub(crate) fn incomplete(snapshot: &ChangeSnapshot) -> bool {
	snapshot.uncommitted.availability == ArtifactAvailability::DiskPressure
}

pub(crate) async fn capture_bytes(root: &Path) -> Result<u64, CoreError> {
	let output = crate::project::repository::git(
		root,
		&[
			"ls-files",
			"--modified",
			"--others",
			"--exclude-standard",
			"-z",
		],
	)
	.await?;
	if !output.status.success() {
		return Err(crate::checkpoint::change_artifact::failed(output.stderr));
	}
	let paths: std::collections::HashSet<_> =
		output.stdout.split('\0').collect();
	let files = crate::checkpoint::omissions::inspect(root, 0).await?;
	// Budget uncompressed content plus object/index overhead before invoking Git.
	Ok(files
		.iter()
		.filter(|file| paths.contains(file.path.as_str()))
		.fold(1024 * 1024u64, |bytes, file| {
			bytes.saturating_add(file.size.saturating_mul(2))
		}))
}
