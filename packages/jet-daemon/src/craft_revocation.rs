//! Signed Jet release metadata. The trusted key is provisioned separately.
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use std::{io::Read, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
	digests: Vec<String>,
	signature: Vec<u8>,
}

pub(crate) fn key(path: &Path) -> Result<VerifyingKey, std::io::Error> {
	let bytes = read(path, 32)?;
	let bytes: [u8; 32] = bytes.try_into().map_err(|_| invalid())?;
	VerifyingKey::from_bytes(&bytes).map_err(|_| invalid())
}

pub(crate) fn load(
	path: &Path,
	key: &VerifyingKey,
) -> Result<Vec<String>, std::io::Error> {
	let bytes = match read(path, 512 * 1024) {
		Ok(bytes) => bytes,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
			return Ok(vec![]);
		}
		Err(error) => return Err(error),
	};
	verify(&bytes, key)
}

fn verify(
	bytes: &[u8],
	key: &VerifyingKey,
) -> Result<Vec<String>, std::io::Error> {
	let metadata: Metadata =
		serde_json::from_slice(bytes).map_err(|_| invalid())?;
	// ASVS 2.2.1: bounded, sorted, unique lowercase SHA-256 digests give one
	// interpretation of the signature, with no paths or executable declarations.
	if metadata.digests.len() > 4096
		|| metadata.digests.windows(2).any(|pair| pair[0] >= pair[1])
		|| metadata.digests.iter().any(|digest| {
			digest.len() != 64
				|| !digest.bytes().all(|byte| {
					byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
				})
		}) {
		return Err(invalid());
	}
	let mut message = b"jet.craft-revocations.v1\0".to_vec();
	for digest in &metadata.digests {
		message.extend_from_slice(digest.as_bytes());
		message.push(b'\n');
	}
	// The key never comes from downloaded metadata or a Craft publisher.
	key.verify_strict(
		&message,
		&Signature::from_slice(&metadata.signature).map_err(|_| invalid())?,
	)
	.map_err(|_| invalid())?;
	Ok(metadata.digests)
}

fn read(path: &Path, limit: u64) -> Result<Vec<u8>, std::io::Error> {
	let file = std::fs::File::open(path)?;
	if !file.metadata()?.is_file() {
		return Err(invalid());
	}
	let mut bytes = Vec::new();
	file.take(limit + 1).read_to_end(&mut bytes)?;
	if bytes.len() as u64 > limit {
		return Err(invalid());
	}
	Ok(bytes)
}

fn invalid() -> std::io::Error {
	std::io::Error::other("invalid signed Jet Craft revocation metadata")
}
