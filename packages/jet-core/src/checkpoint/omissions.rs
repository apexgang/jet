//! Oversized Workspace files keep metadata instead of entering the object store.
use crate::{
	ChangeOrigin, ChangedFile, CoreError, OmittedFile,
	checkpoint::change_artifact, project::repository,
};
use rustix::fs::{Mode, OFlags, open, openat};
use std::{fs::File, os::unix::fs::PermissionsExt, path::Path};

pub(crate) async fn inspect(
	root: &Path,
	artifact_limit: u64,
) -> Result<Vec<OmittedFile>, CoreError> {
	let output = repository::git(
		root,
		&[
			"ls-files",
			"--cached",
			"--others",
			"--exclude-standard",
			"-z",
		],
	)
	.await?;
	if !output.status.success() {
		return Err(change_artifact::failed(output.stderr));
	}
	let root = root.to_path_buf();
	crate::filesystem::blocking(move || {
		let flags = OFlags::RDONLY
			| OFlags::DIRECTORY
			| OFlags::NOFOLLOW
			| OFlags::CLOEXEC;
		let root: File = open(&root, flags, Mode::empty())
			.map_err(change_artifact::failed)?
			.into();
		let mut omitted = Vec::new();
		let paths: std::collections::BTreeSet<_> = output
			.stdout
			.split('\0')
			.filter(|path| !path.is_empty() && !path.ends_with('/'))
			.collect();
		for path in paths {
			crate::RelativePath::parse(path)?;
			if let Some(metadata) =
				metadata(&root, path).map_err(change_artifact::failed)?
				&& metadata.is_file()
				&& metadata.len() > artifact_limit
			{
				omitted.push(OmittedFile {
					path: path.to_owned(),
					size: metadata.len(),
					mode: if metadata.permissions().mode() & 0o111 == 0 {
						"100644"
					} else {
						"100755"
					}
					.into(),
				});
			}
		}
		Ok(omitted)
	})
	.await?
}

fn metadata(
	root: &File,
	path: &str,
) -> std::io::Result<Option<std::fs::Metadata>> {
	// ASVS 5.3.2: descriptor-relative traversal never follows a replaced parent
	// or leaf symlink. Metadata inspection never reads the oversized contents.
	let mut directory = root.try_clone()?;
	let parts: Vec<_> = path.split('/').collect();
	for (index, part) in parts.iter().enumerate() {
		let mut flags = OFlags::RDONLY
			| OFlags::NOFOLLOW
			| OFlags::NONBLOCK
			| OFlags::CLOEXEC;
		if index + 1 < parts.len() {
			flags |= OFlags::DIRECTORY;
		}
		match openat(&directory, *part, flags, Mode::empty()) {
			Ok(file) => directory = file.into(),
			Err(
				rustix::io::Errno::NOENT
				| rustix::io::Errno::NOTDIR
				| rustix::io::Errno::LOOP,
			) => return Ok(None),
			Err(error) => return Err(error.into()),
		}
	}
	Ok(Some(directory.metadata()?))
}

pub(crate) async fn include(
	root: &Path,
	files: &mut Vec<ChangedFile>,
	before: &crate::ChangeSnapshot,
	after: &crate::ChangeSnapshot,
) -> Result<(), CoreError> {
	let paths: std::collections::BTreeSet<_> = before
		.omitted_files
		.iter()
		.chain(&after.omitted_files)
		.map(|file| file.path.as_str())
		.collect();
	files.retain(|file| !paths.contains(file.path.as_str()));
	for path in paths {
		let (before_object, before_mode, before_size) =
			side(root, before, path).await?;
		let (after_object, after_mode, after_size) =
			side(root, after, path).await?;
		files.push(ChangedFile {
			path: path.to_owned(),
			before_object,
			after_object,
			before_size,
			after_size,
			before_mode,
			after_mode,
			origin: ChangeOrigin::ExternalOrUnknown,
		});
	}
	Ok(())
}

async fn side(
	root: &Path,
	snapshot: &crate::ChangeSnapshot,
	path: &str,
) -> Result<(Option<String>, String, Option<u64>), CoreError> {
	if let Some(file) =
		snapshot.omitted_files.iter().find(|file| file.path == path)
	{
		return Ok((None, file.mode.clone(), Some(file.size)));
	}
	// A path absent from omissions still has captured content. Look it up even
	// when restoring HEAD for the other side produced no raw Git diff entry.
	let output = repository::git(
		root,
		&[
			"--literal-pathspecs",
			"ls-tree",
			"-z",
			&snapshot.tree,
			"--",
			path,
		],
	)
	.await?;
	if !output.status.success() {
		return Err(change_artifact::failed(output.stderr));
	}
	if output.stdout.is_empty() {
		return Ok((
			Some("0".repeat(snapshot.commit.len())),
			"000000".into(),
			None,
		));
	}
	let header = output
		.stdout
		.split_once('\t')
		.ok_or_else(|| change_artifact::failed("invalid tree metadata"))?
		.0;
	let mut fields = header.split(' ');
	let (Some(mode), Some(_kind), Some(object)) =
		(fields.next(), fields.next(), fields.next())
	else {
		return Err(change_artifact::failed("invalid tree metadata"));
	};
	Ok((Some(object.into()), mode.into(), None))
}
