//! The Capability half of the translation seam (ADR-0049, ADR-0086).

use jet_core::{
	CapabilityObservation, CapabilitySnapshot, CraftId, CredentialStoreKind,
	CredentialStoreStatus, DegradedCondition, ExternalTool, ExternalToolStatus,
	HarnessId, InstalledCraft, Platform, ToolAvailability,
};
use jet_protocol as wire;

use super::unix_ms;

pub(super) fn observation(
	observation: wire::CapabilityObservation,
) -> CapabilityObservation {
	match observation {
		wire::CapabilityObservation::LastObserved => {
			CapabilityObservation::LastObserved
		}
		wire::CapabilityObservation::Fresh => CapabilityObservation::Fresh,
	}
}

pub(super) fn snapshot(
	snapshot: CapabilitySnapshot,
) -> wire::CapabilitySnapshot {
	wire::CapabilitySnapshot {
		observed_at_unix_ms: unix_ms(snapshot.observed_at),
		core_version: snapshot.core_version.into(),
		platform: platform(snapshot.platform),
		external_tools: snapshot
			.external_tools
			.into_iter()
			.map(external_tool_status)
			.collect(),
		credential_store: credential_store(snapshot.credential_store),
		crafts: snapshot.crafts.into_iter().map(craft).collect(),
		harnesses: snapshot.harnesses.into_iter().map(harness).collect(),
		degraded: snapshot
			.degraded
			.into_iter()
			.map(degraded_condition)
			.collect(),
	}
}

fn platform(platform: Platform) -> wire::Platform {
	wire::Platform {
		operating_system: platform.operating_system.into(),
		architecture: platform.architecture.into(),
	}
}

fn external_tool_status(
	status: ExternalToolStatus,
) -> wire::ExternalToolStatus {
	wire::ExternalToolStatus {
		tool: external_tool(status.tool),
		availability: match status.availability {
			ToolAvailability::Present { version } => {
				wire::ToolAvailability::Present { version }
			}
			ToolAvailability::Missing => wire::ToolAvailability::Missing,
		},
	}
}

fn external_tool(tool: ExternalTool) -> wire::ExternalTool {
	match tool {
		ExternalTool::Git => wire::ExternalTool::Git,
		ExternalTool::Ssh => wire::ExternalTool::Ssh,
		ExternalTool::Tailscale => wire::ExternalTool::Tailscale,
	}
}

fn credential_store(
	store: CredentialStoreStatus,
) -> wire::CredentialStoreStatus {
	match store {
		CredentialStoreStatus::Available { kind } => {
			wire::CredentialStoreStatus::Available {
				kind: credential_store_kind(kind),
			}
		}
		CredentialStoreStatus::Unavailable { kind } => {
			wire::CredentialStoreStatus::Unavailable {
				kind: credential_store_kind(kind),
			}
		}
	}
}

fn credential_store_kind(
	kind: CredentialStoreKind,
) -> wire::CredentialStoreKind {
	match kind {
		CredentialStoreKind::AppleKeychain => {
			wire::CredentialStoreKind::AppleKeychain
		}
		CredentialStoreKind::SecretService => {
			wire::CredentialStoreKind::SecretService
		}
	}
}

fn craft(installed: InstalledCraft) -> wire::InstalledCraft {
	let CraftId(craft_id) = installed.craft;
	wire::InstalledCraft {
		craft_id,
		version: installed.version,
		harnesses: installed.harnesses.into_iter().map(harness).collect(),
	}
}

fn harness(harness: HarnessId) -> String {
	harness.0
}

fn degraded_condition(condition: DegradedCondition) -> wire::DegradedCondition {
	match condition {
		DegradedCondition::MissingExternalTool { tool } => {
			wire::DegradedCondition::MissingExternalTool {
				tool: external_tool(tool),
			}
		}
		DegradedCondition::NoHarnessAvailable => {
			wire::DegradedCondition::NoHarnessAvailable
		}
		DegradedCondition::CredentialStoreUnavailable { kind } => {
			wire::DegradedCondition::CredentialStoreUnavailable {
				kind: credential_store_kind(kind),
			}
		}
	}
}
