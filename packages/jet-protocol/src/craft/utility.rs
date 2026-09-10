//! Isolated, one-shot Craft Utility v1 stdin/stdout contract (ADR-0099).
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Allowlisted content; authentication and job metadata are never part of this input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "purpose", rename_all = "snake_case", deny_unknown_fields)]
pub enum UtilityInput {
	/// Naming context, excluding native output.
	Naming {
		/// At most 256 UTF-8 bytes.
		title: String,
		/// At most 4096 UTF-8 bytes.
		opening_context: String,
	},
	/// One immutable turn checkpoint.
	GitText {
		/// At most 16384 UTF-8 bytes.
		patch: String,
		/// At most 2048 UTF-8 bytes of Plane guidance.
		instructions: String,
	},
	/// Unapproved natural-language rule.
	Autodelete {
		/// At most 4096 UTF-8 bytes.
		prompt: String,
	},
}
/// Content-free selection returned by `--utility-model`.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CraftUtilityModel {
	/// Contract version, exactly one.
	pub version: u32,
	/// Exact Model at minimum reasoning effort.
	pub model: String,
}
/// One request to `--utility`; bounded to 128 KiB including JSON escaping.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CraftUtilityRequest {
	/// Contract version, exactly one.
	pub version: u32,
	/// Exact selection, never replaced by a default.
	pub model: String,
	/// The one Account binding whose Credential may be resolved.
	pub binding_id: Uuid,
	/// Transport authentication reference; never model input.
	pub credential_reference: crate::CredentialReference,
	/// The entire model input, with no tools, paths, or native output.
	pub input: UtilityInput,
}
/// One response, bounded to 64 KiB including JSON escaping.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CraftUtilityReply {
	/// Contract version, exactly one.
	pub version: u32,
	/// Actual Model reported by the Provider.
	pub model: String,
	/// Untrusted JSON, at most 8192 UTF-8 bytes, validated again by Core.
	pub output: String,
}
