//! Content identities for native package files, without a Jet package format.
use crate::extension_error;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
	io::{self, Read},
	path::Path,
};

/// A native package file disclosed before executable trust is granted.
#[derive(Serialize)]
pub struct ExtensionFile {
	/// Relative native path.
	pub path: String,
	/// Exact content identity.
	pub sha256: String,
}
/// Inspect bounded native files without following symlinks or executing content.
/// # Errors
/// Rejects special files, symlinks, non-UTF8 names, and packages over 16 MiB/512 files.
pub fn extension_files(root: &Path) -> io::Result<Vec<ExtensionFile>> {
	if !root.is_absolute()
		|| !root.is_dir()
		|| root.symlink_metadata()?.file_type().is_symlink()
	{
		return Err(extension_error());
	}
	let mut remaining = 16 * 1024 * 1024;
	let mut files = Vec::new();
	walk(root, root, &mut files, &mut remaining, &mut 1024, 0)?;
	files.sort_by(|a, b| a.path.cmp(&b.path));
	Ok(files)
}
fn walk(
	root: &Path,
	dir: &Path,
	files: &mut Vec<ExtensionFile>,
	remaining: &mut u64,
	entries: &mut u32,
	depth: u32,
) -> io::Result<()> {
	if depth > 16 {
		return Err(extension_error());
	}
	for entry in std::fs::read_dir(dir)? {
		*entries = entries.checked_sub(1).ok_or_else(extension_error)?;
		let entry = entry?;
		if entry.file_name() == ".git" {
			continue;
		}
		let path = entry.path();
		let kind = entry.file_type()?;
		if kind.is_dir() {
			walk(root, &path, files, remaining, entries, depth + 1)?;
		} else if kind.is_file() {
			if files.len() >= 512 {
				return Err(extension_error());
			}
			// ASVS 2.2.1: stream under one aggregate budget before retaining metadata.
			let mut file =
				crate::native_config::file(&path)?.take(*remaining + 1);
			let mut digest = Sha256::new();
			let count = io::copy(&mut file, &mut digest)?;
			*remaining =
				remaining.checked_sub(count).ok_or_else(extension_error)?;
			files.push(ExtensionFile {
				path: path
					.strip_prefix(root)
					.map_err(|_| extension_error())?
					.to_str()
					.ok_or_else(extension_error)?
					.into(),
				sha256: format!("{:x}", digest.finalize()),
			});
		} else {
			return Err(extension_error());
		}
	}
	Ok(())
}

/// Same-user host authority disclosed separately from Jet broker permissions.
/// Native extensions have no portable OS containment and cannot customize the GUI.
pub fn extension_host_access() -> serde_json::Value {
	serde_json::json!({"same_user_execution":true,"jet_broker":[],"executable_gui":false,
        "executables":"native extension commands", "filesystem":"all paths accessible to the Harness user",
        "environment":"Harness process environment", "network":"destinations reachable by the Harness user"})
}
/// Mutations require a reviewable native file inventory, including for remote sources.
/// Native catalogs that cannot expose their files remain discoverable but cannot mutate.
pub fn extension_review_complete(metadata: &str) -> bool {
	serde_json::from_str::<serde_json::Value>(metadata)
		.ok()
		.is_some_and(|value| {
			value["files"]
				.as_array()
				.is_some_and(|files| !files.is_empty())
		})
}

/// Hash one bounded native configuration/manifest file for a consent snapshot.
/// # Errors
/// Rejects symlinks, special files and content over 64 KiB.
pub fn extension_file_identity(path: &Path) -> io::Result<serde_json::Value> {
	let bytes = crate::native_config::read(path)?;
	Ok(
		serde_json::json!({"path":path,"sha256":crate::native_config::digest(&bytes)}),
	)
}
