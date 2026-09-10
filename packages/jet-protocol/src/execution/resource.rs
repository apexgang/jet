//! Resource and native child-admission capabilities, introduced in Jet 1.33.
use serde::{Deserialize, Serialize};

/// Operating-system power observation. Unknown state uses the reduced budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PowerState {
	/// External power, without reported low-power mode.
	Normal,
	/// Battery power or low-power mode.
	Constrained,
	/// This platform observation could not be established.
	Unavailable,
}

/// Whether a Craft can prevent new native child work without stopping active work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SubagentControl {
	/// Native events remain visible, but Jet cannot enforce native child limits.
	MonitorOnly,
	/// The accepted Craft supports admission-only child controls.
	NativeLimits,
}

/// Fixed reference targets, reported as budgets rather than measured usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ResourceBudgets {
	/// Most recent on-demand operating-system observation.
	pub power: PowerState,
	/// Resident memory with ten thousand Conversations.
	pub jetd_rss_mib: u32,
	/// Helper resident memory, excluding its child.
	pub helper_rss_mib: u32,
	/// Resident memory per idle Craft.
	pub craft_rss_mib: u32,
	/// Whole idle core CPU must stay below this many thousandths of one CPU.
	pub idle_cpu_millicores: u32,
	/// Required idle CPU measurement window.
	pub idle_window_seconds: u32,
}
