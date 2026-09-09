//! Reserve newly ingested bytes durably before publishing an Artifact.
use crate::{
	ArtifactAvailability, CoreError, RunId,
	change_artifact::{Pending, failed},
};
use rustix::fs::{Mode, OFlags, openat};
use std::{
	fs::File,
	io::{Read, Write},
};

pub(crate) fn publish_with_limit(
	pending: Pending,
	run_id: RunId,
	target: &str,
	size: u64,
	run_limit: u64,
	budget: File,
) -> Result<ArtifactAvailability, CoreError> {
	let directory = &pending.directory;
	// One descriptor lock covers deduplication, reservation and publication across
	// concurrent queries and daemon instances. No database lock is acquired here.
	let lock: File = openat(
		&budget,
		".ingest-lock",
		OFlags::RDWR
			| OFlags::CREATE
			| OFlags::NOFOLLOW
			| OFlags::NONBLOCK
			| OFlags::CLOEXEC,
		Mode::from_raw_mode(0o600),
	)
	.map_err(failed)?
	.into();
	if !lock.metadata().map_err(failed)?.is_file() {
		return Err(failed("invalid Artifact lock"));
	}
	lock.lock().map_err(failed)?;
	match openat(
		directory,
		target,
		OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
		Mode::empty(),
	) {
		Ok(file) => {
			crate::artifact_files::verify(
				&mut File::from(file),
				&crate::ArtifactDescriptor {
					sha256: target.into(),
					size,
				},
			)?;
			return Ok(ArtifactAvailability::Stored);
		}
		Err(rustix::io::Errno::NOENT) => {}
		Err(error) => return Err(failed(error)),
	}
	let name = format!(".run-{}.budget", run_id.0);
	let used = match openat(
		&budget,
		name.as_str(),
		OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
		Mode::empty(),
	) {
		Ok(file) => {
			let mut file = File::from(file);
			let metadata = file.metadata().map_err(failed)?;
			if !metadata.is_file() || metadata.len() != 8 {
				return Err(failed("invalid Artifact budget"));
			}
			let mut bytes = [0; 8];
			file.read_exact(&mut bytes).map_err(failed)?;
			u64::from_be_bytes(bytes)
		}
		Err(rustix::io::Errno::NOENT) => 0,
		Err(error) => return Err(failed(error)),
	};
	let Some(total) =
		used.checked_add(size).filter(|total| *total <= run_limit)
	else {
		return Ok(ArtifactAvailability::RunBudgetExceeded);
	};
	let reservation = Pending {
		directory: budget.try_clone().map_err(failed)?,
		name: format!(".pending-{}", uuid::Uuid::new_v4()),
	};
	let mut file: File = openat(
		&budget,
		reservation.name.as_str(),
		OFlags::WRONLY
			| OFlags::CREATE
			| OFlags::EXCL
			| OFlags::NOFOLLOW
			| OFlags::CLOEXEC,
		Mode::from_raw_mode(0o600),
	)
	.map_err(failed)?
	.into();
	file.write_all(&total.to_be_bytes()).map_err(failed)?;
	file.sync_all().map_err(failed)?;
	rustix::fs::renameat(
		&budget,
		reservation.name.as_str(),
		&budget,
		name.as_str(),
	)
	.map_err(failed)?;
	budget.sync_all().map_err(failed)?;
	// A crash after reservation can overcount, never replenish an exhausted
	// budget. Existing hashes cost no new bytes; failed temporaries are unlinked.
	rustix::fs::renameat(directory, pending.name.as_str(), directory, target)
		.map_err(failed)?;
	directory.sync_all().map_err(failed)?;
	Ok(ArtifactAvailability::Stored)
}
