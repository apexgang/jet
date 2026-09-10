//! Crash-safe publication of verified Craft Artifacts and manifests.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use jet_store::EffectKindRecord;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::craft_installation::{InstallationManifest, InstallationPlan};
use crate::craft_specification::enabled_features;
use crate::effect::{Effect, EffectAdapter, EffectResult};
use crate::{Core, CoreError};

impl Core {
	/// Publishes every durably accepted Craft installation still pending.
	///
	/// # Errors
	/// Returns a stable core error when the outbox cannot be reconciled.
	pub async fn perform_craft_installations(&self) -> Result<(), CoreError> {
		let effects = self
			.reconcile_effects(
				&mut CraftInstallations(self),
				EffectKindRecord::InstallCraft,
			)
			.await?;
		if !effects.is_empty() {
			self.observe_capabilities().await;
		}
		Ok(())
	}
}

struct CraftInstallations<'a>(&'a Core);

impl EffectAdapter for CraftInstallations<'_> {
	async fn execute(&mut self, effect: &Effect) -> EffectResult {
		let Some(plan) = load_plan(self.0, effect.effect_id).await else {
			return EffectResult::Failed;
		};
		let home = self.0.run_home().join("crafts");
		let effect_id = effect.effect_id;
		crate::filesystem::blocking(move || publish(&home, effect_id, &plan))
			.await
			.ok()
			.and_then(Result::ok)
			.map_or(EffectResult::Unknown, |()| EffectResult::Completed)
	}

	async fn reconcile(&mut self, effect: &Effect) -> EffectResult {
		let Some(plan) = load_plan(self.0, effect.effect_id).await else {
			return EffectResult::Failed;
		};
		let home = self.0.run_home().join("crafts");
		match crate::filesystem::blocking(move || {
			installed_matches(&home, &plan)
		})
		.await
		{
			Ok(Ok(true)) => EffectResult::Completed,
			Ok(Ok(false) | Err(_)) | Err(_) => EffectResult::Unknown,
		}
	}
}

async fn load_plan(core: &Core, effect_id: Uuid) -> Option<InstallationPlan> {
	let encoded = core
		.store
		.read(async |tx| tx.craft_installation_plan(effect_id).await)
		.await
		.ok()??;
	serde_json::from_str(&encoded).ok()
}

pub(crate) async fn installed_digest(
	home: PathBuf,
	craft_id: String,
) -> Result<Option<String>, CoreError> {
	crate::filesystem::blocking(move || {
		let path = home.join(format!("{craft_id}.json"));
		match read_manifest(&path) {
			Ok(manifest) => Ok(Some(manifest.sha256)),
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
				Ok(None)
			}
			Err(_) => Err(installation_failed()),
		}
	})
	.await?
}

pub(crate) struct ArtifactStaging {
	file: tokio::fs::File,
	path: PathBuf,
	artifacts: PathBuf,
	sha256: String,
	size: u64,
	written: u64,
	digest: Sha256,
}

pub(crate) async fn begin_stage(
	home: PathBuf,
	effect_id: Uuid,
	sha256: &str,
	size: u64,
) -> Result<ArtifactStaging, CoreError> {
	let sha256 = sha256.to_owned();
	let (file, path, artifacts) =
		crate::filesystem::blocking(move || -> std::io::Result<_> {
			let staging = owner_directory(&home, "staging")?;
			let artifacts = owner_directory(&home, "artifacts")?;
			let path = staging.join(stage_name(effect_id));
			let file = OpenOptions::new()
				.write(true)
				.create_new(true)
				.mode(0o700)
				.open(&path)?;
			Ok((file, path, artifacts))
		})
		.await?
		.map_err(|_| installation_failed())?;
	Ok(ArtifactStaging {
		file: tokio::fs::File::from_std(file),
		path,
		artifacts,
		sha256,
		size,
		written: 0,
		digest: Sha256::new(),
	})
}

impl ArtifactStaging {
	pub(crate) async fn write_chunk(
		&mut self,
		chunk: &[u8],
	) -> Result<(), CoreError> {
		let written = self
			.written
			.checked_add(chunk.len() as u64)
			.filter(|written| *written <= self.size)
			.ok_or_else(artifact_mismatch)?;
		let file = self
			.file
			.try_clone()
			.await
			.map_err(|_| installation_failed())?
			.into_std()
			.await;
		let bytes = chunk.len() as u64;
		crate::filesystem::blocking(move || {
			crate::disk_pressure::reserve(&file, bytes, 0)
		})
		.await??;
		self.file
			.write_all(chunk)
			.await
			.map_err(|_| installation_failed())?;
		self.digest.update(chunk);
		self.written = written;
		Ok(())
	}

	pub(crate) async fn finish(self) -> Result<(), CoreError> {
		let digest = format!("{:x}", self.digest.clone().finalize());
		if self.written != self.size || digest != self.sha256 {
			self.abort().await;
			return Err(artifact_mismatch());
		}
		if self.file.sync_all().await.is_err() {
			self.abort().await;
			return Err(installation_failed());
		}
		drop(self.file);
		let path = self.path;
		let artifacts = self.artifacts;
		let sha256 = self.sha256;
		let size = self.size;
		crate::filesystem::blocking(move || -> std::io::Result<()> {
			let executable = artifacts.join(&sha256);
			if executable.exists() {
				verify_artifact(&executable, &sha256, size)?;
				fs::remove_file(&path)?;
			} else {
				fs::rename(&path, &executable)?;
				sync_directory(&artifacts)?;
			}
			fs::set_permissions(
				&executable,
				fs::Permissions::from_mode(0o700),
			)?;
			Ok(())
		})
		.await?
		.map_err(|_| installation_failed())
	}

	pub(crate) async fn abort(self) {
		drop(self.file);
		let _ = tokio::fs::remove_file(self.path).await;
	}
}

fn publish(
	home: &Path,
	effect_id: Uuid,
	plan: &InstallationPlan,
) -> std::io::Result<()> {
	let artifacts = owner_directory(home, "artifacts")?;
	let executable = artifacts.join(&plan.artifact_sha256);
	verify_artifact(&executable, &plan.artifact_sha256, plan.artifact_size)?;
	fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
	let manifest = InstallationManifest {
		version: plan.version.clone(),
		repository: plan.repository.clone(),
		publisher_claim: plan.publisher_claim.clone(),
		commit: plan.commit.clone(),
		trust: plan.trust,
		executable: executable.canonicalize()?,
		sha256: plan.artifact_sha256.clone(),
		artifact_size: plan.artifact_size,
		specification: plan.specification.clone(),
	};
	publish_manifest(
		home,
		effect_id,
		&plan.craft_id,
		&manifest,
		plan.previous_sha256.as_deref(),
	)
}

fn publish_manifest(
	home: &Path,
	effect_id: Uuid,
	craft_id: &str,
	manifest: &InstallationManifest,
	previous_sha256: Option<&str>,
) -> std::io::Result<()> {
	let encoded =
		serde_json::to_vec(manifest).map_err(std::io::Error::other)?;
	let temporary = home.join(format!(".{effect_id}.json"));
	let destination = home.join(format!("{craft_id}.json"));
	if destination.exists() {
		let current = read_manifest(&destination)?;
		if current == *manifest {
			return Ok(());
		}
		if previous_sha256 != Some(current.sha256.as_str()) {
			return Err(std::io::Error::other(
				"Craft changed since update preparation",
			));
		}
	} else if previous_sha256.is_some() {
		return Err(std::io::Error::other(
			"Craft disappeared since update preparation",
		));
	}
	match fs::symlink_metadata(&temporary) {
		Ok(metadata) if metadata.file_type().is_file() => {
			fs::remove_file(&temporary)?;
		}
		Ok(_) => {
			return Err(std::io::Error::other("invalid temporary manifest"));
		}
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
		Err(error) => return Err(error),
	}
	let mut file = OpenOptions::new()
		.write(true)
		.create_new(true)
		.mode(0o600)
		.open(&temporary)?;
	file.write_all(&encoded)?;
	file.sync_all()?;
	drop(file);
	if previous_sha256.is_some() {
		// ASVS 15.4.2: serialized Effects replace only the expected default;
		// active executions retain their immutable content-addressed Artifact.
		fs::rename(&temporary, &destination)?;
		return sync_directory(home);
	}
	// ASVS 5.3.2: the destination name is derived from a validated Craft id;
	// hard-linking provides create-if-absent semantics and never overwrites.
	let linked = fs::hard_link(&temporary, &destination);
	let _ = fs::remove_file(&temporary);
	match linked {
		Ok(()) => sync_directory(home),
		Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
			exact_manifest(&destination, manifest)
		}
		Err(error) => Err(error),
	}
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
	fs::File::open(path)?.sync_all()
}

fn exact_manifest(
	path: &Path,
	expected: &InstallationManifest,
) -> std::io::Result<()> {
	if read_manifest(path)? == *expected {
		Ok(())
	} else {
		Err(std::io::Error::other("a different Craft is installed"))
	}
}

fn installed_matches(
	home: &Path,
	plan: &InstallationPlan,
) -> std::io::Result<bool> {
	let path = home.join(format!("{}.json", plan.craft_id));
	if !path.exists() {
		return Ok(false);
	}
	let manifest = read_manifest(&path)?;
	let expected = InstallationManifest {
		version: plan.version.clone(),
		repository: plan.repository.clone(),
		publisher_claim: plan.publisher_claim.clone(),
		commit: plan.commit.clone(),
		trust: plan.trust,
		executable: home
			.join("artifacts")
			.join(&plan.artifact_sha256)
			.canonicalize()?,
		sha256: plan.artifact_sha256.clone(),
		artifact_size: plan.artifact_size,
		specification: plan.specification.clone(),
	};
	if manifest != expected {
		return Err(std::io::Error::other("a different Craft is installed"));
	}
	verify_artifact(
		&manifest.executable,
		&manifest.sha256,
		manifest.artifact_size,
	)?;
	Ok(true)
}

fn verify_artifact(
	path: &Path,
	sha256: &str,
	size: u64,
) -> std::io::Result<()> {
	if !fs::symlink_metadata(path)?.file_type().is_file() {
		return Err(std::io::Error::other("invalid Artifact file"));
	}
	let mut file = fs::File::open(path)?;
	if file.metadata()?.len() != size {
		return Err(std::io::Error::other("invalid Artifact file"));
	}
	let mut digest = Sha256::new();
	let mut buffer = [0_u8; 64 * 1024];
	loop {
		let read = file.read(&mut buffer)?;
		if read == 0 {
			break;
		}
		digest.update(&buffer[..read]);
	}
	if format!("{:x}", digest.finalize()) != sha256 {
		return Err(std::io::Error::other("Artifact digest changed"));
	}
	Ok(())
}

fn owner_directory(home: &Path, child: &str) -> std::io::Result<PathBuf> {
	prepare_owner_directory(home)?;
	fs::set_permissions(home, fs::Permissions::from_mode(0o700))?;
	let directory = home.join(child);
	prepare_owner_directory(&directory)?;
	fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
	Ok(directory)
}

fn prepare_owner_directory(path: &Path) -> std::io::Result<()> {
	match fs::symlink_metadata(path) {
		Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
		Ok(_) => {
			Err(std::io::Error::other("Craft directory is not a directory"))
		}
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
			fs::create_dir(path)?;
			if let Some(parent) = path.parent() {
				sync_directory(parent)?;
			}
			Ok(())
		}
		Err(error) => Err(error),
	}
}

fn stage_name(effect_id: Uuid) -> String {
	format!("{effect_id}.artifact")
}

fn read_manifest(path: &Path) -> std::io::Result<InstallationManifest> {
	if !fs::symlink_metadata(path)?.file_type().is_file() {
		return Err(std::io::Error::other("invalid Craft manifest"));
	}
	let file = fs::File::open(path)?;
	if file.metadata()?.len() > 256 * 1024 {
		return Err(std::io::Error::other("invalid Craft manifest"));
	}
	serde_json::from_reader(file.take(256 * 1024 + 1))
		.map_err(std::io::Error::other)
}

pub(crate) async fn installed_crafts(
	home: PathBuf,
) -> Vec<crate::InstalledCraft> {
	crate::filesystem::blocking(move || scan_installed(&home))
		.await
		.unwrap_or_default()
}

fn scan_installed(home: &Path) -> Vec<crate::InstalledCraft> {
	let Ok(entries) = fs::read_dir(home) else {
		return Vec::new();
	};
	let artifact_home = home.join("artifacts").canonicalize().ok();
	let mut installed = Vec::new();
	for entry in entries.flatten().take(256) {
		if entry.path().extension().and_then(|value| value.to_str())
			!= Some("json")
		{
			continue;
		}
		let Ok(manifest) = read_manifest(&entry.path()) else {
			continue;
		};
		let expected_name = format!("{}.json", manifest.specification.id);
		let artifact_name = manifest
			.executable
			.file_name()
			.and_then(|name| name.to_str());
		let artifact_parent = manifest
			.executable
			.parent()
			.and_then(|parent| parent.canonicalize().ok());
		if entry.file_name().to_string_lossy() != expected_name
			|| artifact_name != Some(manifest.sha256.as_str())
			|| artifact_parent.as_ref() != artifact_home.as_ref()
			|| enabled_features(&manifest.specification).is_err()
			|| verify_artifact(
				&manifest.executable,
				&manifest.sha256,
				manifest.artifact_size,
			)
			.is_err()
		{
			continue;
		}
		installed.push(crate::InstalledCraft {
			limits_subagents: crate::craft_specification::limits_subagents(
				&manifest.specification,
			),
			craft: crate::CraftId(manifest.specification.id),
			version: manifest.version,
			harnesses: vec![crate::HarnessId(manifest.specification.harness)],
		});
	}
	installed.sort_by(|left, right| left.craft.0.cmp(&right.craft.0));
	installed.dedup_by(|left, right| left.craft == right.craft);
	installed
}

pub(crate) fn installation_failed() -> CoreError {
	CoreError::unavailable(
		"craft.installation_failed",
		"the verified Craft could not be installed",
		"Craft Artifact publication failed",
	)
}

fn artifact_mismatch() -> CoreError {
	CoreError::invalid_input(
		"craft.artifact_mismatch",
		"the downloaded Artifact does not match the confirmed SHA-256",
	)
}
