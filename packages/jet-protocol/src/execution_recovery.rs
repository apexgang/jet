//! Interactive Orphaned-execution recovery, introduced in Jet protocol minor 14.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Read-only, non-secret metadata; inspection does not authorize adoption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExecutionMetadata {
	/// Fresh helper instance identity.
	pub instance: Uuid,
	/// Helper OS identity.
	pub helper_pid: u32,
	/// Declared canonical working root.
	pub root: String,
	/// Declared Project root.
	pub project_root: String,
	/// Helper product version.
	pub version: String,
}
/// Role of an independently preserved execution.
#[derive(
	Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ExecutionRole {
	/// A Harness Run (also the role of older descriptors).
	#[default]
	Run,
	/// A Workspace terminal.
	Terminal,
}
impl ExecutionRole {
	fn is_run(&self) -> bool {
		*self == Self::Run
	}
}
/// A live execution recovery could not match safely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OrphanedExecution {
	/// Execution role; absent on pre-terminal protocol snapshots.
	#[serde(default, skip_serializing_if = "ExecutionRole::is_run")]
	pub role: ExecutionRole,
	/// Execution selected by the user.
	pub execution_id: Uuid,
	/// Owner-only metadata when safely readable.
	pub metadata: Option<ExecutionMetadata>,
}
/// Bounded page of interactive recovery candidates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OrphanedExecutions {
	/// Read-only metadata; never native output.
	pub executions: Vec<OrphanedExecution>,
	/// Continue after this identity, or finish when absent.
	pub next: Option<Uuid>,
}
/// Unknown control decisions fail closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAction {
	/// Revalidate authority, roots, identity, and offsets before attaching.
	Adopt,
	/// Preserve the execution without acknowledging source.
	Leave,
	/// Stop only the inspected helper instance and its native process.
	Terminate,
}
