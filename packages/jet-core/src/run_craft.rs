//! Immutable accepted artifact and opaque Adapter contract, independent of wire DTOs.
use crate::{CoreError, filesystem};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
	io::Read,
	path::{Path, PathBuf},
};

/// The accepted Craft artifact and Adapter-owned execution contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedCraft {
	/// Canonical executable selected by the owner, never by peer traffic.
	pub executable: PathBuf,
	/// SHA-256 of the exact accepted artifact.
	pub sha256: String,
	/// Versioned opaque Adapter state, including accepted protocol pins.
	pub adapter_state: String,
}
impl PinnedCraft {
	/// Revalidates the exact accepted artifact digest.
	///
	/// # Errors
	/// Returns unavailable when the artifact disappeared or changed.
	pub async fn verify(&self) -> Result<(), CoreError> {
		let craft = self.clone();
		filesystem::blocking(move || {
			let bytes = bounded_read(&craft.executable, 64 * 1024 * 1024)
				.map_err(|_| unavailable())?;
			if !craft.executable.is_absolute()
				|| format!("{:x}", Sha256::digest(bytes)) != craft.sha256
			{
				return Err(unavailable());
			}
			Ok(())
		})
		.await?
	}
}

fn bounded_read(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
	let file = std::fs::File::open(path)?;
	if !file.metadata()?.is_file() {
		return Err(std::io::Error::other("not a file"));
	}
	let mut bytes = Vec::new();
	file.take(limit + 1).read_to_end(&mut bytes)?;
	if bytes.len() as u64 > limit {
		return Err(std::io::Error::other("too large"));
	}
	Ok(bytes)
}

fn unavailable() -> CoreError {
	CoreError::unavailable(
		"craft.unavailable",
		"the accepted Craft is unavailable or incompatible",
		"Craft validation failed",
	)
}
