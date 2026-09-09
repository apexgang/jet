//! Third-party Craft discovery and the exact consent it requires.
//!
//! Repository metadata and Craft specifications are hostile input until this
//! module has bound a qualifying repository, immutable commit, release asset,
//! and SHA-256 digest into one preview (ADR-0013, ADR-0098, ADR-0100).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::audit::{AuditSubject, Decision};
use crate::command::{CommandId, CommandOutcome};
use crate::craft_repository::ReleasedCraft;
pub(crate) use crate::craft_specification::is_sha256;
use crate::craft_specification::{
	CraftSpecification, PublishedSpecification, enabled_features,
	parse as parse_specification, select_artifact, validate_publisher,
	version_text,
};
use crate::{Core, CoreError};
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
			crate::craft_local_source::preview(core, specification, artifact)
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
	crate::craft_artifact_collection::collect_unreferenced(
		&core.store,
		craft_home.clone(),
		core.clock.now(),
	)
	.await?;
	let previous_sha256 = crate::craft_publication::installed_digest(
		craft_home.clone(),
		preview.craft_id.clone(),
	)
	.await?;
	if matches!(&preview.confirmation.source, CraftSource::Local { .. }) {
		require_developer_mode(core).await?;
	}
	let staging = crate::craft_publication::begin_stage(
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
					crate::craft_repository::MAX_ARTIFACT_BYTES,
					staging,
				)
				.await?;
		}
		CraftSource::Local { artifact, .. } => {
			crate::craft_local_source::stage_bounded_file(
				artifact.clone(),
				crate::craft_repository::MAX_ARTIFACT_BYTES,
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
		return Err(crate::craft_lifecycle::revoked());
	}
	if let Some((previous, state)) =
		tx.latest_craft_installation(&plan.craft_id).await?
	{
		let previous: InstallationPlan = serde_json::from_str(&previous)
			.map_err(|_| crate::craft_publication::installation_failed())?;
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
		.map_err(|_| crate::craft_publication::installation_failed())?;
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
		|| asset.size > crate::craft_repository::MAX_ARTIFACT_BYTES
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
