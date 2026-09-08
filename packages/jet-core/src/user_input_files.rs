//! Bounded filesystem operations for authenticated direct edits.

use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use jet_store::ReadTransaction;

use crate::CoreError;
use crate::filesystem::blocking;
use crate::relative_path::{GrantedRoot, RelativePath};
use crate::user_input::{FileRevision, FileTarget, MAX_EDIT_BYTES};

#[derive(Clone)]
pub(crate) struct Observed {
	pub(crate) content: Option<String>,
	pub(crate) revision: FileRevision,
	pub(crate) permission_mode: Option<u32>,
}

pub(crate) async fn root(
	tx: &mut ReadTransaction,
	target: FileTarget,
) -> Result<PathBuf, CoreError> {
	let root = match target {
		FileTarget::Project { project_id } => tx
			.project(project_id.0)
			.await?
			.map(|project| project.root)
			.ok_or_else(|| {
				CoreError::not_found(
					"project.not_found",
					"the Project does not exist",
				)
			})?,
		FileTarget::Workspace { workspace_id } => tx
			.workspace(workspace_id.0)
			.await?
			.map(|workspace| workspace.root)
			.ok_or_else(|| {
				CoreError::not_found(
					"workspace.not_found",
					"the Workspace does not exist",
				)
			})?,
	};
	Ok(PathBuf::from(root))
}

pub(crate) async fn observe(
	root: PathBuf,
	path: RelativePath,
) -> Result<Observed, CoreError> {
	let (content, permissions) = blocking({
		let root = root.clone();
		let path = path.clone();
		move || read_content(&root, &path)
	})
	.await??;
	let mode = permissions.as_ref().map_or("000000", |permissions| {
		if *permissions & 0o111 == 0 {
			"100644"
		} else {
			"100755"
		}
	});
	let revision = revision(&root, &path, content.as_deref(), mode).await?;
	Ok(Observed {
		content,
		revision,
		permission_mode: permissions,
	})
}

fn read_content(
	root: &Path,
	path: &RelativePath,
) -> Result<(Option<String>, Option<u32>), CoreError> {
	let granted = GrantedRoot::verify(root)?;
	let resolved = path.resolve_within(&granted)?;
	if resolved != root.join(path.as_str()) {
		return Err(CoreError::invalid_input(
			"user_edit.symbolic_link",
			"an editable path does not pass through a symbolic link",
		));
	}
	match std::fs::metadata(&resolved) {
		Ok(metadata) if metadata.is_file() => {
			if metadata.len() > MAX_EDIT_BYTES as u64 {
				return Err(too_large());
			}
			let bytes = std::fs::read(&resolved)
				.map_err(|error| file_unavailable("read_failed", error))?;
			if bytes.len() > MAX_EDIT_BYTES {
				return Err(too_large());
			}
			let content = String::from_utf8(bytes).map_err(|_| {
				CoreError::invalid_input(
					"user_edit.not_utf8",
					"editable content must be valid UTF-8",
				)
			})?;
			Ok((Some(content), Some(metadata.permissions().mode())))
		}
		Ok(_) => Err(CoreError::invalid_input(
			"user_edit.not_file",
			"an editable path names a regular file or a missing file",
		)),
		Err(error)
			if matches!(
				error.kind(),
				std::io::ErrorKind::NotFound
					| std::io::ErrorKind::NotADirectory
			) =>
		{
			Ok((None, None))
		}
		Err(error) => Err(file_unavailable("read_failed", error)),
	}
}

pub(crate) async fn revision(
	root: &Path,
	path: &RelativePath,
	content: Option<&str>,
	mode: &str,
) -> Result<FileRevision, CoreError> {
	let argument = format!("--path={}", path.as_str());
	// ASVS 1.2.5, 5.3.8: the path is one argument to Git, never shell text.
	let output = crate::repository::git_with_input(
		root,
		&["hash-object", &argument, "--stdin"],
		content.unwrap_or_default().as_bytes().to_vec(),
	)
	.await?;
	if !output.status.success() {
		return Err(CoreError::unavailable(
			"user_edit.hash_failed",
			"the file Revision could not be computed",
			output.stderr,
		));
	}
	let object = output.stdout.trim();
	if !matches!(object.len(), 40 | 64)
		|| !object
			.bytes()
			.all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
	{
		return Err(CoreError::internal(
			"user_edit.invalid_object",
			"Git returned an invalid object identity",
		));
	}
	Ok(FileRevision {
		object: if content.is_some() {
			object.into()
		} else {
			"0".repeat(object.len())
		},
		mode: mode.into(),
	})
}

pub(crate) fn valid_revision(revision: &FileRevision) -> bool {
	let object_valid = matches!(revision.object.len(), 40 | 64)
		&& revision
			.object
			.bytes()
			.all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
	let missing = revision.object.bytes().all(|byte| byte == b'0');
	object_valid
		&& if missing {
			revision.mode == "000000"
		} else {
			matches!(revision.mode.as_str(), "100644" | "100755")
		}
}

pub(crate) async fn write_atomic(
	root: PathBuf,
	path: RelativePath,
	content: String,
	permission_mode: Option<u32>,
) -> Result<(), CoreError> {
	blocking(move || replace_atomic(root, path, content, permission_mode))
		.await?
}

pub(crate) fn replace_atomic(
	root: PathBuf,
	path: RelativePath,
	content: String,
	permission_mode: Option<u32>,
) -> Result<(), CoreError> {
	let granted = GrantedRoot::verify(&root)?;
	let destination = path.resolve_within(&granted)?;
	if destination != root.join(path.as_str()) {
		return Err(CoreError::invalid_input(
			"user_edit.symbolic_link",
			"an editable path does not pass through a symbolic link",
		));
	}
	let Some(parent) = destination.parent() else {
		return Err(CoreError::internal(
			"user_edit.invalid_destination",
			"an editable path had no parent",
		));
	};
	if !parent.is_dir() {
		return Err(CoreError::invalid_input(
			"user_edit.parent_missing",
			"the edited file's parent directory must already exist",
		));
	}
	let temporary =
		parent.join(format!(".jet-edit-{}.tmp", uuid::Uuid::now_v7()));
	let result = (|| -> std::io::Result<()> {
		let mut file = std::fs::OpenOptions::new()
			.create_new(true)
			.write(true)
			.mode(0o600)
			.open(&temporary)?;
		file.write_all(content.as_bytes())?;
		file.set_permissions(std::fs::Permissions::from_mode(
			permission_mode.unwrap_or(0o644),
		))?;
		file.sync_all()?;
		std::fs::rename(&temporary, &destination)?;
		std::fs::File::open(parent)?.sync_all()
	})();
	if let Err(error) = result {
		let _ = std::fs::remove_file(&temporary);
		return Err(file_unavailable("write_failed", error));
	}
	Ok(())
}

fn too_large() -> CoreError {
	CoreError::invalid_input(
		"user_edit.too_large",
		"editable content must be at most 131072 UTF-8 bytes",
	)
}

fn file_unavailable(code: &'static str, error: std::io::Error) -> CoreError {
	// ASVS 16.5.1: native paths and OS detail remain local diagnostics.
	CoreError::unavailable(
		match code {
			"read_failed" => "user_edit.read_failed",
			_ => "user_edit.write_failed",
		},
		"the editable file could not be accessed on this Plane",
		error.to_string(),
	)
}
