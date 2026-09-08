//! Grace-period collection for unreferenced third-party Craft Artifacts.

use std::collections::HashSet;
use std::ffi::CStr;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use jet_store::Store;
use rustix::fs::{AtFlags, Dir, Mode, OFlags, open, openat, unlinkat};

use crate::craft_installation::{InstallationManifest, InstallationPlan};
use crate::{CoreError, craft_publication};

const UNREFERENCED_GRACE: Duration = Duration::from_secs(24 * 60 * 60);
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::DIRECTORY)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);

pub(crate) async fn collect_unreferenced(
	store: &Store,
	home: PathBuf,
	now: SystemTime,
) -> Result<(), CoreError> {
	let plans = store
		.read(async |tx| tx.craft_installation_plans().await)
		.await?;
	let mut referenced = HashSet::new();
	for encoded in plans {
		let Ok(plan) = serde_json::from_str::<InstallationPlan>(&encoded)
		else {
			return Ok(());
		};
		referenced.insert(plan.artifact_sha256);
	}
	crate::filesystem::blocking(move || collect(&home, &mut referenced, now))
		.await?
		.map_err(|_| craft_publication::installation_failed())
}

fn collect(
	home: &Path,
	referenced: &mut HashSet<String>,
	now: SystemTime,
) -> std::io::Result<()> {
	let Some(home) = open_directory(home)? else {
		return Ok(());
	};
	for entry in Dir::read_from(&home)? {
		let entry = entry?;
		let name = entry.file_name();
		if !name.to_bytes().ends_with(b".json") {
			continue;
		}
		let Some(file) = open_regular(&home, name)? else {
			continue;
		};
		let Ok(manifest) = read_manifest(file) else {
			// A manifest this version cannot interpret may still hold the only
			// reference to an Artifact, so fail closed by collecting nothing.
			return Ok(());
		};
		referenced.insert(manifest.sha256);
	}
	if let Some(artifacts) = open_child_directory(&home, "artifacts")? {
		collect_artifacts(&artifacts, referenced, now)?;
	}
	if let Some(staging) = open_child_directory(&home, "staging")? {
		collect_staging(&staging, now)?;
	}
	Ok(())
}

fn collect_artifacts(
	directory: &File,
	referenced: &HashSet<String>,
	now: SystemTime,
) -> std::io::Result<()> {
	let mut removed = false;
	for entry in Dir::read_from(directory)? {
		let entry = entry?;
		let name = entry.file_name();
		let Ok(name_text) = std::str::from_utf8(name.to_bytes()) else {
			continue;
		};
		if !crate::craft_specification::is_sha256(name_text)
			|| referenced.contains(name_text)
		{
			continue;
		}
		let Some(file) = open_regular(directory, name)? else {
			continue;
		};
		if old_enough(&file, now)? {
			unlinkat(directory, name, AtFlags::empty())?;
			removed = true;
		}
	}
	if removed {
		directory.sync_all()?;
	}
	Ok(())
}

fn collect_staging(directory: &File, now: SystemTime) -> std::io::Result<()> {
	let mut removed = false;
	for entry in Dir::read_from(directory)? {
		let entry = entry?;
		let name = entry.file_name();
		if !name.to_bytes().ends_with(b".artifact") {
			continue;
		}
		let Some(file) = open_regular(directory, name)? else {
			continue;
		};
		if old_enough(&file, now)? {
			unlinkat(directory, name, AtFlags::empty())?;
			removed = true;
		}
	}
	if removed {
		directory.sync_all()?;
	}
	Ok(())
}

fn open_directory(path: &Path) -> std::io::Result<Option<File>> {
	match open(path, DIRECTORY_FLAGS, Mode::empty()) {
		Ok(directory) => Ok(Some(directory.into())),
		Err(
			rustix::io::Errno::NOENT
			| rustix::io::Errno::NOTDIR
			| rustix::io::Errno::LOOP,
		) => Ok(None),
		Err(error) => Err(error.into()),
	}
}

fn open_child_directory(
	directory: &File,
	name: &str,
) -> std::io::Result<Option<File>> {
	match openat(directory, name, DIRECTORY_FLAGS, Mode::empty()) {
		Ok(child) => Ok(Some(child.into())),
		Err(
			rustix::io::Errno::NOENT
			| rustix::io::Errno::NOTDIR
			| rustix::io::Errno::LOOP,
		) => Ok(None),
		Err(error) => Err(error.into()),
	}
}

fn open_regular(
	directory: &File,
	name: &CStr,
) -> std::io::Result<Option<File>> {
	let flags =
		OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
	match openat(directory, name, flags, Mode::empty()) {
		Ok(file) => {
			let file: File = file.into();
			Ok(file.metadata()?.is_file().then_some(file))
		}
		Err(
			rustix::io::Errno::NOENT
			| rustix::io::Errno::NOTDIR
			| rustix::io::Errno::LOOP,
		) => Ok(None),
		Err(error) => Err(error.into()),
	}
}

fn old_enough(file: &File, now: SystemTime) -> std::io::Result<bool> {
	Ok(now
		.duration_since(file.metadata()?.modified()?)
		.is_ok_and(|age| age >= UNREFERENCED_GRACE))
}

fn read_manifest(file: File) -> std::io::Result<InstallationManifest> {
	if file.metadata()?.len() > 256 * 1024 {
		return Err(std::io::Error::other("invalid Craft manifest"));
	}
	serde_json::from_reader(std::io::Read::take(file, 256 * 1024 + 1))
		.map_err(std::io::Error::other)
}
