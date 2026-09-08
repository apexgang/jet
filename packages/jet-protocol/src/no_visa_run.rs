//! Explicit origin execution with bounded tools on selected destinations.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One destination selected by the desktop, never by the Craft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct NoVisaDestination {
	/// Expected paired Plane identity.
	pub plane_id: Uuid,
	/// Registered destination Workspace.
	pub workspace_id: Uuid,
	/// System SSH destination, resolved through the user's SSH configuration.
	pub ssh_endpoint: String,
}

/// A new Run whose Craft and Harness stay on the Conversation's Home Plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct NoVisaRunRequest {
	/// Origin-owned Conversation.
	pub conversation_id: Uuid,
	/// Expected Home Plane, where credentials and native processes stay.
	pub origin_plane_id: Uuid,
	/// Origin-local native Account binding.
	pub account_binding_id: Uuid,
	/// Accepted installed Craft with remote_tools support.
	pub craft: String,
	/// Initial input.
	pub prompt: String,
	/// One to eight explicit destinations, pinned for this Run.
	pub destinations: Vec<NoVisaDestination>,
}

/// Immutable origin and destination selection retained with the execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NoVisaSelection {
	/// Jet-provided equivalents, subject to destination authority and bounds.
	pub jet_equivalent: Vec<String>,
	/// Native remote behavior that this mode cannot promise.
	pub native_unavailable: Vec<String>,
	/// Native execution's Home Plane.
	pub origin_plane_id: Uuid,
	/// Native authentication stays on this Plane.
	pub account_binding_id: Uuid,
	/// Authoritative Conversation on the origin.
	pub conversation_id: Uuid,
	/// Destinations the broker may contact.
	pub destinations: Vec<NoVisaDestination>,
}
