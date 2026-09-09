//! Isolated, one-shot Craft Automatic-review v1 stdin/stdout contract
//! (ADR-0012).
//!
//! A reviewer is asked about one held approval request and answers with a
//! judgement. It receives a bounded visible transcript and the exact
//! requested action, and it is given no tools, no working tree, and no way
//! to reach the Harness it is judging: the answer is data the trusted core
//! validates, never a Command.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Which reviewer a Craft used. A Harness with its own separate reviewer
/// that satisfies this contract reports `Native`; a Craft that instead runs
/// Jet's versioned equivalent through the selected Account binding reports
/// `Equivalent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CraftReviewer {
	/// The Harness's own separate reviewer.
	Native,
	/// Jet's versioned equivalent, run through the selected binding.
	Equivalent,
}

/// Content-free selection returned by `--review-model`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CraftReviewModel {
	/// Contract version, exactly one.
	pub version: u32,
	/// Exact reviewer Model, never an alias resolved after selection.
	pub model: String,
	/// Which reviewer the Craft will use for this Model.
	pub reviewer: CraftReviewer,
}

/// Everything one reviewer may see. It is Conversation content and the
/// exact requested action, with no paths, credentials, or tool definitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CraftReviewInput {
	/// Bounded visible transcript, oldest first, at most 12288 UTF-8 bytes.
	pub transcript: String,
	/// Native tool or permission name, at most 128 UTF-8 bytes.
	pub tool: String,
	/// The exact requested action, at most 4096 UTF-8 bytes.
	pub action: String,
}

/// One request to `--review`; bounded to 128 KiB including JSON escaping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CraftReviewRequest {
	/// Contract version, exactly one.
	pub version: u32,
	/// Exact selection, never replaced by a default.
	pub model: String,
	/// The one Account binding whose Credential may be resolved.
	pub binding_id: Uuid,
	/// Transport authentication reference; never model input.
	pub credential_reference: crate::CredentialReference,
	/// The entire reviewer input.
	pub input: CraftReviewInput,
}

/// One response, bounded to 64 KiB including JSON escaping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CraftReviewReply {
	/// Contract version, exactly one.
	pub version: u32,
	/// Actual Model reported by the Provider.
	pub model: String,
	/// Which reviewer produced the judgement.
	pub reviewer: CraftReviewer,
	/// Untrusted JSON, at most 4096 UTF-8 bytes, validated again by Core.
	pub output: String,
}
