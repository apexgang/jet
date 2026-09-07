//! Git/content inspection; filesystem notification times never imply origin.
use crate::{
	ChangeArtifact, ChangeOrigin, ChangeSnapshot, ChangedFile, Core, CoreError,
	RunId,
};
use crate::{change_artifact, repository, tree_capture, workspace, worktree};
use std::path::Path;

pub(crate) enum Retention {
	Durable,
	Current,
}

pub(crate) async fn snapshot(
	core: &Core,
	root: &Path,
	run_id: RunId,
	retention: Retention,
) -> Result<ChangeSnapshot, CoreError> {
	tokio::time::timeout(
		std::time::Duration::from_secs(300),
		workspace::with_scratch(
			&core.workspace_home,
			"checkpoint",
			async |scratch| {
				if crate::filesystem::canonicalize(root.to_path_buf())
					.await
					.map_err(change_artifact::failed)?
					!= root || repository::verdict(root).await?
					!= repository::Verdict::Registrable
				{
					return Err(change_artifact::failed(
						"the registered root changed",
					));
				}
				let commit = worktree::resolve_commit(root, "HEAD").await?;
				let index_path = scratch.join("index");
				let index = tree_capture::ScratchIndex::new(
					root,
					&index_path,
					change_artifact::failed,
				);
				index.copy_from_checkout().await?;
				let omitted_files =
					crate::checkpoint_omissions::inspect(root).await?;
				let excluded: Vec<_> = omitted_files
					.iter()
					.map(|file| file.path.as_str())
					.collect();
				let (tree, _) =
					index.capture_excluding(&commit, &excluded).await?;
				if worktree::resolve_commit(root, "HEAD").await? != commit {
					return Err(change_artifact::failed(
						"HEAD changed during capture",
					));
				}
				let uncommitted =
					patch(core, root, run_id, &commit, &tree).await?;
				// Keep rewritten commits and uncommitted tree objects reachable across
				// restart and Git GC. These refs create no commits or user-branch changes.
				let prefix = format!("refs/jet/checkpoints/{}", run_id.0);
				if matches!(retention, Retention::Durable) {
					for (suffix, object) in
						[("commit", &commit), ("tree", &tree)]
					{
						let name = format!("{prefix}/{suffix}/{object}");
						let output = repository::git(
							root,
							&[
								"-c",
								"core.fsync=loose-object,reference",
								"update-ref",
								&name,
								object,
							],
						)
						.await?;
						if !output.status.success() {
							return Err(change_artifact::failed(output.stderr));
						}
					}
				}
				Ok(ChangeSnapshot {
					omitted_files,
					commit,
					tree,
					uncommitted,
				})
			},
		),
	)
	.await
	.map_err(change_artifact::failed)?
}
pub(crate) async fn patch(
	core: &Core,
	root: &Path,
	run_id: RunId,
	before: &str,
	after: &str,
) -> Result<ChangeArtifact, CoreError> {
	let mut command = repository::command(root);
	// ASVS 1.2.5: fixed flags, immutable object names, no shell or external
	// diff/textconv programs. Binary patches also preserve arbitrary file bytes.
	command.args([
		"diff",
		"--binary",
		"--full-index",
		"--no-renames",
		"--no-ext-diff",
		"--no-textconv",
		"--src-prefix=a/",
		"--dst-prefix=b/",
		"--end-of-options",
		before,
		after,
		"--",
	]);
	change_artifact::publish(core.run_home(), run_id, command).await
}
pub(crate) async fn files(
	root: &Path,
	before: &ChangeSnapshot,
	after: &ChangeSnapshot,
) -> Result<Vec<ChangedFile>, CoreError> {
	let mut files = tree_capture::diff_trees(
		root,
		&before.tree,
		&after.tree,
		change_artifact::failed,
	)
	.await?
	.into_iter()
	.map(|change| ChangedFile {
		path: change.path,
		before_object: Some(change.source_object),
		before_size: None,
		after_size: None,
		after_object: Some(change.destination_object),
		before_mode: change.source_mode,
		after_mode: change.destination_mode,
		origin: ChangeOrigin::ExternalOrUnknown,
	})
	.collect();
	crate::checkpoint_omissions::include(root, &mut files, before, after)
		.await?;
	Ok(files)
}
