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

pub(crate) fn snapshot(
	snapshot: CapabilitySnapshot,
	minor: u32,
) -> wire::CapabilitySnapshot {
	wire::CapabilitySnapshot {
		resource_budgets: (minor >= wire::ENERGY_MINOR).then_some(
			wire::ResourceBudgets {
				power: match snapshot.power {
					jet_runtime::PowerState::Normal => wire::PowerState::Normal,
					jet_runtime::PowerState::Constrained => {
						wire::PowerState::Constrained
					}
					jet_runtime::PowerState::Unavailable => {
						wire::PowerState::Unavailable
					}
				},
				jetd_rss_mib: 35,
				helper_rss_mib: 8,
				craft_rss_mib: 15,
				idle_cpu_millicores: 2,
				idle_window_seconds: 300,
			},
		),
		observed_at_unix_ms: unix_ms(snapshot.observed_at),
		core_version: snapshot.core_version.into(),
		platform: platform(snapshot.platform),
		// A tool introduced after the negotiated minor is left out rather
		// than sent in a shape the peer cannot read (ADR-0019).
		external_tools: snapshot
			.external_tools
			.into_iter()
			.filter(|status| introduced_in(status.tool) <= minor)
			.map(external_tool_status)
			.collect(),
		credential_store: credential_store(snapshot.credential_store),
		crafts: snapshot
			.crafts
			.into_iter()
			.map(|installed| craft(installed, minor))
			.collect(),
		harnesses: snapshot.harnesses.into_iter().map(harness).collect(),
		degraded: snapshot
			.degraded
			.into_iter()
			.filter(|condition| match condition {
				DegradedCondition::MissingExternalTool { tool } => {
					introduced_in(*tool) <= minor
				}
				DegradedCondition::NoHarnessAvailable
				| DegradedCondition::CredentialStoreUnavailable { .. }
				| DegradedCondition::CredentialStoreLocked { .. } => true,
			})
			.map(degraded_condition)
			.collect(),
	}
}

/// The protocol minor that first named each external tool.
fn introduced_in(tool: ExternalTool) -> u32 {
	match tool {
		ExternalTool::Git | ExternalTool::Ssh | ExternalTool::Tailscale => {
			wire::SETTINGS_AND_CAPABILITIES_MINOR
		}
		ExternalTool::GitLfs => wire::PROJECTS_MINOR,
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
		ExternalTool::GitLfs => wire::ExternalTool::GitLfs,
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
		CredentialStoreStatus::Locked { kind } => {
			wire::CredentialStoreStatus::Locked {
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

pub(super) fn credential_store_kind(
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

fn craft(installed: InstalledCraft, minor: u32) -> wire::InstalledCraft {
	let CraftId(craft_id) = installed.craft;
	wire::InstalledCraft {
		subagent_control: (minor >= wire::ENERGY_MINOR).then_some(
			if installed.limits_subagents {
				wire::SubagentControl::NativeLimits
			} else {
				wire::SubagentControl::MonitorOnly
			},
		),
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
		DegradedCondition::CredentialStoreLocked { kind } => {
			wire::DegradedCondition::CredentialStoreLocked {
				kind: credential_store_kind(kind),
			}
		}
	}
}
