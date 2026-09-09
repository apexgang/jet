//! Bounded, forward-compatible third-party Craft specification validation.

use serde::{Deserialize, Serialize};

use crate::CoreError;
use crate::capability::Platform;
use crate::craft_installation::{BrokerPermission, CraftHostAccess};

const MAX_ID_CHARS: usize = 80;
const MAX_PUBLISHER_CHARS: usize = 256;
const MAX_VERSION_CHARS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CraftSpecification {
	schema: Version,
	pub(crate) id: String,
	pub(crate) harness: String,
	protocol: ProtocolOffer,
	#[serde(default)]
	pub(crate) features: Vec<CraftFeature>,
	#[serde(default)]
	pub(crate) broker_permissions: Vec<BrokerPermission>,
	#[serde(default)]
	pub(crate) host_access: Vec<CraftHostAccess>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct PublishedSpecification {
	#[serde(flatten)]
	pub(crate) specification: CraftSpecification,
	pub(crate) version: String,
	pub(crate) publisher: String,
	pub(crate) artifacts: Vec<PublishedArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Version {
	major: u32,
	minor: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ProtocolOffer {
	family: ProtocolFamily,
	versions: Vec<Version>,
	#[serde(default)]
	capabilities: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProtocolFamily {
	Craft,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CraftFeature {
	name: String,
	#[serde(default)]
	required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct PublishedArtifact {
	pub(crate) name: String,
	pub(crate) operating_system: String,
	pub(crate) architecture: String,
	pub(crate) sha256: String,
}

pub(crate) fn parse(bytes: &[u8]) -> Result<PublishedSpecification, CoreError> {
	if bytes.len() > crate::craft_repository::MAX_SPECIFICATION_BYTES {
		return Err(invalid("the Craft specification exceeds 65536 bytes"));
	}
	let text = std::str::from_utf8(bytes)
		.map_err(|_| invalid("the Craft specification is not UTF-8"))?;
	// ASVS 1.5.2, 2.2.1: authority-bearing declarations decode into closed
	// enums, while additive record metadata remains forward-compatible.
	toml::from_str(text)
		.map_err(|_| invalid("the Craft specification is invalid"))
}

pub(crate) fn validate_publisher(publisher: &str) -> Result<(), CoreError> {
	if publisher.is_empty()
		|| publisher.chars().count() > MAX_PUBLISHER_CHARS
		|| publisher.chars().any(char::is_control)
	{
		return Err(invalid("the publisher claim is invalid"));
	}
	Ok(())
}

pub(crate) fn version_text(version: &str) -> bool {
	!version.is_empty()
		&& version.chars().count() <= MAX_VERSION_CHARS
		&& version.bytes().all(|byte| {
			byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')
		})
}

pub(crate) fn enabled_features(
	specification: &CraftSpecification,
) -> Result<Vec<String>, CoreError> {
	let valid_id =
		identifier(&specification.id) && identifier(&specification.harness);
	let valid_protocol = specification.schema.major == 1
		&& !specification.protocol.versions.is_empty()
		&& specification.protocol.versions.len() <= 16
		&& specification.protocol.versions.iter().enumerate().all(
			|(index, version)| {
				version.major > 0
					&& !specification.protocol.versions[..index]
						.iter()
						.any(|earlier| earlier.major == version.major)
			},
		) && specification
		.protocol
		.versions
		.iter()
		.any(|version| version.major == 1 && version.minor >= 1)
		&& specification.protocol.capabilities.len() <= 64
		&& specification.protocol.capabilities.iter().enumerate().all(
			|(index, value)| {
				identifier(value)
					&& !specification.protocol.capabilities[..index]
						.contains(value)
			},
		) && specification
		.protocol
		.capabilities
		.iter()
		.any(|value| value == "runs");
	let declarations_bounded =
		specification.features.len() <= 64
			&& specification.broker_permissions.len() <= 3
			&& specification.host_access.len() <= 64
			&& specification
				.features
				.iter()
				.all(|feature| identifier(&feature.name))
			&& specification.broker_permissions.iter().enumerate().all(
				|(index, permission)| {
					!specification.broker_permissions[..index]
						.contains(permission)
				},
			) && specification.host_access.iter().all(valid_host_access);
	if !valid_id || !valid_protocol || !declarations_bounded {
		return Err(invalid("the Craft declaration is incompatible"));
	}
	let mut enabled = Vec::new();
	for feature in &specification.features {
		if [
			"turns",
			"actions",
			"resume",
			"fork",
			"remote_tools",
			"extensions",
		]
		.contains(&feature.name.as_str())
		{
			enabled.push(feature.name.clone());
		} else if feature.required {
			return Err(invalid("the Craft requires an unknown declaration"));
		}
	}
	enabled.sort();
	enabled.dedup();
	if !enabled.iter().any(|feature| feature == "turns") {
		return Err(invalid("the Craft does not support managed turns"));
	}
	Ok(enabled)
}

fn valid_host_access(access: &CraftHostAccess) -> bool {
	let value = match access {
		CraftHostAccess::Executable { name }
		| CraftHostAccess::Environment { name } => name,
		CraftHostAccess::Filesystem { path } => path,
		CraftHostAccess::Network { destination } => destination,
	};
	!value.is_empty()
		&& value.chars().count() <= 512
		&& !value.chars().any(char::is_control)
}

pub(crate) fn select_artifact(
	published: &PublishedSpecification,
	platform: Platform,
) -> Result<&PublishedArtifact, CoreError> {
	let mut artifacts = published.artifacts.iter().filter(|artifact| {
		artifact.operating_system == platform.operating_system
			&& artifact.architecture == platform.architecture
	});
	let artifact = artifacts
		.next()
		.ok_or_else(|| invalid("the release has no Artifact for this Plane"))?;
	if published.artifacts.is_empty()
		|| published.artifacts.len() > 32
		|| artifacts.next().is_some()
		|| !filename(&artifact.name)
		|| !is_sha256(&artifact.sha256)
	{
		return Err(invalid(
			"the release Artifact declaration is ambiguous or invalid",
		));
	}
	Ok(artifact)
}

fn identifier(value: &str) -> bool {
	!value.is_empty()
		&& value.len() <= MAX_ID_CHARS
		&& value.bytes().all(|byte| {
			byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
		})
}

fn filename(value: &str) -> bool {
	identifier(value)
		|| (!value.is_empty()
			&& value.len() <= 160
			&& value.bytes().all(|byte| {
				byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)
			}))
}

pub(crate) fn is_sha256(value: &str) -> bool {
	value.len() == 64
		&& value
			.bytes()
			.all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn invalid(message: &'static str) -> CoreError {
	CoreError::invalid_input("craft.release_invalid", message)
}
