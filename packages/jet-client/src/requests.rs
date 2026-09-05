//! The Queries and Commands a client can issue once connected.

use jet_protocol::{
	AccountBinding, AccountBindingList, CapabilityObservation,
	CapabilitySnapshot, ClientPublicKey, CommandRequest, CommandResponse,
	Conversation, ConversationList, ConversationSnapshot, CredentialReference,
	CredentialSource, EventPage, PageCursor, PairedClient, PairingDisclosure,
	PairingGate, PairingMethod, PairingSnapshot, PendingPairing, PlaneStatus,
	QueryRequest, QueryResponse, RetentionPolicy, Run, RunLifecycle,
	SecurityAudit, SettingKey, SettingScope, SettingSelection, SettingSnapshot,
	SettingValue,
};
use uuid::Uuid;

use crate::connection::{Client, ClientError};

impl Client {
	/// Runs the status Query and returns the Plane status snapshot.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn status(&self) -> Result<PlaneStatus, ClientError> {
		match self.query(QueryRequest::Status).await? {
			QueryResponse::Status(status)
				if self.negotiated_minor()
					>= jet_protocol::FENCED_READS_MINOR
					&& status.cursor.is_none() =>
			{
				Err(ClientError::Unexpected(
					"jetd omitted the negotiated status fence".into(),
				))
			}
			QueryResponse::Status(status) => Ok(status),
			other @ (QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)) => Err(unexpected(&other)),
		}
	}

	/// Reads the first bounded page of Conversations with its journal fence.
	/// A minor-zero daemon instead returns its legacy complete list.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn conversations(&self) -> Result<ConversationList, ClientError> {
		match self.query(QueryRequest::Conversations).await? {
			QueryResponse::Conversations(list) => Ok(list),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)) => Err(unexpected(&other)),
		}
	}

	/// Continues a Conversation keyset snapshot from an opaque cursor
	/// returned by [`Client::conversations`] or this method.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] with restart metadata when the cursor
	/// expired or the snapshot changed, or the transport failure otherwise.
	pub async fn next_conversations(
		&self,
		cursor: PageCursor,
	) -> Result<ConversationList, ClientError> {
		self.require_minor(jet_protocol::FENCED_READS_MINOR)?;
		match self
			.query(QueryRequest::NextConversations { cursor })
			.await?
		{
			QueryResponse::Conversations(list) => Ok(list),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)) => Err(unexpected(&other)),
		}
	}

	/// Reads one Conversation with all of its Runs and the journal cursor
	/// the snapshot was read at.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Conversation does not exist
	/// or the daemon reports another stable error, or the transport failure
	/// otherwise.
	pub async fn conversation(
		&self,
		conversation_id: Uuid,
	) -> Result<ConversationSnapshot, ClientError> {
		match self
			.query(QueryRequest::Conversation { conversation_id })
			.await?
		{
			QueryResponse::Conversation(snapshot) => Ok(snapshot),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)) => Err(unexpected(&other)),
		}
	}

	/// Reads one page of journal Events strictly after `sequence`; zero
	/// starts from the beginning of the journal. The page's cursor tells
	/// whether later pages exist (ADR-0092).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn events_after(
		&self,
		sequence: u64,
	) -> Result<EventPage, ClientError> {
		match self.query(QueryRequest::Events { after: sequence }).await? {
			QueryResponse::Events(page) => Ok(page),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Settings(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)) => Err(unexpected(&other)),
		}
	}

	/// Begins a new authority epoch of the Security audit under the Command
	/// identity `command_id`, which a retry must reuse (ADR-0093). It is
	/// refused unless the Plane is in Security-degraded mode (ADR-0105).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn begin_audit_epoch(
		&self,
		command_id: Uuid,
	) -> Result<u64, ClientError> {
		match self
			.execute_command(command_id, CommandRequest::BeginAuditEpoch)
			.await?
		{
			CommandResponse::AuditEpochBegun { epoch } => Ok(epoch),
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }) => Err(unexpected(&other)),
		}
	}

	/// Reads one page of the owner-only Security audit strictly after
	/// `sequence`; zero starts from the oldest retained record. The page's
	/// cursor tells whether later pages exist (ADR-0105).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn security_audit_after(
		&self,
		sequence: u64,
	) -> Result<SecurityAudit, ClientError> {
		match self
			.query(QueryRequest::SecurityAudit { after: sequence })
			.await?
		{
			QueryResponse::SecurityAudit(page) => Ok(page),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::Pairing(_)) => Err(unexpected(&other)),
		}
	}

	/// Creates a Conversation with no Runs under the Command identity
	/// `command_id`, which a retry must reuse (ADR-0093).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn create_conversation(
		&self,
		command_id: Uuid,
		retention: RetentionPolicy,
	) -> Result<Conversation, ClientError> {
		match self
			.execute_command(
				command_id,
				CommandRequest::CreateConversation { retention },
			)
			.await?
		{
			CommandResponse::ConversationCreated(conversation) => {
				Ok(conversation)
			}
			other @ (CommandResponse::RunCreated(_)
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
			| CommandResponse::PairingCompleted { .. }) => Err(unexpected(&other)),
		}
	}

	/// Records a new Run of a Conversation that has no live Run under the
	/// Command identity `command_id`, which a retry must reuse (ADR-0093).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Conversation does not exist
	/// or already has a live Run, or the transport failure otherwise.
	pub async fn create_run(
		&self,
		command_id: Uuid,
		conversation_id: Uuid,
	) -> Result<Run, ClientError> {
		match self
			.execute_command(
				command_id,
				CommandRequest::CreateRun { conversation_id },
			)
			.await?
		{
			CommandResponse::RunCreated(run) => Ok(run),
			other @ (CommandResponse::ConversationCreated(_)
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
			| CommandResponse::PairingCompleted { .. }) => Err(unexpected(&other)),
		}
	}

	/// Moves a Run forward to `lifecycle` if its current Revision is
	/// `expected_revision`, under the Command identity `command_id`, which a
	/// retry must reuse (ADR-0093).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Run does not exist or the
	/// transition is not allowed, or the transport failure otherwise.
	pub async fn transition_run(
		&self,
		command_id: Uuid,
		run_id: Uuid,
		expected_revision: u64,
		lifecycle: RunLifecycle,
	) -> Result<Run, ClientError> {
		match self
			.execute_command(
				command_id,
				CommandRequest::TransitionRun {
					run_id,
					expected_revision,
					lifecycle,
				},
			)
			.await?
		{
			CommandResponse::RunTransitioned(run) => Ok(run),
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }) => Err(unexpected(&other)),
		}
	}

	/// Resolves Settings for `scope`, the scope's own values winning over
	/// the Plane's and built-in defaults beneath both (ADR-0085).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the scope may not store a named
	/// Setting or the daemon reports another stable error, or the transport
	/// failure otherwise.
	pub async fn settings(
		&self,
		scope: SettingScope,
		selection: SettingSelection,
	) -> Result<SettingSnapshot, ClientError> {
		self.require_minor(jet_protocol::SETTINGS_AND_CAPABILITIES_MINOR)?;
		match self
			.query(QueryRequest::Settings { scope, selection })
			.await?
		{
			QueryResponse::Settings(snapshot) => Ok(snapshot),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)) => Err(unexpected(&other)),
		}
	}

	/// Stores `value` for `key` at `scope` under the Command identity
	/// `command_id`, which a retry must reuse (ADR-0093).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the scope may not store the
	/// Setting or the value does not fit it, or the transport failure
	/// otherwise.
	pub async fn set_setting(
		&self,
		command_id: Uuid,
		key: SettingKey,
		scope: SettingScope,
		value: SettingValue,
	) -> Result<SettingValue, ClientError> {
		self.require_minor(jet_protocol::SETTINGS_AND_CAPABILITIES_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::SetSetting { key, scope, value },
			)
			.await?
		{
			CommandResponse::SettingSet { value, .. } => Ok(value),
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }) => Err(unexpected(&other)),
		}
	}

	/// Removes whatever `scope` stores for `key`, leaving the scopes above
	/// it untouched, under the Command identity `command_id`.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the scope may not store the
	/// Setting, or the transport failure otherwise.
	pub async fn clear_setting(
		&self,
		command_id: Uuid,
		key: SettingKey,
		scope: SettingScope,
	) -> Result<(), ClientError> {
		self.require_minor(jet_protocol::SETTINGS_AND_CAPABILITIES_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::ClearSetting { key, scope },
			)
			.await?
		{
			CommandResponse::SettingCleared { .. } => Ok(()),
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }) => Err(unexpected(&other)),
		}
	}

	/// Reports what the Plane can do: its platform, the external tools it
	/// found, whether credentials resolve, its Crafts and Harnesses, and
	/// what leaves it degraded (ADR-0086).
	///
	/// `jetd` observes the Plane at startup and whenever `observation` asks
	/// for a new look; it never polls in between.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn capabilities(
		&self,
		observation: CapabilityObservation,
	) -> Result<CapabilitySnapshot, ClientError> {
		self.require_minor(jet_protocol::SETTINGS_AND_CAPABILITIES_MINOR)?;
		match self
			.query(QueryRequest::Capabilities { observation })
			.await?
		{
			QueryResponse::Capabilities(snapshot) => Ok(snapshot),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)) => Err(unexpected(&other)),
		}
	}

	/// Reads every Account binding on the Plane with the journal cursor the
	/// snapshot was read at. A binding carries non-secret metadata and the
	/// opaque reference its Credential resolves through, never the
	/// Credential itself (ADR-0016, ADR-0076).
	///
	/// Beside each binding is whether its Credential resolves right now,
	/// taken from the last observation of the Plane or from a new one, as
	/// `observation` chooses. A client that has just unlocked the credential
	/// store asks for a new one.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn account_bindings(
		&self,
		observation: CapabilityObservation,
	) -> Result<AccountBindingList, ClientError> {
		self.require_minor(jet_protocol::ACCOUNT_BINDINGS_MINOR)?;
		match self
			.query(QueryRequest::AccountBindings { observation })
			.await?
		{
			QueryResponse::AccountBindings(list) => Ok(list),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)) => Err(unexpected(&other)),
		}
	}

	/// Binds a Provider account to the Plane under the Command identity
	/// `command_id`, which a retry must reuse (ADR-0093).
	///
	/// The request carries no secret: where the binding resolves through the
	/// platform credential store, the answer names the item the Plane will
	/// look in, and writing the secret there is the caller's to do
	/// (ADR-0076).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the metadata is not the metadata
	/// a binding carries or the Provider account is already bound, or the
	/// transport failure otherwise.
	pub async fn bind_account(
		&self,
		command_id: Uuid,
		provider: &str,
		label: &str,
		provider_account: Option<&str>,
		credential_source: CredentialSource,
	) -> Result<AccountBinding, ClientError> {
		self.require_minor(jet_protocol::ACCOUNT_BINDINGS_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::BindAccount {
					provider: provider.into(),
					label: label.into(),
					provider_account: provider_account.map(Into::into),
					credential_source,
				},
			)
			.await?
		{
			CommandResponse::AccountBound(binding) => Ok(binding),
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }) => Err(unexpected(&other)),
		}
	}

	/// Removes an Account binding under the Command identity `command_id`,
	/// which a retry must reuse (ADR-0093). The answer returns the reference
	/// the Plane forgot, so the caller can remove the secret it owns from
	/// the backend that holds it.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the Plane has no such binding,
	/// or the transport failure otherwise.
	pub async fn unbind_account(
		&self,
		command_id: Uuid,
		binding_id: Uuid,
	) -> Result<CredentialReference, ClientError> {
		self.require_minor(jet_protocol::ACCOUNT_BINDINGS_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::UnbindAccount { binding_id },
			)
			.await?
		{
			CommandResponse::AccountUnbound {
				credential_reference,
				..
			} => Ok(credential_reference),
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }) => Err(unexpected(&other)),
		}
	}

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
			| QueryResponse::SecurityAudit(_)) => Err(unexpected(&other)),
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
			| CommandResponse::PairingCompleted { .. }) => Err(unexpected(&other)),
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
			| CommandResponse::PairingCompleted { .. }) => Err(unexpected(&other)),
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
			| CommandResponse::PairingCompleted { .. }) => Err(unexpected(&other)),
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
			| CommandResponse::PairingCompleted { .. }) => Err(unexpected(&other)),
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
			| CommandResponse::PairingConfirmed { .. }) => Err(unexpected(&other)),
		}
	}
}

fn unexpected(reply: &impl std::fmt::Debug) -> ClientError {
	ClientError::Unexpected(format!("{reply:?}"))
}
