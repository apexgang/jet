//! Registration of the Bundled Crafts a Jet release pins (ADR-0107).
//!
//! `jetd` hands over the Crafts that ship beside it, each with the digest its
//! release manifest pins and the declaration it embeds. Each one is copied
//! into the content-addressed Craft Artifacts and published as an installed
//! Craft, so every installation channel exposes it without a confirmation
//! (ADR-0098) while an installed Craft with the same id always wins.

use crate::{
	Core, CoreError, SecurityState,
	craft::{
		installation::{
			CraftOrigin, CraftTrust, InstallationManifest, is_sha256,
		},
		local_source, publication,
		repository::{MAX_ARTIFACT_BYTES, MAX_SPECIFICATION_BYTES},
		specification::{CraftSpecification, enabled_features},
	},
	store_recovery::RecoveryMode,
};
use jet_store::AuditOutcome;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const MAX_VERSION_CHARS: usize = 128;

/// One Craft the running `jetd`'s own release ships and pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundledCraft {
	/// The executable inside the active release, copied but never pinned.
	pub executable: PathBuf,
	/// SHA-256 the release manifest pins for the executable.
	pub sha256: String,
	/// The Craft specification the executable embeds.
	pub specification: &'static str,
	/// The Jet release version, shared by every bundled executable.
	pub version: String,
}

/// What registration did with one Bundled Craft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundledOutcome {
	/// The Craft was published at the release digest.
	Registered,
	/// The release digest was already registered and its Artifact present.
	Unchanged,
	/// A confirmed installation owns the id and is left as it is.
	KeptInstalled,
	/// A manifest this `jetd` cannot read owns the id and is left as it is.
	KeptUnreadable,
	/// Signed release metadata revoked the digest, so nothing is published.
	Revoked,
	/// The Craft could not be registered; any previous registration stays.
	Failed(Box<CoreError>),
}

/// The result of one registration pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundledRegistration {
	/// Nothing was registered: the Plane is in read-only Recovery mode or
	/// its Security audit is not trusted, so Crafts must not change
	/// (ADR-0077, ADR-0105).
	Deferred,
	/// Every Bundled Craft, by Craft id, with what happened to it.
	Done(Vec<(String, BundledOutcome)>),
}

impl Core {
	/// Registers the Bundled Crafts of the running release, replacing only
	/// earlier bundled registrations and never an installed Craft.
	///
	/// # Errors
	/// Returns a store error when revocations cannot be read. A failure for
	/// one Craft is reported as its [`BundledOutcome::Failed`] instead.
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "the publication guard keeps collection and installations \
		          from changing an Artifact or manifest being registered"
	)]
	pub async fn register_bundled_crafts(
		&self,
		crafts: Vec<BundledCraft>,
	) -> Result<BundledRegistration, CoreError> {
		if self.recovery_mode() != RecoveryMode::Serving
			|| self.security().await != SecurityState::Trusted
		{
			return Ok(BundledRegistration::Deferred);
		}
		let revoked = self
			.store
			.read(async |tx| tx.craft_revocations().await)
			.await?;
		let home = self.run_home().join("crafts");
		let _publication = self.craft_artifact_publication.lock().await;
		let mut outcomes = Vec::with_capacity(crafts.len());
		for craft in crafts {
			outcomes.push(self.register_one(&home, craft, &revoked).await);
		}
		if outcomes
			.iter()
			.any(|(_, outcome)| *outcome == BundledOutcome::Registered)
		{
			self.observe_capabilities().await;
		}
		Ok(BundledRegistration::Done(outcomes))
	}

	async fn register_one(
		&self,
		home: &Path,
		craft: BundledCraft,
		revoked: &[String],
	) -> (String, BundledOutcome) {
		let specification = match validate(&craft) {
			Ok(specification) => specification,
			Err(error) => {
				let name = craft
					.executable
					.file_name()
					.map(|name| name.to_string_lossy().into_owned())
					.unwrap_or_default();
				return (name, BundledOutcome::Failed(Box::new(error)));
			}
		};
		let id = specification.id.clone();
		let outcome = if revoked.contains(&craft.sha256) {
			BundledOutcome::Revoked
		} else {
			self.publish(home, craft, specification)
				.await
				.unwrap_or_else(|error| BundledOutcome::Failed(Box::new(error)))
		};
		(id, outcome)
	}

	async fn publish(
		&self,
		home: &Path,
		craft: BundledCraft,
		specification: CraftSpecification,
	) -> Result<BundledOutcome, CoreError> {
		let id = specification.id.clone();
		let previous = match current(home, &craft, &specification).await? {
			Current::Absent => None,
			Current::Bundled(previous) => Some(previous),
			Current::Registered => return Ok(BundledOutcome::Unchanged),
			Current::Installed => return Ok(BundledOutcome::KeptInstalled),
			Current::Unreadable => return Ok(BundledOutcome::KeptUnreadable),
		};
		let source = craft.executable.clone();
		let size = crate::filesystem::blocking(move || {
			Ok::<_, std::io::Error>(
				local_source::open_bounded_file(&source, MAX_ARTIFACT_BYTES)?
					.metadata()?
					.len(),
			)
		})
		.await?
		.map_err(|_| source_unusable())?;
		self.check_disk(size).await?;
		let staging = publication::begin_stage(
			home.to_owned(),
			Uuid::new_v4(),
			&craft.sha256,
			size,
		)
		.await?;
		// The copy is hashed as it is written, so the bytes published are the
		// bytes the release pins, whatever the source does meanwhile.
		local_source::stage_bounded_file(
			craft.executable.clone(),
			MAX_ARTIFACT_BYTES,
			staging,
		)
		.await
		.map_err(|error| match error.code.as_str() {
			"craft.artifact_mismatch" => bundled_mismatch(),
			"craft.local_file_invalid" => source_unusable(),
			_ => error,
		})?;
		// The record commits before the manifest makes the Craft live, so a
		// registration never widens access unrecorded (ADR-0105). One that
		// then fails is recorded as failed; one a crash interrupts is
		// retried, and recorded again, on the next start.
		let sha256 = craft.sha256.clone();
		self.record_registration(&sha256, AuditOutcome::Succeeded)
			.await?;
		let manifest_home = home.to_owned();
		let published = crate::filesystem::blocking(move || {
			let manifest =
				manifest(&manifest_home, &craft, specification, size)?;
			publication::publish_manifest(
				&manifest_home,
				Uuid::new_v4(),
				&id,
				&manifest,
				previous.as_deref(),
			)
		})
		.await
		.and_then(|published| {
			published.map_err(|_| publication::installation_failed())
		});
		if let Err(error) = published {
			// The failure is reported either way; its record is best effort.
			let _ = self
				.record_registration(&sha256, AuditOutcome::Failed)
				.await;
			return Err(error);
		}
		Ok(BundledOutcome::Registered)
	}

	async fn record_registration(
		&self,
		sha256: &str,
		outcome: AuditOutcome,
	) -> Result<(), CoreError> {
		let now = self.now_unix_ms();
		self.store
			.write(async |tx| {
				crate::audit::record_bundled_craft_registration(
					tx, sha256, outcome, now,
				)
				.await
			})
			.await
	}
}

/// What currently owns a Bundled Craft's id.
enum Current {
	Absent,
	/// An earlier bundled registration, by its digest.
	Bundled(String),
	/// This exact registration, with its Artifact in place.
	Registered,
	Installed,
	Unreadable,
}

async fn current(
	home: &Path,
	craft: &BundledCraft,
	specification: &CraftSpecification,
) -> Result<Current, CoreError> {
	let home = home.to_owned();
	let craft = craft.clone();
	let specification = specification.clone();
	crate::filesystem::blocking(move || {
		let path = home.join(format!("{}.json", specification.id));
		let existing = match publication::read_manifest(&path) {
			Ok(existing) => existing,
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
				return Current::Absent;
			}
			Err(_) => return Current::Unreadable,
		};
		match existing.origin {
			CraftOrigin::Installed => return Current::Installed,
			CraftOrigin::Bundled => {}
		}
		// Only a stat: a steady-state start neither hashes nor reads the
		// release. Capability observation still verifies the Artifact.
		let registered = existing.sha256 == craft.sha256
			&& std::fs::symlink_metadata(
				home.join("artifacts").join(&craft.sha256),
			)
			.is_ok_and(|metadata| {
				metadata.file_type().is_file()
					&& metadata.len() == existing.artifact_size
			}) && manifest(
			&home,
			&craft,
			specification,
			existing.artifact_size,
		)
		.is_ok_and(|expected| expected == existing);
		if registered {
			Current::Registered
		} else {
			Current::Bundled(existing.sha256)
		}
	})
	.await
}

fn manifest(
	home: &Path,
	craft: &BundledCraft,
	specification: CraftSpecification,
	artifact_size: u64,
) -> std::io::Result<InstallationManifest> {
	Ok(InstallationManifest {
		version: craft.version.clone(),
		repository: "https://github.com/apexgang/jet".into(),
		publisher_claim: "Jet".into(),
		// Like `developer-source`, a marker rather than a Git commit; the
		// origin is what identifies a Bundled Craft.
		commit: "jet-release".into(),
		trust: CraftTrust::SameUserExecutable,
		origin: CraftOrigin::Bundled,
		executable: home
			.join("artifacts")
			.join(&craft.sha256)
			.canonicalize()?,
		sha256: craft.sha256.clone(),
		artifact_size,
		specification,
	})
}

fn validate(craft: &BundledCraft) -> Result<CraftSpecification, CoreError> {
	if craft.specification.len() > MAX_SPECIFICATION_BYTES
		|| !is_sha256(&craft.sha256)
		|| craft.version.is_empty()
		|| craft.version.chars().count() > MAX_VERSION_CHARS
		|| craft.version.chars().any(char::is_control)
	{
		return Err(bundled_invalid());
	}
	let specification: CraftSpecification =
		toml::from_str(craft.specification).map_err(|_| bundled_invalid())?;
	enabled_features(&specification).map_err(|_| bundled_invalid())?;
	Ok(specification)
}

fn bundled_invalid() -> CoreError {
	CoreError::invalid_input(
		"craft.bundled_invalid",
		"the Bundled Craft's release metadata is invalid",
	)
}

fn bundled_mismatch() -> CoreError {
	CoreError::invalid_input(
		"craft.bundled_mismatch",
		"the Bundled Craft executable does not match the digest its release \
		 pins",
	)
}

fn source_unusable() -> CoreError {
	CoreError::invalid_input(
		"craft.bundled_unusable",
		"the Bundled Craft executable is absent, oversized, or not a regular file",
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		AuditSequence, CraftId, DegradedCondition, HarnessId, InstalledCraft,
		Query, QueryResult,
		test_support::{FixedProbe, actor, start_core_with, stripped},
	};
	use pretty_assertions::assert_eq;
	use sha2::{Digest, Sha256};
	use std::sync::Arc;

	fn declaration(id: &str) -> &'static str {
		match id {
			"codex" => {
				r#"id = "codex"
harness = "codex"
schema = { major = 1, minor = 0 }
host_access = [{ kind = "executable", name = "codex" }]
features = [{ name = "turns", required = true }]

[protocol]
family = "craft"
versions = [{ major = 1, minor = 10 }]
capabilities = ["runs"]
"#
			}
			_ => {
				r#"id = "claude-code"
harness = "claude-code"
schema = { major = 1, minor = 0 }
host_access = [{ kind = "executable", name = "claude" }]
features = [{ name = "turns", required = true }]

[protocol]
family = "craft"
versions = [{ major = 1, minor = 10 }]
capabilities = ["runs"]
"#
			}
		}
	}

	struct Release {
		dir: tempfile::TempDir,
		core: Core,
	}

	impl Release {
		async fn start() -> Self {
			let dir = tempfile::tempdir().unwrap();
			std::fs::create_dir(dir.path().join("release")).unwrap();
			let core = start_core_with(
				&dir.path().join("plane.sqlite3"),
				Arc::new(crate::clock::SystemClock),
				FixedProbe::new(stripped()),
			)
			.await;
			Self { dir, core }
		}

		fn home(&self) -> PathBuf {
			self.dir.path().join("crafts")
		}

		/// Ships `bytes` as the release's executable for `id`.
		fn craft(&self, id: &str, bytes: &[u8]) -> BundledCraft {
			let executable = self.dir.path().join("release").join(id);
			let _ = std::fs::remove_file(&executable);
			std::fs::write(&executable, bytes).unwrap();
			BundledCraft {
				executable,
				sha256: hex::encode(Sha256::digest(bytes)),
				specification: declaration(id),
				version: "0.2.0".into(),
			}
		}

		async fn register(
			&self,
			crafts: &[&BundledCraft],
		) -> BundledRegistration {
			self.core
				.register_bundled_crafts(
					crafts.iter().copied().cloned().collect(),
				)
				.await
				.unwrap()
		}

		fn manifest(&self, id: &str) -> InstallationManifest {
			publication::read_manifest(&self.home().join(format!("{id}.json")))
				.unwrap()
		}

		async fn audit(&self) -> Vec<(String, Option<String>)> {
			let QueryResult::SecurityAudit(page) = self
				.core
				.query(
					&actor(),
					Query::SecurityAudit {
						after: AuditSequence(0),
					},
				)
				.await
				.unwrap()
			else {
				panic!("expected QueryResult::SecurityAudit");
			};
			page.entries
				.into_iter()
				.filter(|entry| entry.target.kind == "craft_digest")
				.map(|entry| (entry.decision, entry.target.identity))
				.collect()
		}
	}

	fn done(outcomes: &[(&str, BundledOutcome)]) -> BundledRegistration {
		BundledRegistration::Done(
			outcomes
				.iter()
				.map(|(id, outcome)| ((*id).to_owned(), outcome.clone()))
				.collect(),
		)
	}

	#[tokio::test]
	async fn a_release_registers_both_bundled_crafts_as_installed_harnesses() {
		let release = Release::start().await;
		let codex = release.craft("codex", b"codex executable");
		let claude = release.craft("claude-code", b"claude executable");

		let registration = release.register(&[&claude, &codex]).await;

		assert_eq!(
			registration,
			done(&[
				("claude-code", BundledOutcome::Registered),
				("codex", BundledOutcome::Registered),
			])
		);
		let artifact = release.home().join("artifacts").join(&codex.sha256);
		assert_eq!(
			release.manifest("codex"),
			InstallationManifest {
				version: "0.2.0".into(),
				repository: "https://github.com/apexgang/jet".into(),
				publisher_claim: "Jet".into(),
				commit: "jet-release".into(),
				trust: CraftTrust::SameUserExecutable,
				origin: CraftOrigin::Bundled,
				executable: artifact.canonicalize().unwrap(),
				sha256: codex.sha256.clone(),
				artifact_size: 16,
				specification: toml::from_str(declaration("codex")).unwrap(),
			}
		);
		assert_eq!(
			std::os::unix::fs::PermissionsExt::mode(
				&std::fs::metadata(&artifact).unwrap().permissions()
			) & 0o777,
			0o700
		);
		let capabilities = release.core.capabilities().await;
		assert_eq!(
			capabilities.crafts,
			vec![
				InstalledCraft {
					limits_subagents: false,
					craft: CraftId("claude-code".into()),
					version: "0.2.0".into(),
					harnesses: vec![HarnessId("claude-code".into())],
				},
				InstalledCraft {
					limits_subagents: false,
					craft: CraftId("codex".into()),
					version: "0.2.0".into(),
					harnesses: vec![HarnessId("codex".into())],
				},
			]
		);
		assert!(
			!capabilities
				.degraded
				.contains(&DegradedCondition::NoHarnessAvailable)
		);
		assert_eq!(
			release.audit().await,
			vec![
				(
					"craft.bundled_registered".to_owned(),
					Some(claude.sha256.clone())
				),
				("craft.bundled_registered".to_owned(), Some(codex.sha256)),
			]
		);
	}

	#[tokio::test]
	async fn a_registered_release_is_not_read_or_audited_again() {
		let release = Release::start().await;
		let codex = release.craft("codex", b"codex executable");
		release.register(&[&codex]).await;
		let manifest =
			std::fs::read(release.home().join("codex.json")).unwrap();
		std::fs::remove_file(&codex.executable).unwrap();

		let registration = release.register(&[&codex]).await;

		assert_eq!(registration, done(&[("codex", BundledOutcome::Unchanged)]));
		assert_eq!(
			std::fs::read(release.home().join("codex.json")).unwrap(),
			manifest
		);
		assert_eq!(release.audit().await.len(), 1);
	}

	#[tokio::test]
	async fn a_removed_artifact_is_copied_again() {
		let release = Release::start().await;
		let codex = release.craft("codex", b"codex executable");
		release.register(&[&codex]).await;
		let artifact = release.home().join("artifacts").join(&codex.sha256);
		std::fs::remove_file(&artifact).unwrap();

		let registration = release.register(&[&codex]).await;

		assert_eq!(
			registration,
			done(&[("codex", BundledOutcome::Registered)])
		);
		assert_eq!(std::fs::read(artifact).unwrap(), b"codex executable");
	}

	#[tokio::test]
	async fn a_new_release_replaces_the_bundled_registration_and_keeps_the_old_artifact()
	 {
		let release = Release::start().await;
		let old = release.craft("codex", b"codex executable");
		release.register(&[&old]).await;
		let new = release.craft("codex", b"codex executable, next release");

		let registration = release.register(&[&new]).await;

		assert_eq!(
			registration,
			done(&[("codex", BundledOutcome::Registered)])
		);
		assert_eq!(release.manifest("codex").sha256, new.sha256);
		assert!(release.home().join("artifacts").join(&old.sha256).is_file());
	}

	#[tokio::test]
	async fn an_installed_craft_with_the_same_id_is_kept() {
		for trust in
			[CraftTrust::SameUserExecutable, CraftTrust::DeveloperSource]
		{
			let release = Release::start().await;
			let old = release.craft("codex", b"codex executable");
			release.register(&[&old]).await;
			let mut installed = release.manifest("codex");
			installed.origin = CraftOrigin::Installed;
			installed.trust = trust;
			let encoded = serde_json::to_vec(&installed).unwrap();
			std::fs::write(release.home().join("codex.json"), &encoded)
				.unwrap();
			let new = release.craft("codex", b"codex executable, next release");

			let registration = release.register(&[&new]).await;

			assert_eq!(
				registration,
				done(&[("codex", BundledOutcome::KeptInstalled)])
			);
			assert_eq!(
				std::fs::read(release.home().join("codex.json")).unwrap(),
				encoded
			);
		}
	}

	#[tokio::test]
	async fn a_hand_provisioned_manifest_is_left_untouched() {
		let release = Release::start().await;
		let codex = release.craft("codex", b"codex executable");
		std::fs::create_dir(release.home()).unwrap();
		let legacy = serde_json::json!({
			"executable": codex.executable, "sha256": codex.sha256,
			"specification": {"id": "codex"},
		})
		.to_string();
		std::fs::write(release.home().join("codex.json"), &legacy).unwrap();

		let registration = release.register(&[&codex]).await;

		assert_eq!(
			registration,
			done(&[("codex", BundledOutcome::KeptUnreadable)])
		);
		assert_eq!(
			std::fs::read_to_string(release.home().join("codex.json")).unwrap(),
			legacy
		);
	}

	#[tokio::test]
	async fn an_executable_that_does_not_match_its_digest_fails_alone() {
		let release = Release::start().await;
		let claude = release.craft("claude-code", b"claude executable");
		let mut codex = release.craft("codex", b"codex executable");
		codex.sha256 = hex::encode(Sha256::digest(b"another executable"));

		let registration = release.register(&[&codex, &claude]).await;

		assert_eq!(
			registration,
			done(&[
				(
					"codex",
					BundledOutcome::Failed(Box::new(bundled_mismatch()))
				),
				("claude-code", BundledOutcome::Registered),
			])
		);
		assert!(!release.home().join("codex.json").exists());
		assert!(
			!release
				.home()
				.join("artifacts")
				.join(&codex.sha256)
				.exists()
		);
		assert_eq!(
			std::fs::read_dir(release.home().join("staging"))
				.unwrap()
				.count(),
			0
		);
	}

	#[tokio::test]
	async fn a_symlinked_executable_is_refused() {
		let release = Release::start().await;
		let mut codex = release.craft("codex", b"codex executable");
		let link = release.dir.path().join("release/linked");
		std::os::unix::fs::symlink(&codex.executable, &link).unwrap();
		codex.executable = link;

		let registration = release.register(&[&codex]).await;

		assert_eq!(
			registration,
			done(&[(
				"codex",
				BundledOutcome::Failed(Box::new(source_unusable()))
			)])
		);
		assert!(!release.home().join("codex.json").exists());
	}

	#[tokio::test]
	async fn a_revoked_digest_is_not_registered() {
		let release = Release::start().await;
		let codex = release.craft("codex", b"codex executable");
		release
			.core
			.store
			.write(async |tx| tx.revoke_craft_digest(&codex.sha256).await)
			.await
			.unwrap();

		let registration = release.register(&[&codex]).await;

		assert_eq!(registration, done(&[("codex", BundledOutcome::Revoked)]));
		assert!(!release.home().join("codex.json").exists());
	}

	#[tokio::test]
	async fn nothing_is_registered_while_the_audit_is_not_trusted() {
		let release = Release::start().await;
		let codex = release.craft("codex", b"codex executable");
		*release.core.security.write().await = SecurityState::Unverified;

		let registration = release.register(&[&codex]).await;

		assert_eq!(registration, BundledRegistration::Deferred);
		assert!(!release.home().exists());
	}

	#[test]
	fn an_installed_manifest_keeps_the_shape_older_jetd_reads() {
		let installed = InstallationManifest {
			version: "1.2.3".into(),
			repository: "https://github.com/apex/jet-craft-demo".into(),
			publisher_claim: "Example Org".into(),
			commit: "0123456789abcdef0123456789abcdef01234567".into(),
			trust: CraftTrust::SameUserExecutable,
			origin: CraftOrigin::Installed,
			executable: "/jet/crafts/artifacts/demo".into(),
			sha256: "ab".repeat(32),
			artifact_size: 1,
			specification: toml::from_str(declaration("codex")).unwrap(),
		};
		let encoded = serde_json::to_value(&installed).unwrap();

		assert_eq!(encoded.get("origin"), None);
		assert_eq!(
			serde_json::from_value::<InstallationManifest>(encoded).unwrap(),
			installed
		);
	}
}
