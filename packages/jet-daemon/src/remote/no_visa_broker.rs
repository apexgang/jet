//! Direct origin-owned SSH broker; Craft input never supplies authority.
use crate::{
	installation_identity::Identity,
	run::{craft::Contract, host::CraftProcesses},
};
use jet_core::{CoreError, LaunchPlan, RunId};
use jet_protocol as wire;

pub(crate) struct Broker {
	identity: Identity,
	selection: jet_core::NoVisaSelection,
	run_id: RunId,
	permissions: Vec<wire::BrokerPermission>,
}
impl Broker {
	pub(crate) fn prepare(
		processes: &CraftProcesses,
		plan: &LaunchPlan,
		run_id: RunId,
	) -> Result<Option<Self>, CoreError> {
		let Some(selection) = &plan.no_visa else {
			return Ok(None);
		};
		let identity = processes.identity.as_ref().filter(|id| id.client_id == plan.client_id.0 && id.executable.is_absolute() && id.executable.is_file())
            .ok_or_else(|| conflict("no_visa.signer_unavailable", "the origin desktop installation's platform credential signer is unavailable"))?;
		let contract = Contract::of(&plan.craft)?;
		if contract.craft_protocol.minor < 6
			|| !contract
				.specification
				.enabled_features()
				.map_err(|_| invalid())?
				.iter()
				.any(|f| f == "remote_tools")
			|| !contract
				.specification
				.broker_permissions
				.contains(&wire::BrokerPermission::RemoteTools)
		{
			return Err(conflict(
				"no_visa.craft_unavailable",
				"the accepted Craft must declare remote_tools and support Craft 1.6",
			));
		}
		for destination in &selection.destinations {
			jet_client::SshEndpoint::new(&destination.ssh_endpoint)
				.map_err(|_| invalid())?;
		}
		Ok(Some(Self {
			identity: identity.clone(),
			selection: selection.clone(),
			run_id,
			permissions: contract.specification.broker_permissions,
		}))
	}
	pub(crate) fn selection(&self) -> wire::NoVisaSelection {
		selection(self.selection.clone())
	}
	pub(crate) async fn call(
		&self,
		call: wire::CraftRemoteTool,
	) -> wire::RemoteToolOutcome {
		let Some(destination) = self.selection.destinations.iter().find(|d| {
			d.plane_id.0 == call.destination_plane_id
				&& d.workspace_id.0 == call.workspace_id
		}) else {
			return failure(
				"no_visa.destination_denied",
				"the destination was not selected for this Run",
			);
		};
		let endpoint =
			match jet_client::SshEndpoint::new(&destination.ssh_endpoint) {
				Ok(endpoint) => endpoint,
				Err(_) => {
					return failure(
						"no_visa.endpoint_invalid",
						"the selected SSH endpoint is invalid",
					);
				}
			};

		let client = match tokio::time::timeout(
			std::time::Duration::from_secs(15),
			jet_client::Client::connect_ssh(&endpoint, &self.identity),
		)
		.await
		{
			Ok(Ok(client)) => client,
			Ok(Err(jet_client::ClientError::Rejected(error))) => {
				return wire::RemoteToolOutcome::Failed { error };
			}
			Ok(Err(_)) | Err(_) => {
				return failure(
					"no_visa.destination_unavailable",
					"the selected destination could not be reached or authenticated",
				);
			}
		};
		let result = tokio::time::timeout(
			std::time::Duration::from_secs(65),
			client.remote_tool(wire::RemoteToolRequest {
				operation_id: call.operation_id,
				origin: wire::NoVisaOrigin {
					plane_id: self.selection.origin_plane_id.0,
					conversation_id: self.selection.conversation_id.0,
					run_id: self.run_id.0,
				},
				destination_plane_id: destination.plane_id.0,
				workspace_id: destination.workspace_id.0,
				permissions: self.permissions.clone(),
				action: call.action,
			}),
		)
		.await;
		// No automatic retries: another destination remains independent, and a
		// lost write reply must never imply that its mutation did not occur.
		match result {
			Ok(Ok(result)) => wire::RemoteToolOutcome::Completed { result },
			Ok(Err(
				jet_client::ClientError::Remote(error)
				| jet_client::ClientError::Rejected(error),
			)) => wire::RemoteToolOutcome::Failed { error },
			Ok(Err(_)) | Err(_) => failure(
				"no_visa.outcome_unknown",
				"the destination operation could not be confirmed; inspect possible effects before repeating it",
			),
		}
	}
}
pub(crate) fn selection(
	value: jet_core::NoVisaSelection,
) -> wire::NoVisaSelection {
	wire::NoVisaSelection {
		origin_plane_id: value.origin_plane_id.0,
		account_binding_id: value.account_binding_id.0,
		conversation_id: value.conversation_id.0,
		jet_equivalent: value.jet_equivalent,
		native_unavailable: value.native_unavailable,
		destinations: value
			.destinations
			.into_iter()
			.map(|d| wire::NoVisaDestination {
				plane_id: d.plane_id.0,
				workspace_id: d.workspace_id.0,
				ssh_endpoint: d.ssh_endpoint,
			})
			.collect(),
	}
}
fn invalid() -> CoreError {
	conflict(
		"no_visa.invalid_selection",
		"the selected remote execution contract is invalid",
	)
}
fn failure(code: &str, message: &str) -> wire::RemoteToolOutcome {
	wire::RemoteToolOutcome::Failed {
		error: crate::connection::wire_error(
			wire::ErrorCategory::Conflict,
			code,
			message.into(),
		),
	}
}

fn conflict(code: &str, message: &str) -> CoreError {
	CoreError {
		category: jet_core::ErrorCategory::Conflict,
		code: code.into(),
		message: message.into(),
		retryable: false,
		detail: None,
		revision_conflict: None,
		recovery_actions: vec![],
	}
}
