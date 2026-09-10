//! Verified payload ingestion followed by transactional Run/Event publication.
use crate::{
	Actor, ArtifactAvailability, ClientId, Core, CoreError, RunId,
	artifact_files as files, filesystem,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Content identity independent of transport or local storage layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactDescriptor {
	/// Canonical lowercase SHA-256 content address.
	pub sha256: String,
	/// Exact uncompressed byte count.
	pub size: u64,
}

/// User-configurable ingestion bounds; free-space reserves always take priority.
#[derive(Debug, Clone, Copy)]
pub struct ArtifactLimits {
	/// Maximum size of one payload, default 512 MiB.
	pub artifact_bytes: u64,
	/// Newly ingested bytes per Run, default 2 GiB.
	pub run_bytes: u64,
	/// Additional reserve floor, always at least 2 GiB or 5% of capacity.
	pub free_reserve_bytes: u64,
}
impl Default for ArtifactLimits {
	fn default() -> Self {
		Self {
			artifact_bytes: 512 * 1024 * 1024,
			run_bytes: 2 * 1024 * 1024 * 1024,
			free_reserve_bytes: crate::disk_pressure::MINIMUM_RESERVE,
		}
	}
}

/// Private, unreferenced ingestion. Dropping it discards the temporary file.
pub struct ArtifactUpload {
	pending: crate::change_artifact::Pending,
	file: tokio::fs::File,
	descriptor: ArtifactDescriptor,
	run_id: RunId,
	client_id: ClientId,
	home: std::path::PathBuf,
	received: u64,
	hash: Sha256,
	failed: bool,
	free_reserve_bytes: u64,
}
impl ArtifactUpload {
	/// Writes a nonempty chunk of at most 256 KiB within the declared size.
	/// A rejected chunk permanently invalidates this upload.
	pub async fn write_chunk(&mut self, bytes: &[u8]) -> Result<(), CoreError> {
		if self.failed
			|| bytes.is_empty()
			|| bytes.len() > 256 * 1024
			|| bytes.len() as u64
				> self.descriptor.size.saturating_sub(self.received)
		{
			self.failed = true;
			return Err(invalid(
				"artifact.invalid_chunk",
				"the chunk exceeds the declared size or chunk limit",
			));
		}
		// ASVS 5.2.1, 11.4.3: stream bounded bytes and hash exactly what is written.
		self.failed = true;
		let directory = self
			.pending
			.directory
			.try_clone()
			.map_err(files::io_error)?;
		let count = bytes.len() as u64;
		let reserve = self.free_reserve_bytes;
		filesystem::blocking(move || {
			files::reserve(&directory, count, reserve)
		})
		.await??;
		self.file.write_all(bytes).await.map_err(files::io_error)?;
		self.hash.update(bytes);
		self.received += bytes.len() as u64;
		self.failed = false;
		Ok(())
	}
}

/// A bounded reader that verifies content again before reporting completion.
#[derive(Debug)]
pub struct ArtifactDownload {
	file: tokio::fs::File,
	descriptor: ArtifactDescriptor,
	received: u64,
	hash: Sha256,
}
impl ArtifactDownload {
	/// Declaration that must precede every download's bytes.
	pub fn descriptor(&self) -> &ArtifactDescriptor {
		&self.descriptor
	}
	/// Reads within the supplied byte credit and 256 KiB chunk bound.
	/// Returns a storage or integrity error rather than completing corrupt data.
	pub async fn read_chunk(
		&mut self,
		credit: usize,
	) -> Result<Vec<u8>, CoreError> {
		if credit == 0 {
			return Ok(vec![]);
		}
		let limit = credit
			.min(256 * 1024)
			.min(self.descriptor.size.saturating_sub(self.received) as usize);
		let mut bytes = vec![0; limit];
		let read = self.file.read(&mut bytes).await.map_err(files::io_error)?;
		if read == 0 && self.received != self.descriptor.size {
			return Err(files::corrupt());
		}
		bytes.truncate(read);
		self.hash.update(&bytes);
		self.received += read as u64;
		Ok(bytes)
	}
	/// Completes only after the declared size and SHA-256 match.
	/// Returns an integrity error on truncation, incomplete reads, or corruption.
	pub fn finish(self) -> Result<(), CoreError> {
		if self.received != self.descriptor.size
			|| format!("{:x}", self.hash.finalize()) != self.descriptor.sha256
		{
			return Err(files::corrupt());
		}
		Ok(())
	}
}

impl Core {
	/// Sets ingestion policy before this Core begins serving requests.
	pub fn with_artifact_limits(mut self, limits: ArtifactLimits) -> Self {
		self.artifact_limits = limits;
		self
	}

	/// Opens private staging for an authenticated Run, without exposing a reference.
	/// Returns authorization, declaration, Run lookup, or storage errors.
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "publication lock serializes durable staging reservations with collection and publication"
	)]
	pub async fn begin_artifact_upload(
		&self,
		actor: &Actor,
		run_id: RunId,
		descriptor: ArtifactDescriptor,
	) -> Result<ArtifactUpload, CoreError> {
		let _access =
			self.remote_access.acquire().await.expect("authority gate");
		actor.authorize(&self.remote_sessions)?;
		validate_hash(&descriptor.sha256)?;
		let limits = self.artifact_policy().await?;
		if descriptor.size > limits.artifact_bytes
			|| descriptor.size > i64::MAX as u64
		{
			return Err(invalid(
				"artifact.size_exceeded",
				"the declared Artifact exceeds the ingestion limit",
			));
		}
		self.store
			.read(async |tx| tx.run(run_id.0).await)
			.await?
			.ok_or_else(files::missing)?;
		drop(_access);
		self.collect_artifacts(actor).await?;
		let _publication = self.artifact_publication.lock().await;
		let _file_lock = files::publication_lock(self.run_home()).await?;
		self.check_disposable(descriptor.size).await?;
		let home = self.run_home();
		let stage_home = home.clone();
		let size = descriptor.size;
		let (pending, file) = filesystem::blocking(move || {
			let (pending, file) = files::stage(&stage_home)?;
			files::reserve(&file, size, limits.free_reserve_bytes)?;
			// Sparse length reserves the declared disposable bytes across Core instances
			// and crashes. Publication still requires every byte and the verified hash.
			file.set_len(size).map_err(files::io_error)?;
			Ok::<_, CoreError>((pending, file))
		})
		.await??;
		Ok(ArtifactUpload {
			pending,
			file: tokio::fs::File::from_std(file),
			descriptor,
			run_id,
			client_id: actor.client_id(),
			home,
			received: 0,
			hash: Sha256::new(),
			failed: false,
			free_reserve_bytes: limits.free_reserve_bytes,
		})
	}

	/// Verifies, atomically publishes, then commits the Run reference and Event.
	/// Retrying the same Run/hash creates no additional reference or Event.
	/// Returns a stable error without a reference for any incomplete publication.
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "publication must fence collection until the asynchronous reference transaction commits"
	)]
	pub async fn publish_artifact(
		&self,
		actor: &Actor,
		upload: ArtifactUpload,
	) -> Result<ArtifactDescriptor, CoreError> {
		let _access =
			self.remote_access.acquire().await.expect("authority gate");
		actor.authorize(&self.remote_sessions)?;
		if upload.client_id != actor.client_id()
			|| upload.home != self.run_home()
		{
			return Err(invalid(
				"artifact.invalid_upload",
				"the upload belongs to another Actor or Plane",
			));
		}
		drop(_access);
		if upload.failed || upload.received != upload.descriptor.size {
			return Err(invalid(
				"artifact.size_mismatch",
				"the upload did not reach its declared size",
			));
		}
		if format!("{:x}", upload.hash.finalize()) != upload.descriptor.sha256 {
			return Err(invalid(
				"artifact.hash_mismatch",
				"the upload did not match its declared SHA-256",
			));
		}
		upload.file.sync_all().await.map_err(files::io_error)?;
		let mut file = upload.file.into_std().await;
		let _publication = self.artifact_publication.lock().await;
		let _file_lock = files::publication_lock(self.run_home()).await?;
		self.check_disposable(0).await?;
		let descriptor = upload.descriptor;
		let expected = descriptor.clone();
		let run_id = upload.run_id;
		let limits = self.artifact_policy().await?;
		if descriptor.size > limits.artifact_bytes {
			return Err(invalid(
				"artifact.size_exceeded",
				"the Artifact exceeds the current ingestion limit",
			));
		}
		filesystem::blocking(move || {
			files::verify(&mut file, &expected)?;
			files::reserve(&file, 0, limits.free_reserve_bytes)?;
			match crate::change_artifact_budget::publish_with_limit(
				upload.pending,
				run_id,
				&expected.sha256,
				expected.size,
				limits.run_bytes,
				crate::change_artifact::directory(&upload.home)
					.map_err(files::io_error)?,
			)? {
				ArtifactAvailability::Stored => Ok(()),
				ArtifactAvailability::DiskPressure => {
					Err(crate::disk_pressure::pressure())
				}
				ArtifactAvailability::RunBudgetExceeded => Err(invalid(
					"artifact.run_budget_exceeded",
					"the Run's Artifact ingestion budget is exhausted",
				)),
				ArtifactAvailability::ArtifactSizeExceeded => Err(invalid(
					"artifact.size_exceeded",
					"the Artifact exceeds the ingestion limit",
				)),
			}
		})
		.await??;
		// ASVS 2.3.3, 15.4.2: collector uses the same publication gate. A failed
		// commit leaves only an unreferenced object eligible after the grace period.
		let _access =
			self.remote_access.acquire().await.expect("authority gate");
		actor.authorize(&self.remote_sessions)?;
		self.store
			.write(async |tx| {
				let run = tx.run(run_id.0).await?.ok_or_else(files::missing)?;
				if tx
					.reference_artifact(
						run_id.0,
						&descriptor.sha256,
						descriptor.size as i64,
					)
					.await?
				{
					tx.append_event(
						crate::EventKind::ArtifactPublished {
							artifact: descriptor.clone(),
						}
						.to_record(
							actor,
							crate::event::EventSubject::Run {
								conversation_id: crate::ConversationId(
									run.conversation_id,
								),
								run_id,
							},
							self.now_unix_ms(),
						)?,
					)
					.await?;
				}
				Ok::<_, CoreError>(())
			})
			.await?;
		Ok(descriptor)
	}

	/// Opens only published content. The reader verifies SHA-256 at completion.
	/// Returns not-found, authorization, storage, or corruption errors.
	pub async fn artifact(
		&self,
		actor: &Actor,
		sha256: &str,
	) -> Result<ArtifactDownload, CoreError> {
		let _access =
			self.remote_access.acquire().await.expect("authority gate");
		actor.authorize(&self.remote_sessions)?;
		validate_hash(sha256)?;
		let size = self
			.store
			.read(async |tx| tx.artifact_size(sha256).await)
			.await?
			.ok_or_else(files::missing)?;
		drop(_access);
		let descriptor = ArtifactDescriptor {
			sha256: sha256.into(),
			size: size as u64,
		};
		let expected = descriptor.clone();
		let home = self.run_home();
		let file = filesystem::blocking(move || {
			let directory = files::directory(&home)?;
			let file = files::open(&directory, &expected.sha256)?;
			if file.metadata().map_err(files::io_error)?.len() != expected.size
			{
				return Err(files::corrupt());
			}
			Ok::<_, CoreError>(file)
		})
		.await??;
		let _access =
			self.remote_access.acquire().await.expect("authority gate");
		actor.authorize(&self.remote_sessions)?;
		Ok(ArtifactDownload {
			file: tokio::fs::File::from_std(file),
			descriptor,
			received: 0,
			hash: Sha256::new(),
		})
	}
}
pub(crate) fn validate_hash(sha256: &str) -> Result<(), CoreError> {
	if !crate::craft_specification::is_sha256(sha256) {
		return Err(invalid(
			"artifact.invalid_hash",
			"Artifact addresses must be canonical SHA-256",
		));
	}
	Ok(())
}
fn invalid(code: &'static str, message: &'static str) -> CoreError {
	CoreError::invalid_input(code, message)
}
