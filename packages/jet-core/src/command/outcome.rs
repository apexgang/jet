//! Durable results of admitted Commands.

use crate::{
	ClientId,
	account::{AccountBinding, AccountBindingId, CredentialReference},
	audit::AuditEpoch,
	conversation::{Conversation, Run, import::ImportedConversation},
	pairing::{
		PairedClient, PairingChallenge, PairingDisclosure, PendingPairing,
	},
	project::Project,
	promotion::WorkspacePromotion,
	setting::{SettingKey, SettingScope, SettingValue},
};
use jet_store::PairingGate;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The durable result of a [`Command`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandOutcome {
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
	/// The requested Auto-continue policy was stored.
	AutoContinueConfigured,
	/// One exact-action retry grant was durably recorded.
	ApprovalRetryAuthorized {
		/// The original denied review.
		review_id: Uuid,
	},
	/// A native mutation was durably staged.
	ExtensionChangeQueued {
		/// Identity for querying execution progress.
		change_id: Uuid,
	},
	/// The exact-action destination decision was recorded.
	RemoteToolReviewed {
		/// Reviewed operation identity.
		operation_id: Uuid,
	},
	/// Durable Utility identity, resolved by a subsequent Query.
	UtilityQueued {
		/// Plane-assigned identity.
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
	CraftInstallationQueued {
		/// Stable Craft identity.
		craft_id: String,
		/// Publisher-declared release version.
		version: String,
		/// SHA-256 of the exact Artifact that will be published.
		artifact_sha256: String,
	},
	/// Enabled immutable schedule and its first persisted firing.
	ScheduleCreated(crate::ScheduledTask),
	/// Removed schedule identity.
	ScheduleCanceled {
		/// Immutable schedule identity.
		schedule_id: Uuid,
	},
	/// A direct edit committed to its registered root.
	UserEditApplied(crate::UserEdit),
	/// The Conversation after its manual name committed.
	ConversationNamed(Conversation),
	/// The Run after its manual name committed.
	RunNamed(Run),
	/// Terminal request committed.
	Terminal(crate::WorkspaceTerminal),
	/// Durable withdrawal of queued user work.
	TurnWithdrawn(crate::Turn),
	/// Durable input identity and its original queue admission.
	TurnAdmitted(crate::Turn),
	/// The interactive resolution was durably queued.
	ExecutionResolutionRecorded(crate::ExecutionResolution),
	/// The control request was durably accepted; its outcome follows in
	/// the Run's Events.
	RunControlAccepted {
		/// The Run as it stands after accepting the request.
		run: Run,
		/// What was asked of it.
		control: crate::RunControl,
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
		/// The challenge to sign.
		challenge: PairingChallenge,
	},
	/// The Pairing offer after the person at the target confirmed it.
	PairingConfirmed {
		/// The offer, now waiting for the client to prove its key.
		pending: PendingPairing,
	},
	/// The client this Plane is now Paired with.
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
		client_id: ClientId,
	},
	/// The authority epoch the Security audit now records in.
	AuditEpochBegun {
		/// The epoch that holds the chain the Plane vouches for.
		epoch: AuditEpoch,
	},
	/// The Plane no longer has the binding, and the reference whose secret
	/// its owner may now remove from the backend.
	AccountUnbound {
		/// The binding that was removed.
		binding_id: AccountBindingId,
		/// The reference it resolved through.
		credential_reference: CredentialReference,
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
