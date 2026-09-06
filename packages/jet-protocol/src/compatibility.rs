//! Independent protocol compatibility and execution pins (ADR-0019).

use serde::{Deserialize, Serialize};

/// Separately versioned contracts; sharing a codec never couples versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProtocolFamily {
	/// GUI to daemon protocol.
	Client,
	/// Harness adapter protocol.
	Craft,
	/// Execution helper protocol.
	Helper,
	/// Craft specification schema.
	Specification,
}

/// A major and its additive minor, independent of product version.
#[derive(
	Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProtocolVersion {
	/// Incompatible schema generation, starting at one.
	#[cfg_attr(feature = "schema", schemars(range(max = 4_294_967_295u64)))]
	pub major: u32,
	/// Additive schema generation, starting at zero.
	#[cfg_attr(feature = "schema", schemars(range(max = 4_294_967_295u64)))]
	pub minor: u32,
}

/// A peer's highest minor for each supported major, plus capability names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProtocolOffer {
	/// Contract to which this offer applies.
	pub family: ProtocolFamily,
	/// One entry per major; every minor from zero through this minor works.
	pub versions: Vec<ProtocolVersion>,
	/// Optional capabilities; unknown names confer no authority.
	#[serde(default)]
	pub capabilities: Vec<String>,
}

/// Whether to choose a new version or preserve a durable execution pin.
#[derive(Debug, Clone, Copy)]
pub enum Negotiation {
	/// Select the newest common major and its common minor.
	NewExecution,
	/// Require this execution's original version, including after restart.
	Resume(ProtocolVersion),
}

/// The immutable selection callers persist with the execution identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NegotiatedProtocol {
	/// Independently negotiated contract.
	pub family: ProtocolFamily,
	/// Selected version; updates must retain support while it is active.
	pub version: ProtocolVersion,
	/// Intersection of understood capability names, sorted and deduplicated.
	pub capabilities: Vec<String>,
}

/// Compatibility failure; callers must remain disabled or non-mutating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("incompatible or invalid protocol offer")]
pub struct IncompatibleProtocol;

impl ProtocolOffer {
	/// Negotiate only this family. Resuming never silently switches majors.
	///
	/// # Errors
	/// Rejects mismatched families, invalid offers, and unsupported versions.
	pub fn negotiate(
		&self,
		peer: &Self,
		mode: Negotiation,
	) -> Result<NegotiatedProtocol, IncompatibleProtocol> {
		// ASVS 2.2.1, 2.3.1: reject ambiguous offers before choosing a version.
		if self.family != peer.family || !self.valid() || !peer.valid() {
			return Err(IncompatibleProtocol);
		}
		let version = match mode {
			Negotiation::NewExecution => self
				.versions
				.iter()
				.flat_map(|local| {
					peer.versions
						.iter()
						.filter(move |remote| remote.major == local.major)
						.map(move |remote| ProtocolVersion {
							major: local.major,
							minor: local.minor.min(remote.minor),
						})
				})
				.max()
				.ok_or(IncompatibleProtocol)?,
			Negotiation::Resume(version)
				if self.supports(version) && peer.supports(version) =>
			{
				version
			}
			Negotiation::Resume(_) => return Err(IncompatibleProtocol),
		};
		let mut capabilities: Vec<_> = self
			.capabilities
			.iter()
			.filter(|name| peer.capabilities.contains(name))
			.cloned()
			.collect();
		capabilities.sort();
		capabilities.dedup();
		Ok(NegotiatedProtocol {
			family: self.family,
			version,
			capabilities,
		})
	}

	fn supports(&self, version: ProtocolVersion) -> bool {
		self.versions.iter().any(|supported| {
			supported.major == version.major && supported.minor >= version.minor
		})
	}

	fn valid(&self) -> bool {
		!self.versions.is_empty()
			&& self.versions.iter().enumerate().all(|(index, version)| {
				version.major != 0
					&& !self.versions[..index]
						.iter()
						.any(|earlier| earlier.major == version.major)
			})
	}
}
