//! Third-party Craft discovery and the exact consent it requires.
//!
//! Repository metadata and Craft specifications are hostile input until this
//! module has bound a qualifying repository, immutable commit, release asset,
//! and SHA-256 digest into one preview (ADR-0013, ADR-0098, ADR-0100).

use crate::{
	audit::{AuditSubject, Decision},
	command::{CommandId, CommandOutcome},
	craft::repository::ReleasedCraft,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(crate) use crate::craft::specification::is_sha256;
use crate::{
	Core, CoreError,
	craft::specification::{
		CraftSpecification, PublishedSpecification, enabled_features,
		parse as parse_specification, select_artifact, validate_publisher,
		version_text,
	},
};
use jet_store::{
	EffectKindRecord, EffectSafetyRecord, NewEffect, WriteTransaction,
};
use uuid::Uuid;

const MAX_REPOSITORY_PART_CHARS: usize = 100;
const MAX_TAG_CHARS: usize = 128;

/// An explicit source from which a Craft may be considered for installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CraftSource {
	/// A pinned public GitHub Release from a qualifying repository.
	GitHubRelease {
		/// GitHub `owner/repository`, whose repository starts `jet-craft-`.
		repository: String,
		/// Exact release tag whose commit and assets are resolved.
		tag: String,
	},
	/// Local files accepted only while Developer Mode is enabled.
	Local {
		/// Path to the versioned `.jet/craft-spec.toml`.
		specification: PathBuf,
		/// Path to the executable artifact.
		artifact: PathBuf,
	},
}

/// Jet-enforced broker authority requested by a Craft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerPermission {
	/// Read an authorized Artifact.
	ArtifactRead,
	/// Publish an Artifact.
	ArtifactWrite,
	/// Use authorized tools on a paired Plane.
	RemoteTools,
}

/// Same-user host access disclosed by the publisher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CraftHostAccess {
	/// An executable the Craft expects to launch.
	Executable {
		/// Executable name disclosed by the publisher.
		name: String,
	},
	/// A filesystem area the Craft expects to use.
	Filesystem {
		/// Path or path class disclosed by the publisher.
		path: String,
	},
	/// An environment input, never its value.
	Environment {
		/// Environment variable name disclosed by the publisher.
		name: String,
	},
	/// A network destination the Craft expects to contact.
	Network {
		/// Network destination disclosed by the publisher.
		destination: String,
	},
}

/// The executable trust implication an interactive user must accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CraftTrust {
	/// A verified release still executes with the user's operating-system authority.
	SameUserExecutable,
	/// A local or source build has no verified release provenance.
	DeveloperSource,
}

/// Every security-sensitive value copied from the preview into confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CraftInstallationConfirmation {
	/// Exact source the user previewed.
	pub source: CraftSource,
	/// Canonical repository identity.
	pub repository: String,
	/// Publisher's claim from the specification; Jet does not endorse it.
	pub publisher_claim: String,
	/// Immutable Git commit, or the local-source marker in Developer Mode.
	pub commit: String,
	/// SHA-256 of the exact executable Artifact.
	pub artifact_sha256: String,
	/// Broker authority the user accepts.
	pub broker_permissions: Vec<BrokerPermission>,
	/// Same-user access the user accepts.
	pub host_access: Vec<CraftHostAccess>,
	/// Explicit statement of Jet's v1 trust boundary.
	pub trust: CraftTrust,
}

/// A verified, immutable proposal that performs no installation by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CraftInstallationPreview {
	/// Stable Craft identity.
	pub craft_id: String,
	/// Publisher-declared release version.
	pub version: String,
	/// Known optional features that remain enabled.
	pub enabled_features: Vec<String>,
	pub(crate) confirmation: CraftInstallationConfirmation,
	pub(crate) artifact_url: String,
	pub(crate) artifact_size: u64,
	pub(crate) specification: CraftSpecification,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InstallationPlan {
	#[serde(default)]
	pub(crate) previous_sha256: Option<String>,
	pub(crate) craft_id: String,
	pub(crate) version: String,
	pub(crate) artifact_sha256: String,
	pub(crate) artifact_size: u64,
	pub(crate) specification: CraftSpecification,
	pub(crate) repository: String,
	pub(crate) publisher_claim: String,
	pub(crate) commit: String,
	pub(crate) trust: CraftTrust,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct InstallationManifest {
	pub(crate) version: String,
	pub(crate) repository: String,
	pub(crate) publisher_claim: String,
	pub(crate) commit: String,
	pub(crate) trust: CraftTrust,
	pub(crate) executable: PathBuf,
	pub(crate) sha256: String,
	pub(crate) artifact_size: u64,
	pub(crate) specification: CraftSpecification,
}

/// Verified installation data staged before the authoritative transaction.
pub(crate) struct PreparedCraftInstallation {
	effect_id: Uuid,
	plan: InstallationPlan,
}

impl CraftInstallationPreview {
	/// Exact values a user must return to authorize this proposal.
	#[must_use]
	pub fn confirmation(&self) -> CraftInstallationConfirmation {
		self.confirmation.clone()
	}
}

pub(crate) async fn discover(
	core: &Core,
	source: CraftSource,
) -> Result<CraftInstallationPreview, CoreError> {
	match &source {
		CraftSource::GitHubRelease { repository, tag } => {
			let repository = qualifying_repository(repository)?;
			validate_tag(tag)?;
			let released =
				core.craft_repository.release(repository, tag).await?;
			preview_release(core, source.clone(), repository, tag, released)
				.await
		}
		CraftSource::Local {
			specification,
			artifact,
		} => {
			require_developer_mode(core).await?;
			crate::craft::local_source::preview(core, specification, artifact)
				.await
		}
	}
}

#[expect(
	clippy::await_holding_invalid_type,
	reason = "the guard must span orphan marking and Artifact publication so \
	          collection cannot remove a content address an installation is \
	          about to reference"
)]
pub(crate) async fn prepare(
	core: &Core,
	confirmation: &CraftInstallationConfirmation,
) -> Result<PreparedCraftInstallation, CoreError> {
	let preview = discover(core, confirmation.source.clone()).await?;
	if preview.confirmation() != *confirmation {
		return Err(CoreError::conflict(
			"craft.installation_stale",
			"the Craft release changed since it was previewed; review it again",
		));
	}
	let effect_id = Uuid::now_v7();
	let craft_home = core.run_home().join("crafts");
	let _artifact_guard = core.craft_artifact_publication.lock().await;
	crate::craft::artifact_collection::collect_unreferenced(
		&core.store,
		craft_home.clone(),
		core.clock.now(),
	)
	.await?;
	let previous_sha256 = crate::craft::publication::installed_digest(
		craft_home.clone(),
		preview.craft_id.clone(),
	)
	.await?;
	if matches!(&preview.confirmation.source, CraftSource::Local { .. }) {
		require_developer_mode(core).await?;
	}
	core.check_disk(preview.artifact_size).await?;
	let staging = crate::craft::publication::begin_stage(
		craft_home,
		effect_id,
		&confirmation.artifact_sha256,
		preview.artifact_size,
	)
	.await?;
	match &preview.confirmation.source {
		CraftSource::GitHubRelease { .. } => {
			core.craft_repository
				.download(
					&preview.artifact_url,
					crate::craft::repository::MAX_ARTIFACT_BYTES,
					staging,
				)
				.await?;
		}
		CraftSource::Local { artifact, .. } => {
			crate::craft::local_source::stage_bounded_file(
				artifact.clone(),
				crate::craft::repository::MAX_ARTIFACT_BYTES,
				staging,
			)
			.await?;
		}
	}
	Ok(PreparedCraftInstallation {
		effect_id,
		plan: InstallationPlan {
			previous_sha256,
			craft_id: preview.craft_id,
			version: preview.version,
			artifact_sha256: confirmation.artifact_sha256.clone(),
			artifact_size: preview.artifact_size,
			specification: preview.specification,
			repository: confirmation.repository.clone(),
			publisher_claim: confirmation.publisher_claim.clone(),
			commit: confirmation.commit.clone(),
			trust: confirmation.trust,
		},
	})
}

pub(crate) async fn record(
	tx: &mut WriteTransaction,
	actor: &crate::Actor,
	command_id: CommandId,
	prepared: PreparedCraftInstallation,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let PreparedCraftInstallation { effect_id, plan } = prepared;
	if tx.craft_revoked(&plan.artifact_sha256).await? {
		return Err(crate::craft::lifecycle::revoked());
	}
	if let Some((previous, state)) =
		tx.latest_craft_installation(&plan.craft_id).await?
	{
		let previous: InstallationPlan = serde_json::from_str(&previous)
			.map_err(|_| crate::craft::publication::installation_failed())?;
		// ASVS 15.4.2: compare the durable predecessor in the admission transaction.
		if state != "completed"
			|| plan.previous_sha256.as_ref() != Some(&previous.artifact_sha256)
		{
			return Err(CoreError::conflict(
				"craft.update_pending",
				"the previous Craft publication must settle before this update; preview it again",
			));
		}
	}
	if plan.trust == CraftTrust::DeveloperSource {
		let developer_mode =
			crate::setting::resolve_plane(tx, crate::SettingKey::DeveloperMode)
				.await?;
		require_developer_mode_value(developer_mode)?;
	}
	tx.insert_effect(&NewEffect {
		effect_id,
		command_id: command_id.0,
		run_id: None,
		promotion_id: None,
		terminal_id: None,
		kind: EffectKindRecord::InstallCraft,
		safety: EffectSafetyRecord::Idempotent {
			external_key: effect_id,
			max_attempts: 3,
		},
	})
	.await?;
	let encoded = serde_json::to_string(&plan)
		.map_err(|_| crate::craft::publication::installation_failed())?;
	tx.insert_craft_installation_plan(effect_id, &plan.craft_id, &encoded)
		.await?;
	crate::audit::record(
		tx,
		actor,
		Decision::succeeded(
			crate::AuditDecision::CraftInstallationApproved,
			AuditSubject::Craft(plan.craft_id.clone()),
		),
		now_unix_ms,
	)
	.await?;
	Ok(CommandOutcome::CraftInstallationQueued {
		craft_id: plan.craft_id,
		version: plan.version,
		artifact_sha256: plan.artifact_sha256,
	})
}

async fn require_developer_mode(core: &Core) -> Result<(), CoreError> {
	let value = core
		.store
		.read(async |tx| {
			crate::setting::resolve_plane(tx, crate::SettingKey::DeveloperMode)
				.await
		})
		.await?;
	require_developer_mode_value(value)
}

fn require_developer_mode_value(
	value: crate::SettingValue,
) -> Result<(), CoreError> {
	if value == crate::SettingValue::Flag(true) {
		Ok(())
	} else {
		Err(CoreError::conflict(
			"craft.developer_mode_required",
			"local and source-built Crafts require Developer Mode",
		))
	}
}

async fn preview_release(
	core: &Core,
	source: CraftSource,
	repository: &str,
	tag: &str,
	released: ReleasedCraft,
) -> Result<CraftInstallationPreview, CoreError> {
	let published = parse_specification(&released.specification)?;
	validate_release(repository, tag, &released, &published)?;
	let platform = core.capabilities.read().await.platform;
	let declared = select_artifact(&published, platform)?.clone();
	let asset = released
		.artifacts
		.iter()
		.find(|asset| asset.name == declared.name)
		.ok_or_else(|| {
			invalid("the declared Artifact is absent from the release")
		})?
		.clone();
	if asset.sha256 != declared.sha256
		|| asset.size > crate::craft::repository::MAX_ARTIFACT_BYTES
	{
		return Err(invalid(
			"the release Artifact does not match its declaration",
		));
	}
	let enabled_features = enabled_features(&published.specification)?;
	let confirmation = CraftInstallationConfirmation {
		source,
		repository: format!("https://github.com/{repository}"),
		publisher_claim: published.publisher.clone(),
		commit: released.commit,
		artifact_sha256: declared.sha256.clone(),
		broker_permissions: published.specification.broker_permissions.clone(),
		host_access: published.specification.host_access.clone(),
		trust: CraftTrust::SameUserExecutable,
	};
	Ok(CraftInstallationPreview {
		craft_id: published.specification.id.clone(),
		version: published.version,
		enabled_features,
		confirmation,
		artifact_url: asset.url,
		artifact_size: asset.size,
		specification: published.specification,
	})
}

fn validate_release(
	repository: &str,
	tag: &str,
	released: &ReleasedCraft,
	published: &PublishedSpecification,
) -> Result<(), CoreError> {
	let (_, name) = repository.split_once('/').expect("validated repository");
	let identity_matches =
		name == format!("jet-craft-{}", published.specification.id);
	let version_matches =
		tag == published.version || tag == format!("v{}", published.version);
	if released.repository != repository
		|| released.tag != tag
		|| !is_commit(&released.commit)
		|| !identity_matches
		|| !version_matches
		|| !version_text(&published.version)
	{
		return Err(invalid(
			"the release provenance or version is inconsistent",
		));
	}
	validate_publisher(&published.publisher)
}

fn qualifying_repository(repository: &str) -> Result<&str, CoreError> {
	let Some((owner, name)) = repository.split_once('/') else {
		return Err(invalid("a GitHub repository must be owner/name"));
	};
	if repository.matches('/').count() != 1
		|| !repository_part(owner)
		|| !repository_part(name)
		|| !name.starts_with("jet-craft-")
	{
		return Err(invalid(
			"the GitHub repository is not a qualifying Jet Craft",
		));
	}
	Ok(repository)
}

fn repository_part(value: &str) -> bool {
	!value.is_empty()
		&& value.chars().count() <= MAX_REPOSITORY_PART_CHARS
		&& value
			.bytes()
			.all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

fn validate_tag(tag: &str) -> Result<(), CoreError> {
	if tag.is_empty()
		|| tag.chars().count() > MAX_TAG_CHARS
		|| tag.chars().any(char::is_control)
	{
		return Err(invalid("the release tag is invalid"));
	}
	Ok(())
}

fn is_commit(value: &str) -> bool {
	value.len() == 40
		&& value
			.bytes()
			.all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn invalid(message: &'static str) -> CoreError {
	CoreError::invalid_input("craft.release_invalid", message)
}

#[cfg(test)]
pub(crate) mod tests {
	use jet_store::audit_head_path;
	use pretty_assertions::assert_eq;
	use sha2::{Digest, Sha256};
	use std::time::{Duration, SystemTime};

	use crate::test_support::{
		FixedCraftRepository, actor, request, start_core_installing,
	};
	use crate::{
		AuditSequence, BrokerPermission, CapabilityObservation, Command,
		CommandEnvelope, CommandId, CommandOutcome, CraftHostAccess,
		CraftSource, Query, QueryResult, SettingKey, SettingScope,
		SettingValue,
	};

	fn published_specification(sha256: &str) -> Vec<u8> {
		format!(
			r#"schema = {{ major = 1, minor = 0 }}
id = "demo"
harness = "demo"
version = "1.2.3"
publisher = "Example Org"
broker_permissions = ["artifact_read"]
host_access = [{{ kind = "network", destination = "api.example.com" }}]
features = [{{ name = "turns", required = true }}, {{ name = "future-ui" }}]

[protocol]
family = "craft"
versions = [{{ major = 1, minor = 5 }}]
capabilities = ["runs"]

[[artifacts]]
name = "jet-craft-demo-linux-aarch64"
operating_system = "linux"
architecture = "aarch64"
sha256 = "{sha256}"
"#
		)
		.into_bytes()
	}

	async fn preview(
		core: &crate::Core,
		source: CraftSource,
	) -> crate::CraftInstallationPreview {
		let result = core
			.query(&actor(), Query::DiscoverCraft { source })
			.await
			.unwrap();
		let QueryResult::CraftInstallationPreview(preview) = result else {
			panic!("expected QueryResult::CraftInstallationPreview");
		};
		*preview
	}

	#[tokio::test]
	async fn a_qualifying_release_is_discovered_as_an_exact_confirmation() {
		let dir = tempfile::tempdir().unwrap();
		let artifact = b"verified executable".to_vec();
		let sha256 = format!("{:x}", Sha256::digest(&artifact));
		let repository = FixedCraftRepository::release(
			"apex/jet-craft-demo",
			"v1.2.3",
			"0123456789abcdef0123456789abcdef01234567",
			published_specification(&sha256),
			"jet-craft-demo-linux-aarch64",
			sha256.clone(),
			artifact,
		);
		let core = start_core_installing(
			&dir.path().join("plane.sqlite3"),
			repository,
		)
		.await;
		let source = CraftSource::GitHubRelease {
			repository: "apex/jet-craft-demo".into(),
			tag: "v1.2.3".into(),
		};

		let result = core
			.query(
				&actor(),
				Query::DiscoverCraft {
					source: source.clone(),
				},
			)
			.await
			.unwrap();
		let QueryResult::CraftInstallationPreview(preview) = result else {
			panic!("expected QueryResult::CraftInstallationPreview");
		};

		assert_eq!(preview.craft_id, "demo");
		assert_eq!(preview.version, "1.2.3");
		assert_eq!(preview.enabled_features, vec!["turns"]);
		assert_eq!(
			preview.confirmation(),
			crate::CraftInstallationConfirmation {
				source,
				repository: "https://github.com/apex/jet-craft-demo".into(),
				publisher_claim: "Example Org".into(),
				commit: "0123456789abcdef0123456789abcdef01234567".into(),
				artifact_sha256: sha256,
				broker_permissions: vec![BrokerPermission::ArtifactRead],
				host_access: vec![CraftHostAccess::Network {
					destination: "api.example.com".into(),
				}],
				trust: crate::CraftTrust::SameUserExecutable,
			},
		);
	}

	#[tokio::test]
	async fn an_exact_confirmation_publishes_an_audited_installed_craft() {
		let dir = tempfile::tempdir().unwrap();
		let artifact = b"verified executable".to_vec();
		let sha256 = format!("{:x}", Sha256::digest(&artifact));
		let repository = FixedCraftRepository::release(
			"apex/jet-craft-demo",
			"v1.2.3",
			"0123456789abcdef0123456789abcdef01234567",
			published_specification(&sha256),
			"jet-craft-demo-linux-aarch64",
			sha256.clone(),
			artifact,
		);
		let core = start_core_installing(
			&dir.path().join("plane.sqlite3"),
			repository,
		)
		.await;
		let proposal = preview(
			&core,
			CraftSource::GitHubRelease {
				repository: "apex/jet-craft-demo".into(),
				tag: "v1.2.3".into(),
			},
		)
		.await;

		let command_id = uuid::Uuid::now_v7();
		let confirmation = proposal.confirmation();
		let outcome = core
			.execute(
				&actor(),
				CommandEnvelope::new(
					CommandId(command_id),
					Command::InstallCraft {
						confirmation: confirmation.clone(),
					},
					b"confirmed Craft installation",
				)
				.unwrap(),
			)
			.await
			.unwrap();
		assert_eq!(
			outcome,
			CommandOutcome::CraftInstallationQueued {
				craft_id: "demo".into(),
				version: "1.2.3".into(),
				artifact_sha256: sha256.clone(),
			}
		);
		assert!(dir.path().join("crafts/artifacts").join(&sha256).is_file());
		assert!(!dir.path().join("crafts/demo.json").exists());
		let replay = core
			.execute(
				&actor(),
				CommandEnvelope::new(
					CommandId(command_id),
					Command::InstallCraft { confirmation },
					b"confirmed Craft installation",
				)
				.unwrap(),
			)
			.await
			.unwrap();
		assert_eq!(replay, outcome);
		core.perform_craft_installations().await.unwrap();

		let capabilities = core
			.query(
				&actor(),
				Query::Capabilities {
					observation: CapabilityObservation::Fresh,
				},
			)
			.await
			.unwrap();
		let QueryResult::Capabilities(capabilities) = capabilities else {
			panic!("expected QueryResult::Capabilities");
		};
		assert!(capabilities.crafts.iter().any(|craft| {
			craft.craft.0 == "demo"
				&& craft.version == "1.2.3"
				&& craft.harnesses == vec![crate::HarnessId("demo".into())]
		}));
		let audit = core
			.query(
				&actor(),
				Query::SecurityAudit {
					after: AuditSequence(0),
				},
			)
			.await
			.unwrap();
		let QueryResult::SecurityAudit(audit) = audit else {
			panic!("expected QueryResult::SecurityAudit");
		};
		assert_eq!(
			audit.entries.last().unwrap().decision,
			"craft.installation_approved"
		);
		assert_eq!(audit.entries.last().unwrap().target.kind, "craft");
		assert_eq!(
			audit.entries.last().unwrap().target.identity.as_deref(),
			Some("demo")
		);
		crate::craft::artifact_collection::collect_unreferenced(
			&core.store,
			dir.path().join("crafts"),
			SystemTime::now() + Duration::from_secs(25 * 60 * 60),
		)
		.await
		.unwrap();
		assert!(dir.path().join("crafts/artifacts").join(&sha256).exists());
	}

	#[tokio::test]
	async fn unreferenced_craft_artifacts_are_collected_only_after_a_grace_period()
	 {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core_installing(
			&dir.path().join("plane.sqlite3"),
			FixedCraftRepository::release(
				"apex/jet-craft-demo",
				"v1.2.3",
				"0123456789abcdef0123456789abcdef01234567",
				Vec::new(),
				"artifact",
				"ab".repeat(32),
				Vec::new(),
			),
		)
		.await;
		let home = dir.path().join("crafts");
		let artifacts = home.join("artifacts");
		let staging = home.join("staging");
		std::fs::create_dir_all(&artifacts).unwrap();
		std::fs::create_dir_all(&staging).unwrap();
		let orphan = artifacts.join("ab".repeat(32));
		let interrupted =
			staging.join(format!("{}.artifact", uuid::Uuid::nil()));
		std::fs::write(&orphan, b"orphan").unwrap();
		std::fs::write(&interrupted, b"partial").unwrap();

		crate::craft::artifact_collection::collect_unreferenced(
			&core.store,
			home.clone(),
			SystemTime::now(),
		)
		.await
		.unwrap();
		assert!(orphan.exists());
		assert!(interrupted.exists());
		crate::craft::artifact_collection::collect_unreferenced(
			&core.store,
			home,
			SystemTime::now() + Duration::from_secs(25 * 60 * 60),
		)
		.await
		.unwrap();
		assert!(!orphan.exists());
		assert!(!interrupted.exists());
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn craft_collection_does_not_follow_an_artifact_directory_symlink() {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core_installing(
			&dir.path().join("plane.sqlite3"),
			FixedCraftRepository::release(
				"apex/jet-craft-demo",
				"v1.2.3",
				"0123456789abcdef0123456789abcdef01234567",
				Vec::new(),
				"artifact",
				"ab".repeat(32),
				Vec::new(),
			),
		)
		.await;
		let home = dir.path().join("crafts");
		let outside = dir.path().join("outside");
		std::fs::create_dir_all(&home).unwrap();
		std::fs::create_dir_all(&outside).unwrap();
		let outside_artifact = outside.join("ab".repeat(32));
		std::fs::write(&outside_artifact, b"outside").unwrap();
		std::os::unix::fs::symlink(&outside, home.join("artifacts")).unwrap();

		let _ = crate::craft::artifact_collection::collect_unreferenced(
			&core.store,
			home,
			SystemTime::now() + Duration::from_secs(25 * 60 * 60),
		)
		.await;

		assert!(outside_artifact.exists());
	}

	#[tokio::test]
	async fn an_unknown_required_declaration_rejects_the_release() {
		let dir = tempfile::tempdir().unwrap();
		let artifact = b"verified executable".to_vec();
		let sha256 = format!("{:x}", Sha256::digest(&artifact));
		let specification = String::from_utf8(published_specification(&sha256))
			.unwrap()
			.replace(
				"{ name = \"future-ui\" }",
				"{ name = \"future-ui\", required = true }",
			)
			.into_bytes();
		let repository = FixedCraftRepository::release(
			"apex/jet-craft-demo",
			"v1.2.3",
			"0123456789abcdef0123456789abcdef01234567",
			specification,
			"jet-craft-demo-linux-aarch64",
			sha256,
			artifact,
		);
		let core = start_core_installing(
			&dir.path().join("plane.sqlite3"),
			repository,
		)
		.await;

		let error = core
			.query(
				&actor(),
				Query::DiscoverCraft {
					source: CraftSource::GitHubRelease {
						repository: "apex/jet-craft-demo".into(),
						tag: "v1.2.3".into(),
					},
				},
			)
			.await
			.unwrap_err();

		assert_eq!(error.code, "craft.release_invalid");
	}

	#[tokio::test]
	async fn an_incompatible_or_ambiguous_craft_protocol_rejects_the_release() {
		for versions in [
			"[{ major = 99, minor = 5 }]",
			"[{ major = 1, minor = 5 }, { major = 1, minor = 4 }]",
		] {
			let dir = tempfile::tempdir().unwrap();
			let artifact = b"verified executable".to_vec();
			let sha256 = format!("{:x}", Sha256::digest(&artifact));
			let specification =
				String::from_utf8(published_specification(&sha256))
					.unwrap()
					.replace("[{ major = 1, minor = 5 }]", versions)
					.into_bytes();
			let repository = FixedCraftRepository::release(
				"apex/jet-craft-demo",
				"v1.2.3",
				"0123456789abcdef0123456789abcdef01234567",
				specification,
				"jet-craft-demo-linux-aarch64",
				sha256,
				artifact,
			);
			let core = start_core_installing(
				&dir.path().join("plane.sqlite3"),
				repository,
			)
			.await;

			let error = core
				.query(
					&actor(),
					Query::DiscoverCraft {
						source: CraftSource::GitHubRelease {
							repository: "apex/jet-craft-demo".into(),
							tag: "v1.2.3".into(),
						},
					},
				)
				.await
				.unwrap_err();

			assert_eq!(error.code, "craft.release_invalid");
		}
	}

	#[tokio::test]
	async fn an_unknown_additive_specification_field_does_not_disable_the_craft()
	 {
		let dir = tempfile::tempdir().unwrap();
		let artifact = b"verified executable".to_vec();
		let sha256 = format!("{:x}", Sha256::digest(&artifact));
		let specification = String::from_utf8(published_specification(&sha256))
			.unwrap()
			.replace(
				"schema = { major = 1, minor = 0 }",
				"schema = { major = 1, minor = 0, patch = 7 }",
			)
			.replace(
				"{ name = \"turns\", required = true }",
				"{ name = \"turns\", required = true, presentation = \"compact\" }",
			)
			.replace(
				"[protocol]",
				"future_presentation = \"compact\"\n\n[protocol]\ntransport = \"stdio\"",
			)
			.replace(
				"{ major = 1, minor = 5 }",
				"{ major = 1, minor = 5, revision = 2 }",
			)
			.replace(
				"architecture = \"aarch64\"",
				"architecture = \"aarch64\"\ncontent_type = \"application/octet-stream\"",
			)
			.into_bytes();
		let repository = FixedCraftRepository::release(
			"apex/jet-craft-demo",
			"v1.2.3",
			"0123456789abcdef0123456789abcdef01234567",
			specification,
			"jet-craft-demo-linux-aarch64",
			sha256,
			artifact,
		);
		let core = start_core_installing(
			&dir.path().join("plane.sqlite3"),
			repository,
		)
		.await;

		let proposal = preview(
			&core,
			CraftSource::GitHubRelease {
				repository: "apex/jet-craft-demo".into(),
				tag: "v1.2.3".into(),
			},
		)
		.await;

		assert_eq!(proposal.enabled_features, vec!["turns"]);
	}

	#[tokio::test]
	async fn local_artifacts_require_developer_mode_and_remain_explicitly_unverified()
	 {
		let dir = tempfile::tempdir().unwrap();
		let artifact = b"locally built executable".to_vec();
		let sha256 = format!("{:x}", Sha256::digest(&artifact));
		let specification_path = dir.path().join("craft-spec.toml");
		let artifact_path = dir.path().join("jet-craft-demo-linux-aarch64");
		std::fs::write(&specification_path, published_specification(&sha256))
			.unwrap();
		std::fs::write(&artifact_path, &artifact).unwrap();
		let repository = FixedCraftRepository::release(
			"apex/jet-craft-demo",
			"v1.2.3",
			"0123456789abcdef0123456789abcdef01234567",
			published_specification(&sha256),
			"jet-craft-demo-linux-aarch64",
			sha256.clone(),
			artifact,
		);
		let core = start_core_installing(
			&dir.path().join("plane.sqlite3"),
			repository,
		)
		.await;
		let source = CraftSource::Local {
			specification: specification_path,
			artifact: artifact_path,
		};

		let refused = core
			.query(
				&actor(),
				Query::DiscoverCraft {
					source: source.clone(),
				},
			)
			.await
			.unwrap_err();
		assert_eq!(refused.code, "craft.developer_mode_required");
		core.execute(
			&actor(),
			request(Command::SetSetting {
				key: SettingKey::DeveloperMode,
				scope: SettingScope::Plane,
				value: SettingValue::Flag(true),
			}),
		)
		.await
		.unwrap();

		let proposal = preview(&core, source).await;
		assert_eq!(
			proposal.confirmation().trust,
			crate::CraftTrust::DeveloperSource
		);
		assert_eq!(proposal.confirmation().commit, "developer-source");
		core.execute(
			&actor(),
			request(Command::InstallCraft {
				confirmation: proposal.confirmation(),
			}),
		)
		.await
		.unwrap();
		core.perform_craft_installations().await.unwrap();
		let QueryResult::Capabilities(capabilities) = core
			.query(
				&actor(),
				Query::Capabilities {
					observation: CapabilityObservation::Fresh,
				},
			)
			.await
			.unwrap()
		else {
			panic!("unexpected capabilities result");
		};
		assert!(
			capabilities
				.crafts
				.iter()
				.any(|craft| craft.craft.0 == "demo")
		);
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn developer_mode_does_not_follow_a_local_artifact_symlink() {
		let dir = tempfile::tempdir().unwrap();
		let artifact = b"locally built executable".to_vec();
		let sha256 = format!("{:x}", Sha256::digest(&artifact));
		let specification_path = dir.path().join("craft-spec.toml");
		let real_directory = dir.path().join("real");
		std::fs::create_dir(&real_directory).unwrap();
		let real_artifact_path =
			real_directory.join("jet-craft-demo-linux-aarch64");
		let artifact_path = dir.path().join("jet-craft-demo-linux-aarch64");
		std::fs::write(&specification_path, published_specification(&sha256))
			.unwrap();
		std::fs::write(&real_artifact_path, artifact).unwrap();
		std::os::unix::fs::symlink(&real_artifact_path, &artifact_path)
			.unwrap();
		let repository = FixedCraftRepository::release(
			"apex/jet-craft-demo",
			"v1.2.3",
			"0123456789abcdef0123456789abcdef01234567",
			published_specification(&sha256),
			"jet-craft-demo-linux-aarch64",
			sha256,
			Vec::new(),
		);
		let core = start_core_installing(
			&dir.path().join("plane.sqlite3"),
			repository,
		)
		.await;
		core.execute(
			&actor(),
			request(Command::SetSetting {
				key: SettingKey::DeveloperMode,
				scope: SettingScope::Plane,
				value: SettingValue::Flag(true),
			}),
		)
		.await
		.unwrap();

		let error = core
			.query(
				&actor(),
				Query::DiscoverCraft {
					source: CraftSource::Local {
						specification: specification_path,
						artifact: artifact_path,
					},
				},
			)
			.await
			.unwrap_err();

		assert_eq!(error.code, "craft.local_file_invalid");
	}

	#[tokio::test]
	async fn local_installation_rechecks_developer_mode_in_its_transaction() {
		let dir = tempfile::tempdir().unwrap();
		let artifact = b"locally built executable".to_vec();
		let sha256 = format!("{:x}", Sha256::digest(&artifact));
		let specification_path = dir.path().join("craft-spec.toml");
		let artifact_path = dir.path().join("jet-craft-demo-linux-aarch64");
		std::fs::write(&specification_path, published_specification(&sha256))
			.unwrap();
		std::fs::write(&artifact_path, artifact).unwrap();
		let repository = FixedCraftRepository::release(
			"apex/jet-craft-demo",
			"v1.2.3",
			"0123456789abcdef0123456789abcdef01234567",
			published_specification(&sha256),
			"jet-craft-demo-linux-aarch64",
			sha256,
			Vec::new(),
		);
		let core = start_core_installing(
			&dir.path().join("plane.sqlite3"),
			repository,
		)
		.await;
		core.execute(
			&actor(),
			request(Command::SetSetting {
				key: SettingKey::DeveloperMode,
				scope: SettingScope::Plane,
				value: SettingValue::Flag(true),
			}),
		)
		.await
		.unwrap();
		let proposal = preview(
			&core,
			CraftSource::Local {
				specification: specification_path,
				artifact: artifact_path,
			},
		)
		.await;
		let prepared = crate::craft::installation::prepare(
			&core,
			&proposal.confirmation(),
		)
		.await
		.unwrap();
		core.execute(
			&actor(),
			request(Command::SetSetting {
				key: SettingKey::DeveloperMode,
				scope: SettingScope::Plane,
				value: SettingValue::Flag(false),
			}),
		)
		.await
		.unwrap();
		let now_unix_ms = core.now_unix_ms();

		let error = core
			.store
			.write(async |tx| {
				crate::craft::installation::record(
					tx,
					&actor(),
					CommandId(uuid::Uuid::now_v7()),
					prepared,
					now_unix_ms,
				)
				.await
			})
			.await
			.unwrap_err();

		assert_eq!(error.code, "craft.developer_mode_required");
	}

	#[tokio::test]
	async fn a_degraded_plane_does_not_stage_a_craft_before_refusing_it() {
		let dir = tempfile::tempdir().unwrap();
		let store_path = dir.path().join("plane.sqlite3");
		let artifact = b"locally built executable".to_vec();
		let sha256 = format!("{:x}", Sha256::digest(&artifact));
		let specification_path = dir.path().join("craft-spec.toml");
		let artifact_path = dir.path().join("jet-craft-demo-linux-aarch64");
		std::fs::write(&specification_path, published_specification(&sha256))
			.unwrap();
		std::fs::write(&artifact_path, artifact.clone()).unwrap();
		let repository = FixedCraftRepository::release(
			"apex/jet-craft-demo",
			"v1.2.3",
			"0123456789abcdef0123456789abcdef01234567",
			published_specification(&sha256),
			"jet-craft-demo-linux-aarch64",
			sha256,
			artifact,
		);
		let trusted =
			start_core_installing(&store_path, repository.clone()).await;
		trusted
			.execute(
				&actor(),
				request(Command::SetSetting {
					key: SettingKey::DeveloperMode,
					scope: SettingScope::Plane,
					value: SettingValue::Flag(true),
				}),
			)
			.await
			.unwrap();
		let proposal = preview(
			&trusted,
			CraftSource::Local {
				specification: specification_path,
				artifact: artifact_path,
			},
		)
		.await;
		trusted.close().await;
		drop(trusted);
		std::fs::remove_file(audit_head_path(&store_path)).unwrap();
		let degraded = start_core_installing(&store_path, repository).await;

		let error = degraded
			.execute(
				&actor(),
				request(Command::InstallCraft {
					confirmation: proposal.confirmation(),
				}),
			)
			.await
			.unwrap_err();

		assert_eq!(error.code, "security.audit_degraded");
		assert!(!dir.path().join("crafts").exists());
	}

	#[tokio::test]
	async fn changed_confirmation_and_changed_artifact_are_both_refused() {
		let dir = tempfile::tempdir().unwrap();
		let declared_artifact = b"declared executable".to_vec();
		let sha256 = format!("{:x}", Sha256::digest(&declared_artifact));
		let repository = FixedCraftRepository::release(
			"apex/jet-craft-demo",
			"v1.2.3",
			"0123456789abcdef0123456789abcdef01234567",
			published_specification(&sha256),
			"jet-craft-demo-linux-aarch64",
			sha256,
			b"changed after publication".to_vec(),
		);
		let core = start_core_installing(
			&dir.path().join("plane.sqlite3"),
			repository,
		)
		.await;
		let proposal = preview(
			&core,
			CraftSource::GitHubRelease {
				repository: "apex/jet-craft-demo".into(),
				tag: "v1.2.3".into(),
			},
		)
		.await;
		let mut changed_confirmation = proposal.confirmation();
		changed_confirmation.publisher_claim = "Impostor".into();

		let changed = core
			.execute(
				&actor(),
				request(Command::InstallCraft {
					confirmation: changed_confirmation,
				}),
			)
			.await
			.unwrap_err();
		assert_eq!(changed.code, "craft.installation_stale");
		let artifact = core
			.execute(
				&actor(),
				request(Command::InstallCraft {
					confirmation: proposal.confirmation(),
				}),
			)
			.await
			.unwrap_err();
		assert_eq!(artifact.code, "craft.artifact_mismatch");
	}
}
