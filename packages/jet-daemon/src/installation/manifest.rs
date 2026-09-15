//! The manifest a core release payload carries (ADR-0026, ADR-0053).
//!
//! One payload holds the four role executables of one Jet release for one
//! target, named by the release version the bundled products share. The
//! manifest pins each executable by digest and records what the release
//! speaks and stores, so an installer can decide compatibility before it
//! drains anything.

use serde::{Deserialize, Serialize};
use std::{
	collections::BTreeMap,
	io::{self, Read as _},
	os::unix::fs::OpenOptionsExt as _,
	path::Path,
};

/// File name of the manifest inside a payload or a staged version.
pub(crate) const MANIFEST_FILE: &str = "manifest.json";

/// The executables every payload ships, one per core role (ADR-0060).
pub(crate) const EXECUTABLES: [&str; 4] =
	["jetd", "jetfueld", "jet-craft-claude", "jet-craft-codex"];

const MANIFEST_LIMIT: u64 = 64 * 1024;
const NAME_LIMIT: usize = 64;

/// What one release payload declares about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseManifest {
	/// The Jet release version shared by every bundled product.
	pub(crate) version: String,
	/// The target the executables were built for, such as
	/// `aarch64-apple-darwin` or `universal-apple-darwin`.
	pub(crate) target: String,
	/// The Jet protocol major the executables speak.
	pub(crate) protocol_major: u32,
	/// The Jet protocol minor the executables speak.
	pub(crate) protocol_minor: u32,
	/// The newest store migration `jetd` embeds (ADR-0073).
	pub(crate) schema_version: i64,
	/// Lowercase SHA-256 of each executable, by file name.
	pub(crate) executables: BTreeMap<String, String>,
}

impl ReleaseManifest {
	/// Reads and validates the manifest inside `dir`. The manifest is data
	/// about the payload, not the trust in it: the digests it declares are
	/// what the staged executables are checked against.
	pub(crate) fn read(dir: &Path) -> io::Result<Self> {
		// ASVS 5.3.2: validate the opened regular file, bounded, without
		// following a final symlink.
		let file = std::fs::OpenOptions::new()
			.read(true)
			.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
			.open(dir.join(MANIFEST_FILE))?;
		if !file.metadata()?.is_file() {
			return Err(invalid("the release manifest is not a regular file"));
		}
		let mut bytes = Vec::new();
		file.take(MANIFEST_LIMIT + 1).read_to_end(&mut bytes)?;
		if bytes.len() as u64 > MANIFEST_LIMIT {
			return Err(invalid("the release manifest is oversized"));
		}
		let manifest: Self =
			serde_json::from_slice(&bytes).map_err(|error| {
				invalid(&format!("malformed manifest: {error}"))
			})?;
		manifest.validate()?;
		Ok(manifest)
	}

	/// Rejects names that could escape the versions directory and digests
	/// that could not have come from the release pipeline.
	fn validate(&self) -> io::Result<()> {
		if !is_safe_name(&self.version) {
			return Err(invalid("the release version is not a safe name"));
		}
		if !is_safe_name(&self.target) {
			return Err(invalid("the release target is not a safe name"));
		}
		let declared: Vec<&str> =
			self.executables.keys().map(String::as_str).collect();
		let mut expected = EXECUTABLES.to_vec();
		expected.sort_unstable();
		if declared != expected {
			return Err(invalid(
				"the manifest must name exactly the four core executables",
			));
		}
		if self.executables.values().any(|digest| !is_digest(digest)) {
			return Err(invalid(
				"an executable digest is not lowercase SHA-256",
			));
		}
		Ok(())
	}

	/// Checks that every executable in `dir` matches its declared digest.
	pub(crate) fn verify_payload(&self, dir: &Path) -> io::Result<()> {
		for (name, digest) in &self.executables {
			if jet_runtime::execution_digest(&dir.join(name))? != *digest {
				return Err(invalid(&format!(
					"{name} does not match the digest its manifest declares"
				)));
			}
		}
		Ok(())
	}
}

/// ASCII letters, digits, `.`, `-`, `_`, and `+`, not starting with a dot:
/// one path segment that cannot climb out of the versions directory.
fn is_safe_name(name: &str) -> bool {
	!name.is_empty()
		&& name.len() <= NAME_LIMIT
		&& !name.starts_with('.')
		&& name.bytes().all(|byte| {
			byte.is_ascii_alphanumeric()
				|| matches!(byte, b'.' | b'-' | b'_' | b'+')
		})
}

fn is_digest(digest: &str) -> bool {
	digest.len() == 64
		&& digest
			.bytes()
			.all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid(reason: &str) -> io::Error {
	io::Error::new(io::ErrorKind::InvalidData, reason.to_owned())
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	fn manifest(version: &str) -> ReleaseManifest {
		ReleaseManifest {
			version: version.into(),
			target: "x86_64-unknown-linux-gnu".into(),
			protocol_major: 1,
			protocol_minor: 42,
			schema_version: 20_260_914_164_556,
			executables: EXECUTABLES
				.iter()
				.map(|name| ((*name).to_owned(), "0".repeat(64)))
				.collect(),
		}
	}

	#[test]
	fn names_that_could_leave_the_versions_directory_are_refused() {
		let refused: Vec<bool> =
			["../x", ".hidden", "a/b", "", "0.2.0", "0.2.0-rc.1+build"]
				.into_iter()
				.map(is_safe_name)
				.collect();
		assert_eq!(refused, vec![false, false, false, false, true, true]);
	}

	#[test]
	fn a_manifest_must_name_exactly_the_core_executables() {
		let mut extra = manifest("0.2.0");
		extra
			.executables
			.insert("jet-craft-other".into(), "0".repeat(64));
		let mut missing = manifest("0.2.0");
		missing.executables.remove("jetfueld");
		let mut bad_digest = manifest("0.2.0");
		bad_digest
			.executables
			.insert("jetd".into(), "ABC".repeat(21) + "0");
		let outcomes: Vec<bool> =
			[manifest("0.2.0"), extra, missing, bad_digest]
				.iter()
				.map(|manifest| manifest.validate().is_ok())
				.collect();
		assert_eq!(outcomes, vec![true, false, false, false]);
	}
}
