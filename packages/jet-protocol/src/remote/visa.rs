//! Explicit destination selection for native Visa execution (ADR-0062).
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A Visa Run executes on its Conversation's Home Plane. Selecting another
/// Plane requires a separate, explicit Plane transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct VisaRunRequest {
	/// Conversation already owned by the selected destination.
	pub conversation_id: Uuid,
	/// Expected Home Plane; prevents launching against the wrong connection.
	pub destination_plane_id: Uuid,
	/// Account binding on that Plane; never a credential copied from the client.
	pub account_binding_id: Uuid,
	/// Installed Craft identity, never an executable path.
	pub craft: String,
	/// Initial Harness input.
	pub prompt: String,
}

/// The immutable selection used by one admitted Visa Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct VisaSelection {
	/// Home Plane on which its native processes execute.
	pub plane_id: Uuid,
	/// Binding in that Plane's own store.
	pub account_binding_id: Uuid,
}
