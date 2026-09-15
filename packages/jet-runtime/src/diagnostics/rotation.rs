//! The owner-only file ring one executable role writes its Diagnostic
//! records into: five files of five MiB, the oldest dropped as the newest
//! fills (ADR-0061).

use std::{
	fs::{self, File, OpenOptions},
	io::{self, Write},
	os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
	path::{Path, PathBuf},
};

/// How large the live file may grow before it is rotated, in bytes.
pub(super) const FILE_LIMIT: u64 = 5 * 1024 * 1024;

/// How many files, the live one included, one role keeps.
pub(super) const FILE_COUNT: usize = 5;

#[derive(Debug)]
pub(super) struct Ring {
	directory: PathBuf,
	role: &'static str,
	file: File,
	written: u64,
}

impl Ring {
	/// Opens the live file of `role` under `directory`, creating both as
	/// owner-only when missing and continuing a live file left by an
	/// earlier process.
	pub(super) fn open(
		directory: &Path,
		role: &'static str,
	) -> io::Result<Self> {
		fs::DirBuilder::new()
			.recursive(true)
			.mode(0o700)
			.create(directory)?;
		fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
		let mut ring = Self {
			directory: directory.to_path_buf(),
			role,
			file: open_live(directory, role)?,
			written: 0,
		};
		ring.written = ring.file.metadata()?.len();
		Ok(ring)
	}

	/// Appends one complete line, rotating first when it would not fit.
	pub(super) fn append(&mut self, line: &[u8]) -> io::Result<()> {
		let length = u64::try_from(line.len()).unwrap_or(u64::MAX);
		if self.written > 0 && self.written.saturating_add(length) > FILE_LIMIT
		{
			self.rotate()?;
		}
		self.file.write_all(line)?;
		self.written = self.written.saturating_add(length);
		Ok(())
	}

	fn rotate(&mut self) -> io::Result<()> {
		for generation in (1..FILE_COUNT).rev() {
			let older = self.path(generation);
			if generation + 1 == FILE_COUNT {
				match fs::remove_file(&older) {
					Ok(()) => {}
					Err(error) if error.kind() == io::ErrorKind::NotFound => {}
					Err(error) => return Err(error),
				}
			} else {
				match fs::rename(&older, self.path(generation + 1)) {
					Ok(()) => {}
					Err(error) if error.kind() == io::ErrorKind::NotFound => {}
					Err(error) => return Err(error),
				}
			}
		}
		// A live file somebody unlinked is simply started again.
		match fs::rename(self.path(0), self.path(1)) {
			Ok(()) => {}
			Err(error) if error.kind() == io::ErrorKind::NotFound => {}
			Err(error) => return Err(error),
		}
		self.file = open_live(&self.directory, self.role)?;
		self.written = 0;
		Ok(())
	}

	/// Generation zero is the live file; older generations count up.
	fn path(&self, generation: usize) -> PathBuf {
		self.directory.join(file_name(self.role, generation))
	}
}

pub(super) fn file_name(role: &str, generation: usize) -> String {
	match generation {
		0 => format!("{role}.log"),
		older => format!("{role}.log.{older}"),
	}
}

fn open_live(directory: &Path, role: &str) -> io::Result<File> {
	// ASVS 15.4.2: never follow a link somebody planted at the log's name.
	OpenOptions::new()
		.append(true)
		.create(true)
		.mode(0o600)
		.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
		.open(directory.join(file_name(role, 0)))
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	fn names(directory: &Path) -> Vec<String> {
		let mut names: Vec<String> = fs::read_dir(directory)
			.unwrap()
			.map(|entry| entry.unwrap().file_name().into_string().unwrap())
			.collect();
		names.sort();
		names
	}

	#[test]
	fn the_ring_never_exceeds_five_files_of_five_mib() {
		let dir = tempfile::tempdir().unwrap();
		let mut ring = Ring::open(dir.path(), "jetd").unwrap();
		let line = vec![b'x'; 1024];
		let lines_per_file = usize::try_from(FILE_LIMIT).unwrap() / line.len();
		for _ in 0..lines_per_file * (FILE_COUNT + 2) + 1 {
			ring.append(&line).unwrap();
		}
		assert_eq!(
			names(dir.path()),
			[
				"jetd.log",
				"jetd.log.1",
				"jetd.log.2",
				"jetd.log.3",
				"jetd.log.4"
			]
		);
		let total: u64 = fs::read_dir(dir.path())
			.unwrap()
			.map(|entry| entry.unwrap().metadata().unwrap().len())
			.sum();
		assert!(total <= FILE_LIMIT * FILE_COUNT as u64, "{total}");
		let permissions = fs::metadata(dir.path().join("jetd.log"))
			.unwrap()
			.permissions();
		assert_eq!(permissions.mode() & 0o777, 0o600);
		assert_eq!(
			fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
			0o700
		);
	}

	#[test]
	fn an_unlinked_live_file_is_started_again() {
		let dir = tempfile::tempdir().unwrap();
		let mut ring = Ring::open(dir.path(), "jetd").unwrap();
		ring.append(&vec![b'x'; 1024]).unwrap();
		fs::remove_file(dir.path().join("jetd.log")).unwrap();
		ring.written = FILE_LIMIT;
		ring.append(b"again\n").unwrap();
		assert_eq!(
			fs::read_to_string(dir.path().join("jetd.log")).unwrap(),
			"again\n"
		);
	}

	#[test]
	fn a_reopened_ring_continues_the_live_file() {
		let dir = tempfile::tempdir().unwrap();
		Ring::open(dir.path(), "jetd")
			.unwrap()
			.append(b"one\n")
			.unwrap();
		Ring::open(dir.path(), "jetd")
			.unwrap()
			.append(b"two\n")
			.unwrap();
		assert_eq!(
			fs::read_to_string(dir.path().join("jetd.log")).unwrap(),
			"one\ntwo\n"
		);
	}
}
