//! Wire form of Conversations and Runs, and the Commands clients execute.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::account::{AccountBinding, CredentialReference, CredentialSource};
use crate::pairing::{
	ClientPublicKey, PairedClient, PairedClientAccess, PairingDisclosure,
	PairingGate, PairingMethod, PendingPairing,
};
use crate::setting::{SettingKey, SettingScope, SettingValue};

/// Opaque token for continuing one fenced keyset snapshot page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PageCursor(pub Uuid);

/// Whether Jet keeps a Conversation after its final Run.
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RetentionPolicy {
	/// Keep the Conversation and its history. The default.
	#[default]
	Retain,
	/// Forget the Conversation once it has no live Run and no other
	/// protected state.
	ForgetAfterFinalRun,
}

/// Mutually exclusive lifecycle state of one Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunLifecycle {
	/// Recorded but not yet launching.
	Created,
	/// Launching its Harness.
	Starting,
	/// Executing.
	Active,
	/// Ending gracefully.
	Stopping,
	/// Terminal: finished its work.
	Completed,
	/// Terminal: ended with an error.
	Failed,
	/// Terminal: ended on request.
	Canceled,
	/// Terminal: its execution can no longer be observed.
	Lost,
}

/// One Conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
	/// Durable identity.
	pub conversation_id: Uuid,
	/// Retention choice.
	pub retention: RetentionPolicy,
	/// When it was created, in signed Unix milliseconds.
	pub created_at_unix_ms: i64,
}

/// One Run of a Conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
	/// Durable identity.
	pub run_id: Uuid,
	/// The Conversation it executes.
	pub conversation_id: Uuid,
	/// Monotonic version used by conflict-sensitive Commands, carried as a
	/// decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub revision: u64,
	/// Current lifecycle state.
	pub lifecycle: RunLifecycle,
	/// When it was created, in signed Unix milliseconds.
	pub created_at_unix_ms: i64,
	/// When it reached a terminal state, if it has.
	pub ended_at_unix_ms: Option<i64>,
}

/// One bounded page of Conversations, fenced by a journal cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationList {
	/// Newest Event sequence visible when the list was read, carried as a
	/// decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// Conversations in creation order.
	pub conversations: Vec<Conversation>,
	/// Opaque continuation token when another page belongs to this snapshot.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub next_page: Option<PageCursor>,
}

/// One Conversation with all of its Runs, fenced by a journal cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSnapshot {
	/// Newest Event sequence visible when the snapshot was read, carried as
	/// a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// The Conversation itself.
	pub conversation: Conversation,
	/// Its Runs in creation order, terminal ones included.
	pub runs: Vec<Run>,
}

/// Commands a client may execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandRequest {
	/// Create a Conversation with no Runs. Retained unless told otherwise.
	CreateConversation {
		/// Retention choice.
		#[serde(default)]
		retention: RetentionPolicy,
	},
	/// Record a new Run of a Conversation that has no live Run.
	CreateRun {
		/// The Conversation to execute.
		conversation_id: Uuid,
	},
	/// Store a Setting value at one scope.
	SetSetting {
		/// The Setting to store.
		key: SettingKey,
		/// The scope that stores the value.
		scope: SettingScope,
		/// The value to store.
		value: SettingValue,
	},
	/// Remove whatever value one scope stores for a Setting, leaving the
	/// scopes above it untouched.
	ClearSetting {
		/// The Setting to clear.
		key: SettingKey,
		/// The scope that stops storing a value.
		scope: SettingScope,
	},
	/// Bind a Provider account to this Plane. The request carries non-secret
	/// metadata only; the Credential itself never crosses this protocol.
	BindAccount {
		/// The Provider the binding authenticates to, such as `anthropic`.
		provider: String,
		/// The user-facing name of the binding.
		label: String,
		/// The Provider's own account identity, when it supplies one.
		#[serde(default)]
		provider_account: Option<String>,
		/// The backend that resolves the binding's Credential.
		credential_source: CredentialSource,
	},
	/// Remove an Account binding from this Plane.
	UnbindAccount {
		/// The binding to remove.
		binding_id: Uuid,
	},
	/// Begin a new authority epoch of the Security audit, carrying on past
	/// an integrity failure and recording the gap it leaves behind.
	BeginAuditEpoch,
	/// Open or close the Plane's Pairing gate, which decides whether a new
	/// GUI client may begin Pairing at all. It does not alter the clients
	/// that are already Paired.
	SetPairingGate {
		/// Where to leave the gate.
		gate: PairingGate,
	},
	/// Issue the Plane's one Pairing offer, replacing whatever it had open,
	/// and disclose its one-time secret to the owner who asked for it.
	OpenPairing {
		/// How the secret reaches the person pairing.
		method: PairingMethod,
	},
	/// Claim the open Pairing offer with the secret a person presented and
	/// the public key of the Client identity presenting it.
	ClaimPairing {
		/// The secret as it was presented.
		secret: String,
		/// The durable public key that becomes the credential once Pairing
		/// completes.
		key: ClientPublicKey,
	},
	/// Confirm, on the Plane being Paired with, that both screens show the
	/// same authentication string. The client being Paired cannot confirm
	/// its own Pairing.
	ConfirmPairing {
		/// The offer being confirmed, which must be the one the Plane has
		/// open.
		offer_id: Uuid,
		/// The string as the person confirming reads it.
		authentication_string: String,
	},
	/// Complete the Pairing by signing the transcript of the claim with the
	/// Client identity that made it.
	CompletePairing {
		/// The offer being completed, which must be the one the Plane has
		/// open.
		offer_id: Uuid,
		/// The signature over the claim's transcript, as 128 lowercase
		/// hexadecimal characters.
		#[serde(with = "crate::hex")]
		signature: [u8; 64],
	},
	/// Stop a Paired client controlling the Plane, or let it control the
	/// Plane again. The Plane keeps its key either way.
	SetPairedClientAccess {
		/// The Paired client to decide about.
		client_id: Uuid,
		/// What it may do from now on.
		access: PairedClientAccess,
	},
	/// Forget a Paired client and the key it was Paired with. The
	/// installation has to be Paired again to control the Plane.
	RevokePairedClient {
		/// The client to forget.
		client_id: Uuid,
	},
	/// Move a Run forward through its lifecycle.
	TransitionRun {
		/// The Run to move.
		run_id: Uuid,
		/// Revision observed when the Command was prepared, carried as a
		/// decimal string (ADR-0089).
		#[serde(with = "crate::decimal")]
		expected_revision: u64,
		/// The state to enter.
		lifecycle: RunLifecycle,
	},
}

/// Durable Command outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandResponse {
	/// The Conversation as created.
	ConversationCreated(Conversation),
	/// The Run as created.
	RunCreated(Run),
	/// The Run after its transition.
	RunTransitioned(Run),
	/// The Setting value the named scope now stores.
	SettingSet {
		/// The Setting that was stored.
		key: SettingKey,
		/// The scope that stores it.
		scope: SettingScope,
		/// The stored value.
		value: SettingValue,
	},
	/// The named scope no longer stores its own value for the Setting.
	SettingCleared {
		/// The Setting that was cleared.
		key: SettingKey,
		/// The scope that no longer stores a value.
		scope: SettingScope,
	},
	/// The Account binding as established.
	AccountBound(AccountBinding),
	/// The Plane no longer has the binding, and the reference whose secret
	/// its owner may now remove from the backend.
	AccountUnbound {
		/// The binding that was removed.
		binding_id: Uuid,
		/// The reference it resolved through.
		credential_reference: CredentialReference,
	},
	/// Where the Plane's Pairing gate now stands.
	PairingGateSet {
		/// The gate as the Plane now records it.
		gate: PairingGate,
	},
	/// The Pairing offer the Plane now has open, and its one-time secret as
	/// it is disclosed once.
	PairingOpened {
		/// The offer, without the secret it was issued with.
		pending: PendingPairing,
		/// The secret, in the form the owner hands it over in.
		disclosure: PairingDisclosure,
	},
	/// The Pairing offer after a client claimed it, and the fresh challenge
	/// that client's key signs to complete the Pairing.
	PairingClaimed {
		/// The offer, now waiting for the people at both ends.
		pending: PendingPairing,
		/// The challenge to sign, as 64 lowercase hexadecimal characters.
		#[serde(with = "crate::hex")]
		challenge: [u8; 32],
	},
	/// The Pairing offer after the person at the target confirmed it.
	PairingConfirmed {
		/// The offer, now waiting for the client to prove its key.
		pending: PendingPairing,
	},
	/// The client the Plane is now Paired with.
	PairingCompleted {
		/// The Paired client the Pairing left behind.
		client: PairedClient,
	},
	/// The Paired client as the Plane now records it.
	PairedClientAccessSet {
		/// The client, with the access it now has.
		client: PairedClient,
	},
	/// The Plane no longer holds that client or its key.
	PairedClientRevoked {
		/// The client that is no longer Paired.
		client_id: Uuid,
	},
	/// The authority epoch the Security audit now records in.
	AuditEpochBegun {
		/// The epoch that holds the chain the Plane vouches for, carried as
		/// a decimal string (ADR-0089).
		#[serde(with = "crate::decimal")]
		epoch: u64,
	},
}

/// Structured state returned when a Revision precondition is stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionConflict {
	/// Revision that is authoritative now, carried as a decimal string
	/// (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub current_revision: u64,
	/// Safe current state with which the caller can refresh.
	pub safe_state: ConflictState,
}

/// Safe resource state attached to a Revision conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConflictState {
	/// The current Run.
	Run {
		/// Complete safe Run state.
		run: Run,
	},
}
