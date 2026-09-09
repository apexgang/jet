//! Harness-native catalogs and deferred lifecycle requests (ADR-0039, ADR-0043).
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Native catalog data, interpreted by its Craft and rendered as data by clients.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionScope {
	/// The native Harness's user configuration on the selected Plane.
	User,
}
/// Explicit consent to the native executable boundary, never GUI code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionTrust {
	/// Skills may direct tools; hooks, MCP servers and plugins execute as the user.
	SameUserExecutable,
}
/// Exact native metadata and authority shown before any lifecycle mutation.
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

/// Trusted host for an accepted Craft's native extension operations. Catalogs
/// must remain bounded and exclude secrets. Apply must revalidate the snapshot,
/// native policy and supported action before mutating; it must never reload a Run.
pub trait ExtensionHost: std::fmt::Debug + Send + Sync {
	/// Pin the exact accepted Craft artifact before Core checks its lifecycle barriers.
	fn pin<'a>(
		&'a self,
		craft_id: &'a str,
	) -> crate::RunFuture<'a, Result<crate::PinnedCraft, crate::CoreError>>;
	/// Discover official and configured native sources without changing activation.
	fn catalog<'a>(
		&'a self,
		craft: &'a crate::PinnedCraft,
	) -> crate::RunFuture<'a, Result<ExtensionCatalog, crate::CoreError>>;
	/// Inspect a selected native extension, including source, files, components and requested access.
	fn inspect<'a>(
		&'a self,
		craft: &'a crate::PinnedCraft,
		extension_id: &'a str,
	) -> crate::RunFuture<'a, Result<ExtensionCatalog, crate::CoreError>>;
	/// Apply exactly the confirmed native operation, with no automatic retries.
	fn apply<'a>(
		&'a self,
		craft: &'a crate::PinnedCraft,
		confirmation: &'a ExtensionConfirmation,
	) -> crate::RunFuture<'a, Result<(), crate::CoreError>>;
}
