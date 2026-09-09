//! Harness-native catalogs and deferred lifecycle requests (ADR-0039, ADR-0043).
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Native catalog data, interpreted by its Craft and rendered as data by clients.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionCatalog {
	/// Installed Craft identity.
	pub craft_id: String,
	/// Harness whose native formats this catalog describes.
	pub harness: String,
	/// Bounded native JSON containing sources, versions, publishers and components.
	pub native_metadata: String,
}
/// Operations supported by the responsible native adapter.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionAction {
	/// Install a native extension.
	Install,
	/// Update a native extension.
	Update,
	/// Stop loading a native extension.
	Disable,
	/// Remove a native extension.
	Remove,
}
/// Native scopes currently admitted by the Plane.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionScope {
	/// The native Harness's user configuration on the selected Plane.
	User,
}
/// Explicit consent to the native executable boundary, never GUI code.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionTrust {
	/// Skills may direct tools; hooks, MCP servers and plugins execute as the user.
	SameUserExecutable,
}
/// Exact native metadata and authority shown before any lifecycle mutation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionConfirmation {
	/// Catalog snapshot which the Craft must revalidate before execution.
	pub catalog: ExtensionCatalog,
	/// Native identity, for example plugin@marketplace.
	pub extension_id: String,
	/// Exactly one lifecycle operation.
	pub action: ExtensionAction,
	/// Native installation scope.
	pub scope: ExtensionScope,
	/// Same-user executable access accepted by the interactive client.
	pub trust: ExtensionTrust,
}
/// Durable outcome, separate from acceptance of the initiating Command.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionChangeState {
	/// Waiting for existing managed Runs to finish.
	Staged,
	/// Native lifecycle operation completed for subsequent Runs.
	Applied,
	/// Validation refused the mutation before execution.
	Refused,
	/// Execution began but its complete outcome cannot be established. Never retried.
	OutcomeUnknown,
}
/// Inspectable, content-free lifecycle progress.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionChange {
	/// Durable operation identity.
	pub change_id: Uuid,
	/// Responsible Craft.
	pub craft_id: String,
	/// Native extension identity.
	pub extension_id: String,
	/// Requested operation.
	pub action: ExtensionAction,
	/// Execution outcome.
	pub state: ExtensionChangeState,
}

/// One-shot Craft extension v1 request on owner-only stdin/stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CraftExtensionRequest {
	/// Discover a native catalog.
	Catalog,
	/// Inspect one native target before accepting executable access.
	Inspect {
		/// Exact native target.
		extension_id: String,
	},
	/// Perform one admitted lifecycle operation after Runs have finished.
	Apply {
		/// Exact native metadata and accepted access.
		confirmation: ExtensionConfirmation,
	},
}
/// Bounded reply from the responsible Craft. Native errors never cross this boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CraftExtensionReply {
	/// Native metadata, never executable GUI customization.
	Catalog {
		/// Unconverted native inventory.
		catalog: ExtensionCatalog,
	},
	/// The native operation completed.
	Applied,
	/// No native mutation was attempted.
	Refused,
}
