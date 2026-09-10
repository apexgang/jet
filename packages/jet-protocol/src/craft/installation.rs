//! Verified third-party Craft discovery and installation consent (ADR-0013,
//! ADR-0098, ADR-0100).

use serde::{Deserialize, Serialize};

/// An explicit source from which a third-party Craft may be considered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CraftSource {
	/// A public GitHub Release from a qualifying `jet-craft-*` repository.
	#[serde(rename = "github_release")]
	GitHubRelease {
		/// GitHub repository in `owner/name` form.
		repository: String,
		/// Exact release tag to resolve and pin.
		tag: String,
	},
	/// Local files accepted only while Developer Mode is enabled.
	Local {
		/// Absolute or Plane-local path to `.jet/craft-spec.toml`.
		specification: String,
		/// Path to the locally built executable Artifact.
		artifact: String,
	},
}

/// The executable trust implication an interactive user must accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CraftTrust {
	/// A verified release still executes with the user's OS authority.
	SameUserExecutable,
	/// A local or source build has no verified release provenance.
	DeveloperSource,
}

/// Every security-sensitive value copied from preview into confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CraftInstallationConfirmation {
	/// Exact source the user previewed.
	pub source: CraftSource,
	/// Canonical repository identity, or the local specification identity.
	pub repository: String,
	/// Publisher's unendorsed claim from the specification.
	pub publisher_claim: String,
	/// Immutable Git commit, or `developer-source` for a local build.
	pub commit: String,
	/// SHA-256 of the exact executable Artifact.
	pub artifact_sha256: String,
	/// Jet-enforced broker authority the user accepts.
	pub broker_permissions: Vec<crate::BrokerPermission>,
	/// Same-user host access the user accepts.
	pub host_access: Vec<crate::CraftHostAccess>,
	/// Jet's explicit trust boundary for this executable.
	pub trust: CraftTrust,
}

/// A verified immutable proposal that performs no installation by itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CraftInstallationPreview {
	/// Stable Craft identity.
	pub craft_id: String,
	/// Publisher-declared release version.
	pub version: String,
	/// Known optional features that remain enabled.
	pub enabled_features: Vec<String>,
	/// Exact consent object a subsequent installation Command must return.
	pub confirmation: CraftInstallationConfirmation,
}

/// Durable publication work accepted for one verified Craft Artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CraftInstallationQueued {
	/// Stable Craft identity.
	pub craft_id: String,
	/// Publisher-declared release version.
	pub version: String,
	/// SHA-256 of the exact Artifact being published.
	pub artifact_sha256: String,
}
