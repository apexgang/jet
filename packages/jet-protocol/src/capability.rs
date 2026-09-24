//! Wire form of Plane Capability snapshots (ADR-0086).

use serde::{Deserialize, Serialize};

/// Which snapshot a Capability Query answers with. A Plane is observed at
/// startup and whenever a caller asks for a new look, never on a timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CapabilityObservation {
	/// The snapshot the Plane observed most recently.
	LastObserved,
	/// A new observation, taken now and kept as the Plane's latest.
	Fresh,
}

/// A point-in-time report of what a Plane can do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CapabilitySnapshot {
	/// Reference resource targets; absent before Jet 1.33.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub resource_budgets: Option<crate::ResourceBudgets>,
	/// When the Plane was observed, in signed Unix milliseconds.
	pub observed_at_unix_ms: i64,
	/// Version of the core that observed it.
	pub core_version: String,
	/// The operating system the Plane runs.
	pub platform: Platform,
	/// Every external tool the core knows how to invoke.
	pub external_tools: Vec<ExternalToolStatus>,
	/// Whether credentials can be resolved without storing secrets.
	pub credential_store: CredentialStoreStatus,
	/// The Crafts installed on the Plane.
	pub crafts: Vec<InstalledCraft>,
	/// Every Harness an installed Craft adapts.
	pub harnesses: Vec<String>,
	/// What the Plane cannot do in this state.
	pub degraded: Vec<DegradedCondition>,
}

/// The operating system and processor a Plane runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Platform {
	/// Operating system name, such as `linux` or `macos`.
	pub operating_system: String,
	/// Processor architecture, such as `aarch64`.
	pub architecture: String,
}

/// A command-line tool the core invokes but never bundles or installs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ExternalTool {
	/// Git, which every Workspace and Change checkpoint rests on.
	Git,
	/// Git LFS, reported so a Project that tracks large files knows whether
	/// the Plane can handle them. Named from protocol minor 8, spelled as
	/// the program the Plane looks for.
	#[serde(rename = "git-lfs")]
	GitLfs,
	/// OpenSSH, which No-Visa Runs reach paired Planes through.
	Ssh,
	/// Tailscale, which discovers and reaches Planes across networks.
	Tailscale,
}

/// Whether one external tool answered, and with which version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExternalToolStatus {
	/// The tool that was looked for.
	pub tool: ExternalTool,
	/// What looking for it found.
	pub availability: ToolAvailability,
}

/// The result of looking for one external tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolAvailability {
	/// The tool answered with the version line it reports.
	Present {
		/// The tool's own version line, bounded and unparsed.
		version: String,
	},
	/// The tool could not be run at all.
	Missing,
}

/// The platform facility that resolves Credential references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CredentialStoreKind {
	/// The macOS Keychain.
	AppleKeychain,
	/// The freedesktop Secret Service, reached over the session bus.
	SecretService,
}

/// Whether the platform credential store can be reached, and whether it
/// will answer. Jet never falls back to plaintext, so each of these is
/// reported rather than worked around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CredentialStoreStatus {
	/// The store is present and can be asked for credentials.
	Available {
		/// Which store the Plane resolves through.
		kind: CredentialStoreKind,
	},
	/// The store is present but locked: it answers nothing until the user
	/// unlocks it through the operating system. `jetd` never asks for that
	/// itself, so work that needs a Credential waits instead.
	Locked {
		/// Which store is locked.
		kind: CredentialStoreKind,
	},
	/// The store cannot be reached on this Plane right now.
	Unavailable {
		/// Which store was expected.
		kind: CredentialStoreKind,
	},
}

/// The result of proving that the platform credential store holds a
/// Credential: one probe item created, read back, and deleted. Durable
/// Pairing waits for this, and the probe never prompts: a store that would
/// need the user is reported as locked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CredentialStoreVerification {
	/// The store created the probe item, returned it unchanged, and
	/// deleted it.
	Verified {
		/// Which store did.
		kind: CredentialStoreKind,
	},
	/// The store is locked, so it holds nothing until the user unlocks it
	/// through the operating system. Nothing was created.
	Locked {
		/// Which store is locked.
		kind: CredentialStoreKind,
	},
	/// The store could not be reached. Nothing was created.
	Unavailable {
		/// Which store was expected.
		kind: CredentialStoreKind,
	},
	/// The store answered but did not complete `step`.
	Failed {
		/// Which store answered.
		kind: CredentialStoreKind,
		/// The step of the round trip it did not complete.
		step: CredentialProbeStep,
	},
}

/// One step of the create/read/delete round trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CredentialProbeStep {
	/// Storing the probe item.
	Create,
	/// Reading it back unchanged.
	Read,
	/// Deleting it again.
	Delete,
}

/// One Craft installed on the Plane and the Harnesses it adapts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InstalledCraft {
	/// Native child-admission support; absent before Jet 1.33.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub subagent_control: Option<crate::SubagentControl>,
	/// The Craft's identity.
	pub craft_id: String,
	/// The version its specification declares.
	pub version: String,
	/// The Harnesses it teaches this Plane to orchestrate.
	pub harnesses: Vec<String>,
}

/// Something a Plane cannot do in its current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "condition", rename_all = "snake_case")]
pub enum DegradedCondition {
	/// An external tool ordinary work needs is not installed.
	MissingExternalTool {
		/// The tool the core could not run.
		tool: ExternalTool,
	},
	/// No Craft is installed, so the Plane can orchestrate no Harness.
	NoHarnessAvailable,
	/// Credentials cannot be resolved, so no Account binding can be used.
	CredentialStoreUnavailable {
		/// The store that was expected.
		kind: CredentialStoreKind,
	},
	/// The credential store is locked, so Credentials resolve only after
	/// the user unlocks it through the operating system.
	CredentialStoreLocked {
		/// The store that is locked.
		kind: CredentialStoreKind,
	},
}
