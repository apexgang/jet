//! The two explicitly allowed enrollment operations on a restricted connection.

use jet_core::{
	Actor, ClientId, CommandEnvelope, CommandId, CommandOutcome, Core,
};
use jet_protocol::{RemotePairingRequest, RemotePairingResponse};

pub(super) async fn enroll(
	core: &Core,
	client_id: ClientId,
	request: RemotePairingRequest,
	minor: u32,
) -> RemotePairingResponse {
	match apply(core, client_id, request).await {
		Ok(response) => response,
		Err(error) => RemotePairingResponse::Rejected {
			error: crate::translate::error(error, minor),
		},
	}
}

async fn apply(
	core: &Core,
	client_id: ClientId,
	request: RemotePairingRequest,
) -> Result<RemotePairingResponse, jet_core::CoreError> {
	let (command_id, command) = match request {
		RemotePairingRequest::Claim {
			command_id,
			secret,
			key,
		} => (
			command_id,
			jet_protocol::CommandRequest::ClaimPairing { secret, key },
		),
		RemotePairingRequest::Complete {
			command_id,
			offer_id,
			signature,
		} => (
			command_id,
			jet_protocol::CommandRequest::CompletePairing {
				offer_id,
				signature,
			},
		),
	};
	let bytes =
		jet_protocol::encode_control(&command).map_err(|_| malformed())?;
	let envelope = CommandEnvelope::new(
		CommandId(command_id),
		crate::translate::command(&command),
		&bytes,
	)?;
	// This temporary actor can reach only the two enumerated enrollment
	// Commands above. It is never returned as connection authority.
	match core
		.execute(&Actor::InteractiveClient { client_id }, envelope)
		.await?
	{
		CommandOutcome::PairingClaimed { pending, .. } => {
			let signing_bytes = core
				.remote_pairing_signing_bytes(client_id, pending.offer_id)
				.await?;
			Ok(RemotePairingResponse::Claimed {
				pending: crate::translate::pairing_pending(pending),
				signing_bytes,
			})
		}
		CommandOutcome::PairingCompleted { client } => {
			Ok(RemotePairingResponse::Completed {
				client: crate::translate::paired_client(client),
			})
		}
		CommandOutcome::ConversationCreated(_)
		| CommandOutcome::RunCreated(_)
		| CommandOutcome::RunTransitioned(_)
		| CommandOutcome::SettingSet { .. }
		| CommandOutcome::SettingCleared { .. }
		| CommandOutcome::AccountBound(_)
		| CommandOutcome::AccountUnbound { .. }
		| CommandOutcome::PairingGateSet { .. }
		| CommandOutcome::PairingOpened { .. }
		| CommandOutcome::PairedClientAccessSet { .. }
		| CommandOutcome::PairingConfirmed { .. }
		| CommandOutcome::AuditEpochBegun { .. }
		| CommandOutcome::PairedClientRevoked { .. } => Err(malformed()),
	}
}

fn malformed() -> jet_core::CoreError {
	jet_core::CoreError {
		category: jet_core::ErrorCategory::InvalidInput,
		code: "pairing.invalid_request".into(),
		message: "invalid remote Pairing request".into(),
		retryable: false,
		detail: None,
		revision_conflict: None,
		recovery_actions: vec![],
	}
}
