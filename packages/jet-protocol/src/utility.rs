//! Utility requests and results contain data, never Commands (ADR-0099).
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Closed set of Utility purposes, used for durable attribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum UtilityPurpose {
	/// Conversation or Run naming.
	Naming,
	/// Git commit or pull-request text.
	GitText,
	/// Unapproved natural-language rule compilation.
	Autodelete,
}

/// One purpose-specific request for Jet-owned inference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "purpose", rename_all = "snake_case", deny_unknown_fields)]
pub enum UtilityRequest {
	/// Suggest a Run name using only its title and opening input.
	Naming {
		/// Owning Run on this Plane.
		run_id: Uuid,
	},
	/// Draft Git commit or pull-request text from one retained turn.
	GitText {
		/// Owning Run.
		run_id: Uuid,
		/// Completed or interrupted turn, starting at one.
		turn: u32,
	},
	/// Compile only this rule prompt into an unapproved draft.
	Autodelete {
		/// Rule prompt, bounded to 4096 UTF-8 bytes.
		prompt: String,
	},
}
/// The exact policy used by a job. Version 1 means smallest suitable Model,
/// minimum reasoning, one inference request, no tools, and bounded input/output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UtilityPolicy {
	/// Version of the allowlists and limits.
	pub version: u32,
	/// Whether the purpose was enabled.
	pub enabled: bool,
	/// Persistent consent to send content outside its originating Provider.
	pub cross_provider_consent: bool,
}
/// A Utility result carries no execution or deletion authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum UtilityOutcome {
	/// Durable admission, awaiting a single inference attempt.
	Pending,
	/// A draft inactivity predicate. It cannot execute until reviewed separately.
	Draft {
		/// Whole days of inactivity, from 1 through 36500.
		inactive_days: u32,
	},
	/// Validated text, or a deterministic local fallback. Never executed.
	Text {
		/// Name or Git subject.
		text: String,
		/// Git body; empty for naming.
		body: String,
		/// Content-free fallback explanation; absent for inference.
		fallback_reason: Option<String>,
	},
	/// Compilation did not produce a usable draft.
	Refused {
		/// Stable, content-free reason.
		reason: String,
	},
}
/// Durable Utility result and its complete routing and policy attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UtilityJob {
	/// Plane-assigned request identity.
	pub job_id: Uuid,
	/// The only Plane authorized to execute this request.
	pub plane_id: Uuid,
	/// Purpose, independent of the generated data.
	pub purpose: UtilityPurpose,
	/// Exact selected Provider; absent when no usable binding was configured.
	pub provider: Option<String>,
	/// Explicit selected binding, including one that later became unavailable.
	pub binding_id: Option<Uuid>,
	/// Exact selected Model; absent when selection was unavailable.
	pub model: Option<String>,
	/// Policy at admission and the consent it recorded.
	pub policy: UtilityPolicy,
	/// Validated data or a closed failure.
	pub outcome: UtilityOutcome,
}
