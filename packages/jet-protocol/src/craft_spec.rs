//! Craft declarations, separate from grants and OS containment (ADR-0100).

use crate::{
	IncompatibleProtocol, Negotiation, ProtocolFamily, ProtocolOffer,
	ProtocolVersion,
};
use serde::{Deserialize, Serialize};

/// A supported Harness feature. Unknown optional features are disabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CraftFeature {
	/// Feature name; v1 understands turns, actions, and resume.
	pub name: String,
	/// Whether installation must reject an unrecognized feature.
	#[serde(default)]
	pub required: bool,
}

/// Jet-enforced broker permissions, all required declarations in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerPermission {
	/// Read authorized Artifacts through the broker.
	ArtifactRead,
	/// Publish Artifacts through the broker.
	ArtifactWrite,
	/// Use authorized paired-Plane tools through the broker.
	RemoteTools,
}

/// Disclosed same-user host access; not a portable sandbox guarantee.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CraftHostAccess {
	/// An executable the Craft expects to launch.
	Executable {
		/// Executable name or absolute path.
		name: String,
	},
	/// A filesystem area the Craft expects to use.
	Filesystem {
		/// Disclosed filesystem path.
		path: String,
	},
	/// An environment input, never its secret value.
	Environment {
		/// Environment variable name.
		name: String,
	},
	/// An expected network destination.
	Network {
		/// Destination description shown during installation.
		destination: String,
	},
}

/// Contents of `.jet/craft-spec.toml`, also sent as JSON during handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CraftSpecification {
	/// This document's schema version, independent of Craft wire versions.
	pub schema: ProtocolVersion,
	/// Stable Craft identity.
	pub id: String,
	/// The single Harness adapted by this Craft.
	pub harness: String,
	/// Craft protocol majors and each major's highest minor.
	pub protocol: ProtocolOffer,
	/// Harness feature declarations, never authorization grants.
	#[serde(default)]
	pub features: Vec<CraftFeature>,
	/// Required broker declarations; the host separately authorizes each use.
	#[serde(default)]
	pub broker_permissions: Vec<BrokerPermission>,
	/// Required host-access disclosures.
	#[serde(default)]
	pub host_access: Vec<CraftHostAccess>,
}

impl CraftSpecification {
	/// Validate v1 declarations and return only understood Harness features.
	///
	/// # Errors
	/// Rejects unsupported schemas, invalid identities/offers, and unknown
	/// required features. Unknown permissions and host kinds fail decoding.
	pub fn enabled_features(
		&self,
	) -> Result<Vec<String>, IncompatibleProtocol> {
		if self.schema.major != 1
			|| self.id.trim().is_empty()
			|| self.harness.trim().is_empty()
			|| self.protocol.family != ProtocolFamily::Craft
		{
			return Err(IncompatibleProtocol);
		}
		self.protocol
			.negotiate(&self.protocol, Negotiation::NewExecution)?;
		let mut enabled = Vec::new();
		for feature in &self.features {
			if ["turns", "actions", "resume"].contains(&feature.name.as_str()) {
				enabled.push(feature.name.clone());
			} else if feature.required {
				return Err(IncompatibleProtocol);
			}
		}
		enabled.sort();
		enabled.dedup();
		Ok(enabled)
	}

	/// Whether an update expands access and needs renewed user confirmation.
	/// Removals need no new consent; changed targets conservatively count as
	/// expansion. Callers compare the accepted installation, not peer claims.
	#[must_use]
	pub fn requires_confirmation(&self, accepted: &Self) -> bool {
		// ASVS 8.3.1: declarations are inputs to host authorization, not grants.
		self.broker_permissions
			.iter()
			.any(|permission| !accepted.broker_permissions.contains(permission))
			|| self
				.host_access
				.iter()
				.any(|access| !accepted.host_access.contains(access))
	}
}
