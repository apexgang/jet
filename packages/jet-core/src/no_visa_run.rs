//! Explicit origin execution with bounded tools on selected destinations.
use crate::{AccountBindingId, ConversationId, PlaneId, WorkspaceId};
use serde::{Deserialize, Serialize};

/// One destination selected by the desktop, never by the Craft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoVisaDestination {
	/// Expected paired Plane identity.
	pub plane_id: PlaneId,
	/// Registered destination Workspace.
	pub workspace_id: WorkspaceId,
	/// System SSH destination, resolved through the user's SSH configuration.
	pub ssh_endpoint: String,
}

/// A new Run whose Craft and Harness stay on the Conversation's Home Plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoVisaRunRequest {
	/// Origin-owned Conversation.
	pub conversation_id: ConversationId,
	/// Expected Home Plane, where credentials and native processes stay.
	pub origin_plane_id: PlaneId,
	/// Origin-local native Account binding.
	pub account_binding_id: AccountBindingId,
	/// Accepted installed Craft with remote_tools support.
	pub craft: String,
	/// Initial input.
	pub prompt: String,
	/// One to eight explicit destinations, pinned for this Run.
	pub destinations: Vec<NoVisaDestination>,
}

/// Immutable origin and destination selection retained with the execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoVisaSelection {
	/// Jet-provided equivalents, subject to destination authority and bounds.
	pub jet_equivalent: Vec<String>,
	/// Native remote behavior that this mode cannot promise.
	pub native_unavailable: Vec<String>,
	/// Native execution's Home Plane.
	pub origin_plane_id: PlaneId,
	/// Native authentication stays on this Plane.
	pub account_binding_id: AccountBindingId,
	/// Authoritative Conversation on the origin.
	pub conversation_id: ConversationId,
	/// Destinations the broker may contact.
	pub destinations: Vec<NoVisaDestination>,
}

/// Admission uses origin-local credentials and never transfers Conversation authority.
pub(crate) async fn prepare(
	core: &crate::Core,
	actor: &crate::Actor,
	request: &NoVisaRunRequest,
) -> Result<crate::LaunchPlan, crate::CoreError> {
	if !matches!(actor, crate::Actor::InteractiveClient { .. }) {
		return Err(crate::CoreError::conflict(
			"no_visa.desktop_required",
			"No-Visa Runs must be started through the origin desktop installation",
		));
	}
	if request.destinations.is_empty()
		|| request.destinations.len() > 8
		|| request.destinations.iter().enumerate().any(|(i, d)| {
			d.plane_id == request.origin_plane_id
				|| request.destinations[..i].iter().any(|p| {
					p.plane_id == d.plane_id && p.workspace_id == d.workspace_id
				})
		}) {
		return Err(crate::CoreError::invalid_input(
			"no_visa.invalid_destinations",
			"select one to eight distinct remote Plane and Workspace pairs",
		));
	}
	let mut plan = crate::visa::prepare(
		core,
		actor,
		&crate::VisaRunRequest {
			conversation_id: request.conversation_id,
			destination_plane_id: request.origin_plane_id,
			account_binding_id: request.account_binding_id,
			craft: request.craft.clone(),
			prompt: request.prompt.clone(),
		},
	)
	.await?;
	plan.no_visa = Some(NoVisaSelection {
		jet_equivalent: ["files", "git", "processes", "bounded_terminals"]
			.map(str::to_owned)
			.to_vec(),
		native_unavailable: [
			"remote_checkpoints",
			"tool_discovery",
			"extensions",
			"sandbox_internals",
			"persistent_terminals",
		]
		.map(str::to_owned)
		.to_vec(),
		origin_plane_id: request.origin_plane_id,
		account_binding_id: request.account_binding_id,
		conversation_id: request.conversation_id,
		destinations: request.destinations.clone(),
	});
	plan.version = 3;
	core.run_host
		.as_ref()
		.expect("Visa preparation validated host")
		.validate_no_visa(&plan)?;
	Ok(plan)
}
