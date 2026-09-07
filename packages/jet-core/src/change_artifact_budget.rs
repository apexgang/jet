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

const RUN_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

pub(crate) fn publish(
	pending: Pending,
	run_id: RunId,
	target: &str,
	size: u64,
) -> Result<ArtifactAvailability, CoreError> {
	let directory = &pending.directory;
	// One descriptor lock covers deduplication, reservation and publication across
	// concurrent queries and daemon instances. No database lock is acquired here.
	let lock: File = openat(
		directory,
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
			let metadata = File::from(file).metadata().map_err(failed)?;
			if metadata.is_file() && metadata.len() == size {
				return Ok(ArtifactAvailability::Stored);
			}
			return Err(failed("existing Artifact size changed"));
		}
		Err(rustix::io::Errno::NOENT) => {}
		Err(error) => return Err(failed(error)),
	}
	let name = format!(".run-{}.budget", run_id.0);
	let used = match openat(
		directory,
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
		used.checked_add(size).filter(|total| *total <= RUN_LIMIT)
	else {
		return Ok(ArtifactAvailability::RunBudgetExceeded);
	};
	let reservation = Pending {
		directory: directory.try_clone().map_err(failed)?,
		name: format!(".pending-{}", uuid::Uuid::new_v4()),
	};
	let mut file: File = openat(
		directory,
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
		directory,
		reservation.name.as_str(),
		directory,
		name.as_str(),
	)
	.map_err(failed)?;
	directory.sync_all().map_err(failed)?;
	// A crash after reservation can overcount, never replenish an exhausted
	// budget. Existing hashes cost no new bytes; failed temporaries are unlinked.
	rustix::fs::renameat(directory, pending.name.as_str(), directory, target)
		.map_err(failed)?;
	directory.sync_all().map_err(failed)?;
	Ok(ArtifactAvailability::Stored)
}
