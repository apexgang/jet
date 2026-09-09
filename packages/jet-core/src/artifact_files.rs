//! Descriptor-relative payload storage. Only verified regular files are published.
use crate::{ArtifactDescriptor, CoreError, change_artifact::Pending};
use rustix::fs::{Mode, OFlags, openat};
use sha2::{Digest, Sha256};
use std::{
	fs::File,
	io::{Read, Seek, SeekFrom},
	path::Path,
};

pub(crate) fn directory(home: &Path) -> Result<File, CoreError> {
	let parent = crate::change_artifact::directory(home).map_err(io_error)?;
	// Keep general payload collection separate from legacy checkpoint/Craft retention.
	match rustix::fs::mkdirat(&parent, "payloads", Mode::from_raw_mode(0o700)) {
		Ok(()) | Err(rustix::io::Errno::EXIST) => {}
		Err(error) => return Err(io_error(error)),
	}
	let directory: File = openat(
		&parent,
		"payloads",
		OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
		Mode::empty(),
	)
	.map_err(io_error)?
	.into();
	parent.sync_all().map_err(io_error)?;
	Ok(directory)
}

pub(crate) fn stage(home: &Path) -> Result<(Pending, File), CoreError> {
	let directory = directory(home)?;
	let name = format!(".pending-{}", uuid::Uuid::new_v4());
	let file: File = openat(
		&directory,
		name.as_str(),
		OFlags::RDWR
			| OFlags::CREATE
			| OFlags::EXCL
			| OFlags::NOFOLLOW
			| OFlags::CLOEXEC,
		Mode::from_raw_mode(0o600),
	)
	.map_err(io_error)?
	.into();
	file.lock().map_err(io_error)?;
	Ok((Pending { directory, name }, file))
}

pub(crate) async fn publication_lock(
	home: std::path::PathBuf,
) -> Result<File, CoreError> {
	crate::filesystem::blocking(move || {
		let directory = directory(&home)?;
		let file: File = openat(
			&directory,
			".publication-lock",
			OFlags::RDWR
				| OFlags::CREATE
				| OFlags::NOFOLLOW
				| OFlags::NONBLOCK
				| OFlags::CLOEXEC,
			Mode::from_raw_mode(0o600),
		)
		.map_err(io_error)?
		.into();
		if !file.metadata().map_err(io_error)?.is_file() {
			return Err(corrupt());
		}
		file.lock().map_err(io_error)?;
		Ok(file)
	})
	.await?
}

pub(crate) fn open(directory: &File, name: &str) -> Result<File, CoreError> {
	let file: File = openat(
		directory,
		name,
		OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
		Mode::empty(),
	)
	.map_err(|error| {
		if error == rustix::io::Errno::NOENT {
			missing()
		} else {
			corrupt()
		}
	})?
	.into();
	if !file.metadata().map_err(io_error)?.is_file() {
		return Err(corrupt());
	}
	Ok(file)
}

pub(crate) fn verify(
	file: &mut File,
	expected: &ArtifactDescriptor,
) -> Result<(), CoreError> {
	if file.metadata().map_err(io_error)?.len() != expected.size {
		return Err(corrupt());
	}
	file.seek(SeekFrom::Start(0)).map_err(io_error)?;
	let mut hash = Sha256::new();
	let mut buffer = [0; 65536];
	let mut remaining = expected.size;
	while remaining > 0 {
		let limit = remaining.min(buffer.len() as u64) as usize;
		let read = file.read(&mut buffer[..limit]).map_err(io_error)?;
		if read == 0 {
			return Err(corrupt());
		}
		hash.update(&buffer[..read]);
		remaining -= read as u64;
	}
	if format!("{:x}", hash.finalize()) != expected.sha256
		|| file.metadata().map_err(io_error)?.len() != expected.size
	{
		return Err(corrupt());
	}
	file.seek(SeekFrom::Start(0)).map_err(io_error)?;
	Ok(())
}

pub(crate) fn reserve(
	file: &File,
	bytes: u64,
	reserve: u64,
) -> Result<(), CoreError> {
	let stat = rustix::fs::fstatvfs(file).map_err(io_error)?;
	let free = stat.f_bavail.saturating_mul(stat.f_frsize);
	if free < reserve || free - reserve < bytes {
		return Err(CoreError::unavailable(
			"artifact.disk_pressure",
			"Artifact ingestion is paused until disk space recovers",
			"free-space reserve",
		));
	}
	Ok(())
}

pub(crate) fn missing() -> CoreError {
	CoreError::not_found(
		"artifact.not_found",
		"the Artifact has no published reference",
	)
}
pub(crate) fn corrupt() -> CoreError {
	CoreError::unavailable(
		"artifact.corrupt",
		"stored Artifact content failed verification",
		"size, type or SHA-256 mismatch",
	)
}
pub(crate) fn io_error(error: impl std::fmt::Display) -> CoreError {
	CoreError::unavailable(
		"artifact.storage_unavailable",
		"Artifact storage is unavailable",
		error.to_string(),
	)
}
