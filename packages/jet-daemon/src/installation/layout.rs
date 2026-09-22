//! Immutable core versions under `~/.jet/core` (ADR-0026).
//!
//! ```text
//! ~/.jet/core/
//!   versions/<version>/   read-only executables and manifest.json
//!   current -> versions/<version>
//!   previous -> versions/<version>
//! ```
//!
//! A version is populated under a dot-prefixed staging name and renamed
//! into place, so a version directory that exists is complete. `current`
//! and `previous` are relative symbolic links replaced by rename, so a
//! service definition that starts `current/jetd` never sees a half-written
//! switch and the pair survives moving the Jet home.

use super::manifest::{EXECUTABLES, MANIFEST_FILE, ReleaseManifest};
use jet_runtime::JetHome;
use std::{
	fs, io,
	os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink},
	path::{Path, PathBuf},
};

const CORE_DIR: &str = "core";
const VERSIONS_DIR: &str = "versions";
const CURRENT_LINK: &str = "current";
const PREVIOUS_LINK: &str = "previous";

/// The GUI-managed version layout of one Jet home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoreVersions {
	root: PathBuf,
}

/// The two links that name the supported release pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Link {
	Current,
	Previous,
}

impl Link {
	fn name(self) -> &'static str {
		match self {
			Self::Current => CURRENT_LINK,
			Self::Previous => PREVIOUS_LINK,
		}
	}
}

impl CoreVersions {
	pub(crate) fn of(home: &JetHome) -> Self {
		Self {
			root: home.root().join(CORE_DIR),
		}
	}

	/// Directory of one staged version.
	pub(crate) fn version_dir(&self, version: &str) -> PathBuf {
		self.root.join(VERSIONS_DIR).join(version)
	}

	/// The version `current` names, when it names one.
	pub(crate) fn current(&self) -> io::Result<Option<String>> {
		self.linked(Link::Current)
	}

	/// The version `previous` names, when it names one.
	pub(crate) fn previous(&self) -> io::Result<Option<String>> {
		self.linked(Link::Previous)
	}

	/// The manifest of a staged version.
	pub(crate) fn manifest(
		&self,
		version: &str,
	) -> io::Result<ReleaseManifest> {
		ReleaseManifest::read(&self.version_dir(version))
	}

	/// Every staged version, sorted by name.
	pub(crate) fn staged(&self) -> io::Result<Vec<String>> {
		let entries = match fs::read_dir(self.root.join(VERSIONS_DIR)) {
			Ok(entries) => entries,
			Err(error) if error.kind() == io::ErrorKind::NotFound => {
				return Ok(vec![]);
			}
			Err(error) => return Err(error),
		};
		let mut versions = vec![];
		for entry in entries {
			let name = entry?.file_name().to_string_lossy().into_owned();
			if !name.starts_with('.') {
				versions.push(name);
			}
		}
		versions.sort();
		Ok(versions)
	}

	/// Verifies the payload in `payload` against its manifest and copies it
	/// into an immutable version directory. Staging the same release twice
	/// is accepted; a different payload under a staged version's name is
	/// refused rather than replaced.
	pub(crate) fn stage(&self, payload: &Path) -> io::Result<ReleaseManifest> {
		let manifest = ReleaseManifest::read(payload)?;
		manifest.verify_payload(payload)?;
		let destination = self.version_dir(&manifest.version);
		if destination.exists() {
			if ReleaseManifest::read(&destination)? != manifest {
				return Err(io::Error::new(
					io::ErrorKind::AlreadyExists,
					format!(
						"version {} is already staged with different contents",
						manifest.version
					),
				));
			}
			manifest.verify_payload(&destination)?;
			return Ok(manifest);
		}
		let versions = self.root.join(VERSIONS_DIR);
		for dir in [&self.root, &versions] {
			fs::DirBuilder::new()
				.recursive(true)
				.mode(0o700)
				.create(dir)?;
		}
		let staging = versions.join(format!(".stage-{}", manifest.version));
		remove_version_dir(&staging)?;
		fs::DirBuilder::new().mode(0o700).create(&staging)?;
		for name in EXECUTABLES {
			let mut source = fs::File::open(payload.join(name))?;
			let mut copy = fs::OpenOptions::new()
				.write(true)
				.create_new(true)
				.mode(0o500)
				.open(staging.join(name))?;
			io::copy(&mut source, &mut copy)?;
			copy.sync_all()?;
		}
		let mut file = fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.mode(0o400)
			.open(staging.join(MANIFEST_FILE))?;
		io::Write::write_all(
			&mut file,
			&serde_json::to_vec(&manifest).map_err(io::Error::other)?,
		)?;
		file.sync_all()?;
		fs::set_permissions(&staging, fs::Permissions::from_mode(0o500))?;
		fs::rename(&staging, &destination)?;
		fs::File::open(&versions)?.sync_all()?;
		Ok(manifest)
	}

	/// Points `current` at a staged version and `previous` at what
	/// `current` named before. Activating the current version changes
	/// nothing.
	pub(crate) fn activate(&self, version: &str) -> io::Result<()> {
		self.manifest(version)?;
		let former = self.current()?;
		if former.as_deref() == Some(version) {
			return Ok(());
		}
		self.point(Link::Current, version)?;
		if let Some(former) = former {
			self.point(Link::Previous, &former)?;
		}
		fs::File::open(&self.root)?.sync_all()
	}

	/// Removes every staged version other than `current` and `previous`,
	/// returning the names removed.
	pub(crate) fn prune(&self) -> io::Result<Vec<String>> {
		let keep = [self.current()?, self.previous()?];
		let mut removed = vec![];
		for version in self.staged()? {
			if keep.iter().flatten().any(|kept| *kept == version) {
				continue;
			}
			remove_version_dir(&self.version_dir(&version))?;
			removed.push(version);
		}
		Ok(removed)
	}

	fn linked(&self, link: Link) -> io::Result<Option<String>> {
		match fs::read_link(self.root.join(link.name())) {
			Ok(target) => Ok(target
				.file_name()
				.map(|name| name.to_string_lossy().into_owned())),
			Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
			Err(error) => Err(error),
		}
	}

	fn point(&self, link: Link, version: &str) -> io::Result<()> {
		let temporary = self.root.join(format!(".{}.tmp", link.name()));
		match fs::remove_file(&temporary) {
			Ok(()) => {}
			Err(error) if error.kind() == io::ErrorKind::NotFound => {}
			Err(error) => return Err(error),
		}
		symlink(Path::new(VERSIONS_DIR).join(version), &temporary)?;
		fs::rename(&temporary, self.root.join(link.name()))
	}
}

/// Removes a read-only version directory, or an interrupted staging one.
fn remove_version_dir(dir: &Path) -> io::Result<()> {
	match fs::symlink_metadata(dir) {
		Ok(metadata) if metadata.is_dir() => {
			fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
			fs::remove_dir_all(dir)
		}
		Ok(_) => fs::remove_file(dir),
		Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
		Err(error) => Err(error),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;
	use sha2::{Digest, Sha256};

	/// Writes a payload whose executables are small scripts named by
	/// `version`, with a manifest that pins them.
	fn write_payload(
		dir: &Path,
		version: &str,
		schema_version: i64,
	) -> ReleaseManifest {
		fs::create_dir_all(dir).unwrap();
		let mut executables = std::collections::BTreeMap::new();
		for name in EXECUTABLES {
			let body = format!("#!/bin/sh\necho {name} {version}\n");
			fs::write(dir.join(name), &body).unwrap();
			executables.insert(
				name.to_owned(),
				hex::encode(Sha256::digest(body.as_bytes())),
			);
		}
		let manifest = ReleaseManifest {
			version: version.into(),
			target: "x86_64-unknown-linux-gnu".into(),
			protocol_major: 1,
			protocol_minor: 42,
			schema_version,
			executables,
		};
		fs::write(
			dir.join(MANIFEST_FILE),
			serde_json::to_vec(&manifest).unwrap(),
		)
		.unwrap();
		manifest
	}

	fn home(dir: &Path) -> JetHome {
		let home = JetHome::at(dir.join(".jet"));
		home.prepare().unwrap();
		home
	}

	fn pair(layout: &CoreVersions) -> (Option<String>, Option<String>) {
		(layout.current().unwrap(), layout.previous().unwrap())
	}

	#[test]
	fn staging_copies_an_immutable_version_and_activation_keeps_the_pair() {
		let dir = tempfile::tempdir().unwrap();
		let layout = CoreVersions::of(&home(dir.path()));
		for version in ["0.2.0", "0.2.1", "0.2.2"] {
			let payload = dir.path().join(format!("payload-{version}"));
			write_payload(&payload, version, 1);
			layout.stage(&payload).unwrap();
		}
		let staged_mode =
			fs::metadata(layout.version_dir("0.2.0").join("jetd"))
				.unwrap()
				.permissions()
				.mode() & 0o777;

		let fresh = pair(&layout);
		layout.activate("0.2.0").unwrap();
		let first = pair(&layout);
		layout.activate("0.2.1").unwrap();
		let second = pair(&layout);
		layout.activate("0.2.1").unwrap();
		let unchanged = pair(&layout);
		layout.activate("0.2.0").unwrap();
		let rolled_back = pair(&layout);
		layout.activate("0.2.2").unwrap();
		let pruned = layout.prune().unwrap();

		assert_eq!(
			(
				staged_mode,
				fresh,
				first,
				second,
				unchanged,
				rolled_back,
				pruned,
				layout.staged().unwrap()
			),
			(
				0o500,
				(None, None),
				(Some("0.2.0".into()), None),
				(Some("0.2.1".into()), Some("0.2.0".into())),
				(Some("0.2.1".into()), Some("0.2.0".into())),
				(Some("0.2.0".into()), Some("0.2.1".into())),
				vec!["0.2.1".to_owned()],
				vec!["0.2.0".to_owned(), "0.2.2".to_owned()],
			)
		);
	}

	#[test]
	fn a_payload_that_does_not_match_its_manifest_is_not_staged() {
		let dir = tempfile::tempdir().unwrap();
		let layout = CoreVersions::of(&home(dir.path()));
		let payload = dir.path().join("payload");
		write_payload(&payload, "0.2.0", 1);
		fs::write(payload.join("jetfueld"), "#!/bin/sh\necho tampered\n")
			.unwrap();

		let error = layout.stage(&payload).unwrap_err();

		assert_eq!(
			(error.kind(), layout.staged().unwrap()),
			(io::ErrorKind::InvalidData, vec![])
		);
	}

	#[test]
	fn restaging_a_version_accepts_the_same_payload_and_refuses_another() {
		let dir = tempfile::tempdir().unwrap();
		let layout = CoreVersions::of(&home(dir.path()));
		let payload = dir.path().join("payload");
		let manifest = write_payload(&payload, "0.2.0", 1);
		layout.stage(&payload).unwrap();
		let again = layout.stage(&payload).unwrap();
		let other = dir.path().join("other");
		write_payload(&other, "0.2.0", 2);

		let refused = layout.stage(&other).unwrap_err();

		assert_eq!(
			(again, refused.kind()),
			(manifest, io::ErrorKind::AlreadyExists)
		);
	}
}
