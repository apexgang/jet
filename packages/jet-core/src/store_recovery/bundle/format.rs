//! Bounded binary container inside the authenticated age stream.
use super::crypto::invalid;
use crate::{ArtifactDescriptor, CoreError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(super) const MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_MANIFEST: usize = 1024 * 1024;
const MAGIC: &[u8] = b"JET-RECOVERY\0\x01";
pub(super) type Components = BTreeMap<String, Vec<u8>>;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
	version: u32,
	event_payload_version: u32,
	components: Vec<Component>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Component {
	name: String,
	size: usize,
	sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CraftMetadata {
	pub(super) craft: String,
	pub(super) version: String,
	pub(super) harnesses: Vec<String>,
}
pub(super) fn hash(bytes: &[u8]) -> String {
	format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn artifact_name(name: &str) -> Option<&str> {
	name.strip_prefix("artifacts/").filter(|hash| {
		hash.len() == 64
			&& hash
				.bytes()
				.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
	})
}
pub(super) fn encode(parts: Components) -> Result<Vec<u8>, CoreError> {
	let manifest = Manifest {
		version: 1,
		event_payload_version: 1,
		components: parts
			.iter()
			.map(|(name, bytes)| Component {
				name: name.clone(),
				size: bytes.len(),
				sha256: hash(bytes),
			})
			.collect(),
	};
	let manifest = serde_json::to_vec(&manifest).map_err(|_| invalid())?;
	let size = parts
		.values()
		.try_fold(MAGIC.len() + 4 + 32 + manifest.len(), |size, bytes| {
			size.checked_add(bytes.len())
		})
		.ok_or_else(invalid)?;
	if manifest.len() > MAX_MANIFEST || size > MAX_BYTES || parts.len() > 4096 {
		return Err(invalid());
	}
	let mut bytes = Vec::with_capacity(size);
	bytes.extend_from_slice(MAGIC);
	bytes.extend_from_slice(&(manifest.len() as u32).to_be_bytes());
	bytes.extend_from_slice(&Sha256::digest(&manifest));
	bytes.extend_from_slice(&manifest);
	for payload in parts.into_values() {
		bytes.extend_from_slice(&payload);
	}
	Ok(bytes)
}
pub(super) fn decode(bytes: &[u8]) -> Result<Components, CoreError> {
	// ASVS 5.2.1, 11.2.5: no paths or payloads are trusted until every size,
	// manifest hash, component hash, and the complete stream have been checked.
	if bytes.len() > MAX_BYTES || !bytes.starts_with(MAGIC) {
		return Err(invalid());
	}
	let mut remaining = &bytes[MAGIC.len()..];
	let length = u32::from_be_bytes(
		take(&mut remaining, 4)?.try_into().map_err(|_| invalid())?,
	) as usize;
	if length > MAX_MANIFEST {
		return Err(invalid());
	}
	let expected = take(&mut remaining, 32)?;
	let manifest = take(&mut remaining, length)?;
	if Sha256::digest(manifest).as_slice() != expected {
		return Err(invalid());
	}
	let manifest: Manifest =
		serde_json::from_slice(manifest).map_err(|_| invalid())?;
	if manifest.version != 1
		|| manifest.event_payload_version != 1
		|| manifest.components.len() > 4096
	{
		return Err(invalid());
	}
	let mut parts = Components::new();
	for component in manifest.components {
		if !matches!(component.name.as_str(), "state.sqlite3" | "crafts.json")
			&& artifact_name(&component.name).is_none()
		{
			return Err(invalid());
		}
		let payload = take(&mut remaining, component.size)?;
		if hash(payload) != component.sha256
			|| artifact_name(&component.name)
				.is_some_and(|name| name != component.sha256)
			|| parts.insert(component.name, payload.to_vec()).is_some()
		{
			return Err(invalid());
		}
	}
	if !remaining.is_empty()
		|| !parts
			.get("state.sqlite3")
			.is_some_and(|state| state.starts_with(b"SQLite format 3\0"))
	{
		return Err(invalid());
	}
	let crafts: Vec<CraftMetadata> =
		serde_json::from_slice(parts.get("crafts.json").ok_or_else(invalid)?)
			.map_err(|_| invalid())?;
	if crafts.len() > 1024
		|| crafts.iter().any(|c| {
			c.craft.is_empty()
				|| c.craft.len() > 128
				|| c.version.len() > 128
				|| c.harnesses.len() > 128
				|| c.harnesses.iter().any(|h| h.len() > 128)
		}) {
		return Err(invalid());
	}
	Ok(parts)
}
fn take<'a>(
	bytes: &mut &'a [u8],
	length: usize,
) -> Result<&'a [u8], CoreError> {
	let (value, rest) = bytes.split_at_checked(length).ok_or_else(invalid)?;
	*bytes = rest;
	Ok(value)
}
pub(super) fn artifacts(parts: &Components) -> Vec<ArtifactDescriptor> {
	parts
		.iter()
		.filter_map(|(name, bytes)| {
			artifact_name(name).map(|hash| ArtifactDescriptor {
				sha256: hash.into(),
				size: bytes.len() as u64,
			})
		})
		.collect()
}
