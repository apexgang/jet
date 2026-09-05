//! The Pairing Query and Commands (ADR-0017).
//!
//! Pairing is how a GUI client comes to control a Plane at all, so it sits
//! in its own module rather than among the Conversation, Setting, and
//! Account requests beside it.

use jet_protocol::{
	ClientPublicKey, CommandRequest, CommandResponse, PairedClient,
	PairedClientAccess, PairingDisclosure, PairingGate, PairingMethod,
	PairingSnapshot, PendingPairing, QueryRequest, QueryResponse,
};
use uuid::Uuid;

use crate::connection::{Client, ClientError};
use crate::requests::unexpected;

impl Client {
	/// Reads the Plane's Pairing with the journal cursor the snapshot was
	/// read at: whether a new GUI client may begin Pairing at all
	/// (ADR-0017).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn pairing(&self) -> Result<PairingSnapshot, ClientError> {
		self.require_minor(jet_protocol::PAIRING_MINOR)?;
		match self.query(QueryRequest::Pairing).await? {
			QueryResponse::Pairing(snapshot) => Ok(snapshot),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Projects(_)) => Err(unexpected(&other)),
		}
	}

	/// Leaves the Plane's Pairing gate at `gate` under the Command identity
	/// `command_id`, which a retry must reuse (ADR-0093).
	///
	/// The gate decides whether a new GUI client may begin Pairing. It does
	/// not alter the clients that are already Paired, in either direction
	/// (ADR-0017).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn set_pairing_gate(
		&self,
		command_id: Uuid,
		gate: PairingGate,
	) -> Result<PairingGate, ClientError> {
		self.require_minor(jet_protocol::PAIRING_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::SetPairingGate { gate },
			)
			.await?
		{
			CommandResponse::PairingGateSet { gate } => Ok(gate),
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }
			| CommandResponse::PairedClientAccessSet { .. }
			| CommandResponse::PairedClientRevoked { .. }
			| CommandResponse::ProjectRegistered(_)) => Err(unexpected(&other)),
		}
	}

	/// Issues the Plane's one Pairing offer under the Command identity
	/// `command_id`, which a retry must reuse (ADR-0093), and returns it
	/// beside the one-time secret to hand over.
	///
	/// The secret is disclosed once: a retry is answered with the same
	/// offer and [`PairingDisclosure::AlreadyDisclosed`], because the
	/// receipt that makes the retry idempotent outlives the secret
	/// (ADR-0017).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Pairing gate is closed or
	/// the endpoint is not an endpoint, or the transport failure otherwise.
	pub async fn open_pairing(
		&self,
		command_id: Uuid,
		method: PairingMethod,
	) -> Result<(PendingPairing, PairingDisclosure), ClientError> {
		self.require_minor(jet_protocol::PAIRING_MINOR)?;
		match self
			.execute_command(command_id, CommandRequest::OpenPairing { method })
			.await?
		{
			CommandResponse::PairingOpened {
				pending,
				disclosure,
			} => Ok((pending, disclosure)),
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }
			| CommandResponse::PairedClientAccessSet { .. }
			| CommandResponse::PairedClientRevoked { .. }
			| CommandResponse::ProjectRegistered(_)) => Err(unexpected(&other)),
		}
	}

	/// Claims the Plane's open Pairing offer with the secret a person
	/// presented and this installation's public key, under the Command
	/// identity `command_id`, which a retry must reuse (ADR-0093).
	///
	/// The answer carries the offer, whose progress now holds the
	/// authentication string to display, and the fresh challenge this
	/// installation's key signs to complete the Pairing (ADR-0090).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when no offer is open, the offer is
	/// over or already claimed, or the secret does not match, or the
	/// transport failure otherwise.
	pub async fn claim_pairing(
		&self,
		command_id: Uuid,
		secret: &str,
		key: ClientPublicKey,
	) -> Result<(PendingPairing, [u8; 32]), ClientError> {
		self.require_minor(jet_protocol::PAIRING_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::ClaimPairing {
					secret: secret.into(),
					key,
				},
			)
			.await?
		{
			CommandResponse::PairingClaimed { pending, challenge } => {
				Ok((pending, challenge))
			}
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }
			| CommandResponse::PairedClientAccessSet { .. }
			| CommandResponse::PairedClientRevoked { .. }
			| CommandResponse::ProjectRegistered(_)) => Err(unexpected(&other)),
		}
	}

	/// Confirms, on the Plane being Paired with, that both screens show the
	/// same authentication string, under the Command identity `command_id`,
	/// which a retry must reuse (ADR-0093).
	///
	/// The client being Paired cannot confirm its own Pairing: mutual
	/// confirmation is the step a client that answered the code from
	/// somewhere else cannot pass (ADR-0017).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the offer is not the open one,
	/// is not waiting to be confirmed, or the string does not match, or the
	/// transport failure otherwise.
	pub async fn confirm_pairing(
		&self,
		command_id: Uuid,
		offer_id: Uuid,
		authentication_string: &str,
	) -> Result<PendingPairing, ClientError> {
		self.require_minor(jet_protocol::PAIRING_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::ConfirmPairing {
					offer_id,
					authentication_string: authentication_string.into(),
				},
			)
			.await?
		{
			CommandResponse::PairingConfirmed { pending } => Ok(pending),
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingCompleted { .. }
			| CommandResponse::PairedClientAccessSet { .. }
			| CommandResponse::PairedClientRevoked { .. }
			| CommandResponse::ProjectRegistered(_)) => Err(unexpected(&other)),
		}
	}

	/// Completes the Pairing with `signature` over the transcript of this
	/// installation's claim, under the Command identity `command_id`, which
	/// a retry must reuse (ADR-0093).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the offer is not the open one,
	/// is not confirmed, belongs to another client, or the signature does
	/// not verify, or the transport failure otherwise.
	pub async fn complete_pairing(
		&self,
		command_id: Uuid,
		offer_id: Uuid,
		signature: [u8; 64],
	) -> Result<PairedClient, ClientError> {
		self.require_minor(jet_protocol::PAIRING_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::CompletePairing {
					offer_id,
					signature,
				},
			)
			.await?
		{
			CommandResponse::PairingCompleted { client } => Ok(client),
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairedClientAccessSet { .. }
			| CommandResponse::PairedClientRevoked { .. }
			| CommandResponse::ProjectRegistered(_)) => Err(unexpected(&other)),
		}
	}

	/// Stops a Paired client controlling the Plane, or lets it control the
	/// Plane again, under the Command identity `command_id`, which a retry
	/// must reuse (ADR-0093).
	///
	/// The Plane keeps the client's key either way, so a disabled client is
	/// enabled again without anybody pairing anything (ADR-0017).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Plane is not Paired with
	/// that client, or the transport failure otherwise.
	pub async fn set_paired_client_access(
		&self,
		command_id: Uuid,
		client_id: Uuid,
		access: PairedClientAccess,
	) -> Result<PairedClient, ClientError> {
		self.require_minor(jet_protocol::PAIRING_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::SetPairedClientAccess { client_id, access },
			)
			.await?
		{
			CommandResponse::PairedClientAccessSet { client } => Ok(client),
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }
			| CommandResponse::PairedClientRevoked { .. }
			| CommandResponse::ProjectRegistered(_)) => Err(unexpected(&other)),
		}
	}

	/// Forgets a Paired client and the key it was Paired with, under the
	/// Command identity `command_id`, which a retry must reuse (ADR-0093).
	///
	/// Nothing in Jet brings either back: the installation is Paired again
	/// or it does not control the Plane (ADR-0017).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Plane is not Paired with
	/// that client, or the transport failure otherwise.
	pub async fn revoke_paired_client(
		&self,
		command_id: Uuid,
		client_id: Uuid,
	) -> Result<Uuid, ClientError> {
		self.require_minor(jet_protocol::PAIRING_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::RevokePairedClient { client_id },
			)
			.await?
		{
			CommandResponse::PairedClientRevoked { client_id } => Ok(client_id),
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }
			| CommandResponse::PairedClientAccessSet { .. }
			| CommandResponse::ProjectRegistered(_)) => Err(unexpected(&other)),
		}
	}
}
