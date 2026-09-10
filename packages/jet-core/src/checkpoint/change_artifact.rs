//! Stream patches into owner-only, atomic SHA-256 Artifacts (ADR-0072).
use crate::{ArtifactAvailability, ChangeArtifact, CoreError, filesystem};
use rustix::fs::{Mode, OFlags, open, openat};
use sha2::{Digest, Sha256};
use std::{
	fs::File,
	path::{Path, PathBuf},
	process::Stdio,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) const PREVIEW_LIMIT: usize = 128 * 1024;

pub(crate) struct Pending {
	pub(crate) directory: File,
	pub(crate) name: String,
}
impl Drop for Pending {
	fn drop(&mut self) {
		let _ = rustix::fs::unlinkat(
			&self.directory,
			self.name.as_str(),
			rustix::fs::AtFlags::empty(),
		);
	}
}
pub(crate) fn directory(home: &Path) -> std::io::Result<File> {
	// ASVS 5.3.2, 15.4.2: resolve internal names relative to pinned directory
	// handles; neither an Artifact nor its directory can redirect through a link.
	let flags =
		OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
	let home = open(home, flags, Mode::empty())?;
	match rustix::fs::mkdirat(&home, "artifacts", Mode::from_raw_mode(0o700)) {
		Ok(()) => {
			rustix::fs::fsync(&home)?;
		}
		Err(rustix::io::Errno::EXIST) => {}
		Err(error) => return Err(error.into()),
	}
	Ok(openat(home, "artifacts", flags, Mode::empty())?.into())
}
pub(crate) async fn publish(
	home: PathBuf,
	run_id: crate::RunId,
	mut command: tokio::process::Command,
	limits: crate::ArtifactLimits,
	retention: crate::checkpoint::capture::Retention,
) -> Result<ChangeArtifact, CoreError> {
	let budget_home = home.clone();
	let (pending, file) = filesystem::blocking(move || {
		let directory = match retention {
			crate::checkpoint::capture::Retention::Durable => directory(&home)?,
			crate::checkpoint::capture::Retention::Current => {
				crate::artifact::files::cache_directory(&home)
					.map_err(std::io::Error::other)?
			}
		};
		let name = format!(".pending-{}", uuid::Uuid::new_v4());
		let file: File = openat(
			&directory,
			name.as_str(),
			OFlags::WRONLY
				| OFlags::CREATE
				| OFlags::EXCL
				| OFlags::NOFOLLOW
				| OFlags::CLOEXEC,
			Mode::from_raw_mode(0o600),
		)?
		.into();
		file.lock()?;
		Ok::<_, std::io::Error>((Pending { directory, name }, file))
	})
	.await?
	.map_err(failed)?;
	let mut file = tokio::fs::File::from_std(file);
	let mut child = command
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.map_err(failed)?;
	let mut output = child.stdout.take().expect("piped output");
	let mut hash = Sha256::new();
	let mut size = 0;
	let mut pressure = false;
	let mut chunk = [0; 65536];
	loop {
		let read = output.read(&mut chunk).await.map_err(failed)?;
		if read == 0 {
			break;
		}
		size += read as u64;
		hash.update(&chunk[..read]);
		if size <= limits.artifact_bytes && !pressure {
			let directory = pending.directory.try_clone().map_err(failed)?;
			let space = filesystem::blocking(move || {
				crate::artifact::files::reserve(
					&directory,
					read as u64,
					limits.free_reserve_bytes,
				)
			})
			.await?;
			match space {
				Ok(()) => {
					file.write_all(&chunk[..read]).await.map_err(failed)?
				}
				Err(error) if error.code == "storage.disk_pressure" => {
					pressure = true
				}
				Err(error) => return Err(error),
			}
		}
	}
	if !child.wait().await.map_err(failed)?.success() {
		return Err(failed("Git could not produce the patch"));
	}
	file.sync_all().await.map_err(failed)?;
	drop(file);
	let sha256 = format!("{:x}", hash.finalize());
	let target = sha256.clone();
	let availability = if pressure {
		// Preserve checkpoint metadata and Run recovery even when payload writes pause.
		ArtifactAvailability::DiskPressure
	} else if size > limits.artifact_bytes {
		ArtifactAvailability::ArtifactSizeExceeded
	} else {
		filesystem::blocking(move || {
			let budget = directory(&budget_home).map_err(failed)?;
			crate::checkpoint::change_artifact_budget::publish_with_limit(
				pending,
				run_id,
				&target,
				size,
				limits.run_bytes,
				budget,
			)
		})
		.await??
	};
	Ok(ChangeArtifact {
		sha256,
		size,
		availability,
	})
}
fn open_artifact(home: &Path, sha256: &str) -> Result<File, CoreError> {
	if sha256.len() != 64
		|| !sha256
			.bytes()
			.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
	{
		return Err(failed("invalid Artifact address"));
	}
	let directory = directory(home).map_err(failed)?;
	let flags =
		OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
	let file: File = match openat(&directory, sha256, flags, Mode::empty()) {
		Ok(file) => file.into(),
		Err(rustix::io::Errno::NOENT) => openat(
			&crate::artifact::files::cache_directory(home)?,
			sha256,
			flags,
			Mode::empty(),
		)
		.map_err(failed)?
		.into(),
		Err(error) => return Err(failed(error)),
	};
	let metadata = file.metadata().map_err(failed)?;
	if !metadata.is_file() {
		return Err(failed("invalid Artifact file"));
	}
	Ok(file)
}
pub(crate) async fn read(
	home: PathBuf,
	sha256: String,
	offset: u64,
) -> Result<crate::ChangeArtifactChunk, CoreError> {
	filesystem::blocking(move || {
		use std::io::{Read, Seek, SeekFrom};
		let mut file = open_artifact(&home, &sha256)?;
		let size = file.metadata().map_err(failed)?.len();
		if offset > size {
			return Err(CoreError::invalid_input(
				"checkpoint.invalid_offset",
				"the offset is beyond the Artifact",
			));
		}
		file.seek(SeekFrom::Start(offset)).map_err(failed)?;
		let mut bytes = Vec::new();
		file.take(65536).read_to_end(&mut bytes).map_err(failed)?;
		Ok(crate::ChangeArtifactChunk {
			artifact: ChangeArtifact {
				sha256,
				size,
				availability: ArtifactAvailability::Stored,
			},
			offset,
			bytes,
		})
	})
	.await?
}
pub(crate) async fn preview(
	home: PathBuf,
	artifact: &ChangeArtifact,
) -> Result<String, CoreError> {
	if artifact.availability != ArtifactAvailability::Stored {
		return Ok(String::new());
	}
	let artifact = artifact.clone();
	filesystem::blocking(move || {
		use std::io::Read;
		let file = open_artifact(&home, &artifact.sha256)?;
		if file.metadata().map_err(failed)?.len() != artifact.size {
			return Err(failed("Artifact size changed"));
		}
		let mut bytes = Vec::new();
		file.take(PREVIEW_LIMIT as u64)
			.read_to_end(&mut bytes)
			.map_err(failed)?;
		Ok(String::from_utf8_lossy(&bytes).into_owned())
	})
	.await?
}
pub(crate) fn failed(detail: impl std::fmt::Display) -> CoreError {
	CoreError::unavailable(
		"checkpoint.capture_failed",
		"the Change checkpoint could not be captured",
		detail.to_string().chars().take(512).collect::<String>(),
	)
}
