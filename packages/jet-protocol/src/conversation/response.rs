//! Wire replies to committed client Commands.

use super::{Conversation, Run};
use crate::{
	account::{AccountBinding, CredentialReference},
	conversation::{
		import::ImportedConversation, promotion::WorkspacePromotion,
	},
	pairing::{PairedClient, PairingDisclosure, PairingGate, PendingPairing},
	project::Project,
	setting::{SettingKey, SettingScope, SettingValue},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Durable Command outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandResponse {
	/// A user released the uncertainty barrier; Git was not changed.
	GitDeliveryAcknowledged {
		/// Exact Effect identity.
		delivery_id: Uuid,
	},
	/// Durable Git operation identity.
	GitDeliveryQueued {
		/// Stable Effect identity.
		delivery_id: Uuid,
	},
	/// The Auto-continue policy was stored.
	AutoContinueConfigured,
	/// A single exact-action review retry was authorized.
	ApprovalRetryAuthorized {
		/// Original denied review.
		review_id: Uuid,
	},
	/// Native lifecycle request was durably staged.
	ExtensionChangeQueued {
		/// Identity for reading progress.
		change_id: Uuid,
	},
	/// The exact-action destination decision was recorded.
	RemoteToolReviewed {
		/// Reviewed operation identity.
		operation_id: Uuid,
	},
	/// Durable Utility admission; query for its result.
	UtilityQueued {
		/// Plane-assigned job identity.
		job_id: Uuid,
	},
	/// Durable publication work accepted for one verified Craft Artifact.
	CraftDisabled {
		/// Disabled Craft identity.
		craft_id: String,
		/// Applied disable behavior.
		mode: crate::CraftDisableMode,
	},
	/// A verified Artifact was accepted for publication.
	CraftInstallationQueued(crate::CraftInstallationQueued),
	/// Enabled daily schedule.
	ScheduleCreated {
		/// Enabled schedule and next firing.
		task: crate::ScheduledTask,
	},
	/// Canceled immutable schedule identity.
	ScheduleCanceled {
		/// Immutable schedule identity.
		schedule_id: Uuid,
	},
	/// A direct edit committed to the registered root.
	UserEditApplied {
		/// Registered root that was edited.
		target: crate::FileTarget,
		/// Validated path relative to the root.
		path: String,
		/// Exact resulting Git content and mode.
		revision: crate::FileRevision,
	},
	/// A Conversation's manual name committed.
	ConversationNamed(Conversation),
	/// A Run's manual name committed.
	RunNamed(Run),
	/// Terminal admission or close was committed.
	Terminal {
		/// Current terminal state.
		terminal: crate::WorkspaceTerminal,
	},
	/// The original durable withdrawal result.
	TurnWithdrawn {
		/// Input with its final outcome.
		turn: crate::Turn,
	},
	/// The original durable admission, unchanged by later queue progress.
	TurnAdmitted {
		/// Plane-assigned input identity and sequence.
		turn: crate::Turn,
	},
	/// The control request was durably accepted. Its outcome follows in
	/// the Run's Events; acceptance alone never claims the work stopped.
	RunControlAccepted {
		/// The Run as it stands after accepting the request.
		run: Run,
		/// What was asked of it.
		control: crate::RunControl,
	},
	/// The interactive decision was durably queued.
	ExecutionResolutionRecorded {
		/// Selected execution.
		execution_id: Uuid,
		/// Explicit decision.
		action: crate::ExecutionAction,
	},
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
		#[serde(with = "crate::transport::hex")]
		#[cfg_attr(feature = "schema", schemars(with = "crate::Hex<32>"))]
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
	/// The Plane serves again from the restored snapshot.
	RecoverySnapshotRestored {
		/// The snapshot that is now the store.
		snapshot: String,
		/// The file name, beside the store, the damaged database was
		/// moved to.
		damaged: String,
	},
	/// The authority epoch the Security audit now records in.
	AuditEpochBegun {
		/// The epoch that holds the chain the Plane vouches for, carried as
		/// a decimal string (ADR-0089).
		#[serde(with = "crate::transport::decimal")]
		#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
		epoch: u64,
	},
	/// The Project as registered.
	ProjectRegistered(Project),
	/// The promotion as recorded: applying, with its Effect committed, or
	/// conflicted, with the paths that keep it from being applied.
	WorkspacePromotionRecorded(WorkspacePromotion),
	/// The Imported conversation as registered, with no Conversation
	/// continuing it yet.
	ConversationImported(ImportedConversation),
}
