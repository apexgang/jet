//! Explicit Developer Mode ingestion of local Craft specifications and Artifacts.

use crate::{
	Core, CoreError,
	craft::{
		installation::{
			CraftInstallationConfirmation, CraftInstallationPreview,
			CraftSource, CraftTrust,
		},
		publication::ArtifactStaging,
		specification::{
			enabled_features, parse, select_artifact, validate_publisher,
			version_text,
		},
	},
};
use sha2::{Digest, Sha256};
use std::{
	fs,
	os::unix::fs::OpenOptionsExt,
	path::{Path, PathBuf},
};

pub(crate) async fn preview(
	core: &Core,
	specification_path: &Path,
	artifact_path: &Path,
) -> Result<CraftInstallationPreview, CoreError> {
	let (specification_path, artifact_path) = canonical_local_paths(
		specification_path.to_path_buf(),
		artifact_path.to_path_buf(),
	)
	.await?;
	let specification_bytes = read_bounded_file(
		specification_path.clone(),
		crate::craft::repository::MAX_SPECIFICATION_BYTES as u64,
	)
	.await?;
	let (sha256, artifact_size) = hash_bounded_file(
		artifact_path.clone(),
		crate::craft::repository::MAX_ARTIFACT_BYTES,
	)
	.await?;
	let published = parse(&specification_bytes)?;
	validate_publisher(&published.publisher)?;
	if !version_text(&published.version) {
		return Err(invalid("the local Craft version is invalid"));
	}
	let platform = core.capabilities.read().await.platform;
	let declared = select_artifact(&published, platform)?.clone();
	if artifact_path.file_name().and_then(|name| name.to_str())
		!= Some(&declared.name)
	{
		return Err(invalid(
			"the local Artifact name does not match its declaration",
		));
	}
	if sha256 != declared.sha256 {
		return Err(invalid(
			"the local Artifact does not match its declared SHA-256",
		));
	}
	let enabled_features = enabled_features(&published.specification)?;
	let confirmation = CraftInstallationConfirmation {
		source: CraftSource::Local {
			specification: specification_path.clone(),
			artifact: artifact_path,
		},
		repository: format!("local:{}", specification_path.display()),
		publisher_claim: published.publisher.clone(),
		commit: "developer-source".into(),
		artifact_sha256: sha256,
		broker_permissions: published.specification.broker_permissions.clone(),
		host_access: published.specification.host_access.clone(),
		trust: CraftTrust::DeveloperSource,
	};
	Ok(CraftInstallationPreview {
		craft_id: published.specification.id.clone(),
		version: published.version,
		enabled_features,
		confirmation,
		artifact_url: String::new(),
		artifact_size,
		specification: published.specification,
	})
}

async fn canonical_local_paths(
	specification: PathBuf,
	artifact: PathBuf,
) -> Result<(PathBuf, PathBuf), CoreError> {
	crate::filesystem::blocking(move || {
		let specification = canonical_regular_path(&specification)?;
		let artifact = canonical_regular_path(&artifact)?;
		if specification == artifact {
			return Err(std::io::Error::other(
				"the specification and Artifact are the same file",
			));
		}
		Ok((specification, artifact))
	})
	.await?
	.map_err(|_| {
		CoreError::invalid_input(
			"craft.local_file_invalid",
			"the local Craft files cannot be resolved",
		)
	})
}

fn canonical_regular_path(path: &Path) -> std::io::Result<PathBuf> {
	let name = path.file_name().ok_or_else(|| {
		std::io::Error::other("local Craft path has no file name")
	})?;
	let parent = path
		.parent()
		.filter(|parent| !parent.as_os_str().is_empty());
	let parent = parent.unwrap_or_else(|| Path::new(".")).canonicalize()?;
	let path = parent.join(name);
	if !fs::symlink_metadata(&path)?.file_type().is_file() {
		return Err(std::io::Error::other(
			"local Craft path is not a regular file",
		));
	}
	Ok(path)
}

pub(crate) async fn read_bounded_file(
	path: PathBuf,
	limit: u64,
) -> Result<Vec<u8>, CoreError> {
	crate::filesystem::blocking(move || {
		let file = open_bounded_file(&path, limit)?;
		let mut bytes = Vec::with_capacity(file.metadata()?.len() as usize);
		let mut limited = std::io::Read::take(file, limit + 1);
		std::io::Read::read_to_end(&mut limited, &mut bytes)?;
		if bytes.len() as u64 > limit {
			return Err(std::io::Error::other(
				"local Craft file exceeds its limit",
			));
		}
		Ok(bytes)
	})
	.await?
	.map_err(|_| local_file_invalid())
}

pub(crate) async fn stage_bounded_file(
	path: PathBuf,
	limit: u64,
	mut staging: ArtifactStaging,
) -> Result<(), CoreError> {
	let file = match crate::filesystem::blocking(move || {
		open_bounded_file(&path, limit)
	})
	.await?
	{
		Ok(file) => file,
		Err(_) => {
			staging.abort().await;
			return Err(local_file_invalid());
		}
	};
	let mut file = tokio::fs::File::from_std(file);
	let mut buffer = [0_u8; 64 * 1024];
	loop {
		let read =
			match tokio::io::AsyncReadExt::read(&mut file, &mut buffer).await {
				Ok(read) => read,
				Err(_) => {
					staging.abort().await;
					return Err(local_file_invalid());
				}
			};
		if read == 0 {
			break;
		}
		if let Err(error) = staging.write_chunk(&buffer[..read]).await {
			staging.abort().await;
			return Err(error);
		}
	}
	staging.finish().await
}

async fn hash_bounded_file(
	path: PathBuf,
	limit: u64,
) -> Result<(String, u64), CoreError> {
	crate::filesystem::blocking(move || -> std::io::Result<(String, u64)> {
		let mut file = open_bounded_file(&path, limit)?;
		let mut digest = Sha256::new();
		let mut size = 0_u64;
		let mut buffer = [0_u8; 64 * 1024];
		loop {
			let read = std::io::Read::read(&mut file, &mut buffer)?;
			if read == 0 {
				break;
			}
			size = size
				.checked_add(read as u64)
				.filter(|size| *size <= limit)
				.ok_or_else(|| {
					std::io::Error::other("local Craft file exceeds its limit")
				})?;
			digest.update(&buffer[..read]);
		}
		Ok((format!("{:x}", digest.finalize()), size))
	})
	.await?
	.map_err(|_| local_file_invalid())
}

fn open_bounded_file(path: &Path, limit: u64) -> std::io::Result<fs::File> {
	let file = fs::OpenOptions::new()
		.read(true)
		.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
		.open(path)?;
	let metadata = file.metadata()?;
	if !metadata.is_file() || metadata.len() > limit {
		return Err(std::io::Error::other("local Craft file is invalid"));
	}
	Ok(file)
}

fn local_file_invalid() -> CoreError {
	CoreError::invalid_input(
		"craft.local_file_invalid",
		"the local Craft file is absent, oversized, or not a regular file",
	)
}

fn invalid(message: &'static str) -> CoreError {
	CoreError::invalid_input("craft.release_invalid", message)
}
