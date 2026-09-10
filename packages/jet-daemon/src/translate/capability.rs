//! The Capability half of the translation seam (ADR-0049, ADR-0086).

use super::unix_ms;
use jet_core::{
	CapabilityObservation, CapabilitySnapshot, CraftId, CredentialStoreKind,
	CredentialStoreStatus, DegradedCondition, ExternalTool, ExternalToolStatus,
	HarnessId, InstalledCraft, Platform, ToolAvailability,
};
use jet_protocol as wire;

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

#[cfg(test)]
mod tests {

	//! A peer negotiated to a lower minor never sees what that minor does not
	//! name (ADR-0019). The rule lives at this seam, so it is pinned here.

	use std::time::{Duration, UNIX_EPOCH};

	use jet_core::{
		CapabilitySnapshot, CredentialStoreKind, CredentialStoreStatus,
		ExternalTool, ExternalToolStatus, Platform, ResolvedSetting,
		SettingKey, SettingSource, SettingValue, ToolAvailability,
	};
	use jet_protocol as wire;
	use pretty_assertions::assert_eq;

	use crate::translate::{capability, setting};

	use crate::translate::test_support::resolved;
	/// A Plane observed once, with every tool the core looks for missing.
	fn observed() -> CapabilitySnapshot {
		CapabilitySnapshot {
			power: jet_runtime::PowerState::Normal,
			observed_at: UNIX_EPOCH + Duration::from_secs(1),
			core_version: "0.2.0",
			platform: Platform {
				operating_system: "linux",
				architecture: "aarch64",
			},
			external_tools: [
				ExternalTool::Git,
				ExternalTool::GitLfs,
				ExternalTool::Ssh,
				ExternalTool::Tailscale,
			]
			.into_iter()
			.map(|tool| ExternalToolStatus {
				tool,
				availability: ToolAvailability::Missing,
			})
			.collect(),
			credential_store: CredentialStoreStatus::Available {
				kind: CredentialStoreKind::SecretService,
			},
			crafts: vec![],
			harnesses: vec![],
			degraded: vec![],
		}
	}

	fn tools(snapshot: wire::CapabilitySnapshot) -> Vec<wire::ExternalTool> {
		snapshot
			.external_tools
			.into_iter()
			.map(|status| status.tool)
			.collect()
	}

	/// An enum variant is not a field an older reader can skip, so a tool the
	/// negotiated minor does not name is left out of the snapshot (ADR-0019).
	#[test]
	fn a_tool_is_reported_only_to_a_minor_that_names_it() {
		assert_eq!(
			(
				tools(capability::snapshot(
					observed(),
					wire::PROJECTS_MINOR - 1
				)),
				tools(capability::snapshot(observed(), wire::PROJECTS_MINOR)),
			),
			(
				vec![
					wire::ExternalTool::Git,
					wire::ExternalTool::Ssh,
					wire::ExternalTool::Tailscale
				],
				vec![
					wire::ExternalTool::Git,
					wire::ExternalTool::GitLfs,
					wire::ExternalTool::Ssh,
					wire::ExternalTool::Tailscale
				],
			)
		);
	}

	#[test]
	fn energy_reporting_requires_the_negotiated_minor() {
		let mut source = observed();
		source.crafts = vec![jet_core::InstalledCraft {
			limits_subagents: true,
			craft: jet_core::CraftId("native".into()),
			version: "1".into(),
			harnesses: vec![jet_core::HarnessId("native".into())],
		}];
		let old = serde_json::to_value(capability::snapshot(
			source.clone(),
			wire::ENERGY_MINOR - 1,
		))
		.unwrap();
		let new = serde_json::to_value(capability::snapshot(
			source,
			wire::ENERGY_MINOR,
		))
		.unwrap();
		assert!(old.get("resource_budgets").is_none());
		assert!(old["crafts"][0].get("subagent_control").is_none());
		assert_eq!(new["crafts"][0]["subagent_control"], "native_limits");
		let mut source = resolved();
		source.settings.push(ResolvedSetting {
			key: SettingKey::EnergyForegroundOverride,
			value: SettingValue::Flag(true),
			source: SettingSource::BuiltIn,
		});
		let old = setting::snapshot(source.clone(), wire::ENERGY_MINOR - 1);
		let new = setting::snapshot(source, wire::ENERGY_MINOR);
		assert_eq!(new.settings.len(), old.settings.len() + 1);
	}
}
