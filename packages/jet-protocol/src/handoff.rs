//! Explicit Handoff input for client protocol 1.24 (ADR-0021).
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Selected context; the daemon captures files, current diff, and provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HandoffRequest {
	/// Managed source Run.
	pub source_run_id: Uuid,
	/// Installed destination Craft for a different Harness.
	pub craft: String,
	/// Selected summary; empty selects none. Maximum 8 KiB.
	pub summary: String,
	/// Selected plan; empty selects none. Maximum 8 KiB.
	pub plan: String,
	/// At most 16 distinct relative regular-file paths, each at most 8 KiB.
	pub files: Vec<String>,
}
