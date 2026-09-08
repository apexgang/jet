//! Versioned Conversation and Run name data (ADR-0044).

use serde::{Deserialize, Serialize};

/// Authority that supplied a current Conversation or Run name.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameSource {
	/// An interactive user's authoritative choice.
	Manual,
	/// A validated Utility-model result.
	Utility,
	/// A structured title supplied by the Harness through its Craft.
	HarnessNative,
	/// Stable local text used without Utility work.
	Deterministic,
}

/// One resolved user-facing name and its authority.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Name {
	/// Original validated text; clients encode it for their render context.
	pub value: String,
	/// Authority that supplied `value`.
	pub source: NameSource,
}
