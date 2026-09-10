//! Typed semantic payloads retained in the Event journal.

use super::EventPayload;
use crate::{
	ClientId, ProjectId,
	account::{AccountBindingId, CredentialSource, ProviderId},
	audit::AuditEpoch,
	capability::HarnessId,
	conversation::{
		ConversationOrigin,
		import::{ImportId, NativeConversationId},
	},
	pairing::{PairingEnd, PairingOfferId},
	promotion::{PromotionBinding, PromotionId, PromotionState},
	setting::{SettingKey, SettingScope, SettingValue},
	workspace::{WorkingTree, WorkspaceBase, WorkspaceId, seed::WorkspaceSeed},
};
use jet_store::{
	PairedClientAccess, PairingGate, PairingMethod, RetentionPolicy,
	RunLifecycle,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// What an Event records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum EventKind {
	/// An Auto-continue retry decision changed durably.
	#[serde(rename = "auto_continue.changed")]
	AutoContinueChanged {
		/// Evidence, timing, policy, and bound.
		retry: Box<crate::AutoContinueRetry>,
	},
	/// A user configured a bounded Auto-continue policy.
	#[serde(rename = "auto_continue.configured")]
	AutoContinueConfigured {
		/// Scope of the policy.
		target: crate::AutoContinueTarget,
		/// Selected policy.
		policy: crate::AutoContinuePolicy,
	},
	/// A daily schedule was attached to this Conversation.
	#[serde(rename = "schedule.created")]
	ScheduleCreated {
		/// Enabled schedule and next firing.
		task: crate::ScheduledTask,
	},
	/// Future firings were canceled.
	#[serde(rename = "schedule.canceled")]
	ScheduleCanceled {
		/// Immutable schedule identity.
		schedule_id: Uuid,
	},
	/// Each elapsed occurrence retains its selection and admission outcome.
	#[serde(rename = "schedule.fired")]
	ScheduleFired {
		/// Responsible schedule.
		schedule_id: Uuid,
		/// Persisted intended instant and identity.
		firing: crate::ScheduleFiring,
		/// Admission outcome. Later execution outcomes use the same Turn identity.
		outcome: crate::ScheduleFiringOutcome,
	},
	/// An authenticated direct edit changed one file through a registered root.
	#[serde(rename = "user_edit.applied")]
	UserEditApplied {
		/// Root through which the edit was authorized.
		target: crate::FileTarget,
		/// Validated path relative to that root.
		path: crate::RelativePath,
		/// Exact state before the edit.
		before_revision: crate::FileRevision,
		/// Exact state after the edit.
		after_revision: crate::FileRevision,
	},
	/// One structured review batch entered the normal Turn queue.
	#[serde(rename = "review.submitted")]
	ReviewSubmitted {
		/// Queue admission that owns the review.
		turn_id: Uuid,
		/// Comments in the order the user submitted them.
		comments: Vec<crate::ReviewComment>,
	},
	/// An authoritative source changed a Conversation's resolved name.
	#[serde(rename = "conversation.name_changed")]
	ConversationNameChanged {
		/// New resolved name and the source that supplied it.
		name: crate::Name,
	},
	/// An authoritative source changed a Run's resolved name.
	#[serde(rename = "run.name_changed")]
	RunNameChanged {
		/// New resolved name and the source that supplied it.
		name: crate::Name,
	},
	/// A Workspace terminal changed lifecycle, without recording terminal bytes.
	#[serde(rename = "terminal.state_changed")]
	TerminalStateChanged {
		/// Terminal identity.
		terminal_id: crate::TerminalId,
		/// Owning Workspace.
		workspace_id: WorkspaceId,
		/// Observed lifecycle.
		state: crate::TerminalState,
	},
	/// Verified activity evidence was retained for the active turn.
	#[serde(rename = "change.evidence_recorded")]
	ChangeEvidenceRecorded {
		/// Active turn number.
		turn: u32,
		/// Content transition and its trusted origin.
		evidence: crate::ChangeEvidence,
	},
	/// A turn boundary and its Artifact committed with its source receipt.
	#[serde(rename = "change.checkpoint_recorded")]
	ChangeCheckpointRecorded {
		/// Turn number within the Run.
		turn: u32,
		/// Completion or interruption.
		outcome: crate::TurnOutcome,
		/// Immutable turn patch.
		artifact: crate::ChangeArtifact,
	},
	/// Verified immutable content became a durable Run reference.
	#[serde(rename = "artifact.published")]
	ArtifactPublished {
		/// Content address and exact byte count; payload bytes stay outside SQLite.
		artifact: crate::ArtifactDescriptor,
	},
	/// One bounded UTF-8 segment of admitted input; concatenate in Event order.
	#[serde(rename = "turn.input")]
	TurnInput {
		/// Admission whose original input this segment belongs to.
		turn_id: Uuid,
		/// At most 8192 UTF-8 bytes, keeping escaped JSON below the store limit.
		text: String,
	},
	/// An admission or subsequent observable queue outcome.
	#[serde(rename = "turn.changed")]
	TurnChanged {
		/// Stable input identity, authenticated client and authoritative order.
		turn: crate::Turn,
	},
	/// An interactive client asked to interrupt the current turn or stop
	/// the whole execution. Acceptance is not an outcome (ADR-0083).
	#[serde(rename = "run.control_requested")]
	RunControlRequested {
		/// What was asked of the execution.
		control: crate::RunControl,
	},
	/// How a control request actually ended the execution, including which
	/// escalation step it took.
	#[serde(rename = "run.terminated")]
	RunTerminated {
		/// The request and the step that answered it.
		termination: crate::RunTermination,
	},
	/// A Harness asked for something that needs a decision and is waiting
	/// for exactly one. Recording it grants nothing (ADR-0012).
	#[serde(rename = "approval.requested")]
	ApprovalRequested {
		/// The exact request, as the Craft is holding it.
		request: crate::ApprovalRequest,
	},
	/// How Automatic review answered one held request, including a review
	/// that could not happen and left the request for a person.
	#[serde(rename = "approval.reviewed")]
	ApprovalReviewed {
		/// The review, its routing, and the decision the core made.
		review: Box<crate::ApprovalReview>,
	},
	/// An interactive user authorized one retry of a stored denied action.
	#[serde(rename = "approval.retry_authorized")]
	ApprovalRetryAuthorized {
		/// The original denied review, retained in the journal.
		review_id: Uuid,
	},
	/// An active Run began working or waiting for a specific reason.
	#[serde(rename = "run.activity_changed")]
	RunActivityChanged {
		/// Absent outside the active lifecycle.
		activity: Option<crate::RunActivity>,
	},
	/// Native processes began or ended their participation in the Run.
	#[serde(rename = "run.processes_changed")]
	RunProcessesChanged {
		/// The current process projection.
		processes: Vec<crate::ManagedProcess>,
	},
	/// Lossless native JSON and portable views; strings preserve original bytes.
	#[serde(rename = "run.output")]
	RunOutput {
		/// Original JSON event, never reparsed through a lossy value tree.
		native_json: String,
		/// Original JSON Presentation blocks, including unknown kinds.
		presentation_json: Vec<String>,
	},
	/// The Craft reported the Harness-native Conversation identity.
	#[serde(rename = "run.native_conversation")]
	RunNativeConversation {
		/// Identity for a later explicit resume.
		native_conversation: String,
	},
	/// A new destination admitted its first Run from an explicit Handoff.
	#[serde(rename = "conversation.handoff_created")]
	HandoffCreated {
		/// Core-observed source and destination identities and captured objects.
		provenance: crate::HandoffProvenance,
	},
	/// A Conversation came into existence.
	#[serde(rename = "conversation.created")]
	ConversationCreated {
		/// Initial deterministic name. Absent in journals written before names.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		name: Option<crate::Name>,
		/// Its retention choice.
		retention: RetentionPolicy,
		/// Where it does its work. A journal written before Conversations
		/// had one reads as no Project.
		#[serde(default)]
		working_tree: WorkingTree,
		/// Where it came from. A journal written before Conversations could
		/// be imported reads as created in Jet.
		#[serde(default)]
		origin: ConversationOrigin,
	},
	/// A Harness-native Conversation discovered outside Jet was registered
	/// for managed continuation (ADR-0010).
	#[serde(rename = "conversation.imported")]
	ConversationImported {
		/// The import that was made.
		import_id: ImportId,
		/// The Harness whose identity it is.
		harness: HarnessId,
		/// The identity as the Harness spells it.
		native_conversation: NativeConversationId,
		/// The directory the Harness reported working in, if it reported
		/// one.
		working_directory: Option<PathBuf>,
	},
	/// A managed Workspace was created for a Conversation (ADR-0025).
	#[serde(rename = "workspace.created")]
	WorkspaceCreated {
		/// The Workspace that was created.
		workspace_id: WorkspaceId,
		/// The Project it was created from.
		project_id: ProjectId,
		/// The Jet-owned root of its worktree.
		root: PathBuf,
		/// What it started from.
		base: WorkspaceBase,
	},
	/// A Workspace was seeded with changes from its Project's Local
	/// checkout as it was created (ADR-0025).
	#[serde(rename = "workspace.seeded")]
	WorkspaceSeeded {
		/// The Workspace that was seeded.
		workspace_id: WorkspaceId,
		/// What it was seeded with.
		seed: WorkspaceSeed,
	},
	/// A Workspace promotion was recorded: applying, with the Effect that
	/// applies it committed beside it, or conflicted, with the paths that
	/// keep it from being applied (ADR-0025).
	#[serde(rename = "workspace.promotion_recorded")]
	WorkspacePromotionRecorded {
		/// The Workspace being promoted.
		workspace_id: WorkspaceId,
		/// The promotion that was recorded.
		promotion_id: PromotionId,
		/// What the user confirmed, the paths that could not be settled
		/// included.
		binding: PromotionBinding,
		/// Where the promotion starts: applying or conflicted.
		state: PromotionState,
	},
	/// A Workspace promotion's Effect settled: the destination was
	/// verified to hold the result, the Effect failed before changing
	/// anything, or its outcome could not be established (ADR-0025,
	/// ADR-0067).
	#[serde(rename = "workspace.promotion_settled")]
	WorkspacePromotionSettled {
		/// The Workspace that was promoted.
		workspace_id: WorkspaceId,
		/// The promotion that settled.
		promotion_id: PromotionId,
		/// Where it stands now.
		state: PromotionState,
	},
	/// A Run was recorded in the `created` state.
	#[serde(rename = "run.created")]
	RunCreated {
		/// Initial deterministic name. Absent in journals written before names.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		name: Option<crate::Name>,
	},
	/// One scope stored a Setting value.
	#[serde(rename = "setting.changed")]
	SettingChanged {
		/// The Setting that changed.
		key: SettingKey,
		/// The scope that stores the new value.
		scope: SettingScope,
		/// The value that scope now stores.
		value: SettingValue,
	},
	/// One scope stopped storing its own value for a Setting.
	#[serde(rename = "setting.cleared")]
	SettingCleared {
		/// The Setting that was cleared.
		key: SettingKey,
		/// The scope that no longer stores a value.
		scope: SettingScope,
	},
	/// A Provider account was bound to this Plane. The Event names the
	/// binding, its Provider, and the backend that resolves its Credential;
	/// no part of the Credential itself is recorded (ADR-0076).
	#[serde(rename = "account.bound")]
	AccountBound {
		/// The binding that was established.
		binding_id: AccountBindingId,
		/// The Provider it authenticates to.
		provider: ProviderId,
		/// The backend that resolves its Credential.
		credential_source: CredentialSource,
	},
	/// A normalized Usage record was stored for this Plane (ADR-0023).
	/// The Event names what was recorded so a client knows to read the
	/// Usage Query again; the numbers stay in that Query, which reports
	/// their freshness and estimation with them.
	#[serde(rename = "usage.recorded")]
	UsageRecorded {
		/// Where the measurement came from.
		source: crate::UsageSource,
		/// The Account binding it belongs to, when the Run named one.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		binding_id: Option<AccountBindingId>,
	},
	/// An Account binding was removed from this Plane.
	#[serde(rename = "account.unbound")]
	AccountUnbound {
		/// The binding that was removed.
		binding_id: AccountBindingId,
	},
	/// An owner began a new authority epoch of the Security audit after it
	/// failed to validate (ADR-0105). The gap it leaves behind is recorded
	/// in the audit itself; the journal only says that it happened.
	#[serde(rename = "audit.epoch_begun")]
	AuditEpochBegun {
		/// The epoch that now holds the chain the Plane vouches for.
		epoch: AuditEpoch,
	},
	/// The Plane's Pairing gate was opened or closed, deciding whether a
	/// new GUI client may begin Pairing (ADR-0017). It says nothing about
	/// the clients that are already Paired.
	#[serde(rename = "pairing.gate_changed")]
	PairingGateChanged {
		/// Where the owner left the gate.
		gate: PairingGate,
	},
	/// The Plane issued a Pairing offer, replacing whatever it had open
	/// (ADR-0017). The Event names the offer and how its secret was handed
	/// over; no part of the secret itself is recorded.
	#[serde(rename = "pairing.offered")]
	PairingOffered {
		/// The offer that was made.
		offer_id: PairingOfferId,
		/// How its secret reached the person pairing.
		method: PairingMethod,
	},
	/// A GUI client presented the offer's secret and its durable public
	/// key. The Pairing now waits for the people at both ends.
	#[serde(rename = "pairing.claimed")]
	PairingClaimed {
		/// The offer that was claimed.
		offer_id: PairingOfferId,
		/// The Client identity that claimed it.
		client_id: ClientId,
	},
	/// The person at the target confirmed that both screens showed the same
	/// authentication string.
	#[serde(rename = "pairing.confirmed")]
	PairingConfirmed {
		/// The offer that was confirmed.
		offer_id: PairingOfferId,
		/// The Client identity being Paired.
		client_id: ClientId,
	},
	/// A Pairing completed: the client proved it holds the identity it
	/// presented, and the Plane now holds its durable public key.
	#[serde(rename = "pairing.completed")]
	PairingCompleted {
		/// The offer that completed.
		offer_id: PairingOfferId,
		/// The Client identity that is now Paired.
		client_id: ClientId,
	},
	/// A Paired client was allowed to control this Plane again, or stopped
	/// from controlling it. The Plane keeps its key either way (ADR-0017).
	#[serde(rename = "pairing.client_access_changed")]
	PairedClientAccessChanged {
		/// The Paired client whose access changed.
		client_id: ClientId,
		/// What it may do now.
		access: PairedClientAccess,
	},
	/// A Paired client and the key it was Paired with were forgotten.
	#[serde(rename = "pairing.client_revoked")]
	PairedClientRevoked {
		/// The client that is no longer Paired.
		client_id: ClientId,
	},
	/// A Pairing offer stopped being usable before it completed.
	#[serde(rename = "pairing.offer_ended")]
	PairingOfferEnded {
		/// The offer that ended.
		offer_id: PairingOfferId,
		/// Why it ended.
		reason: PairingEnd,
	},
	/// A Run moved to a later lifecycle state.
	#[serde(rename = "run.lifecycle_changed")]
	RunLifecycleChanged {
		/// The state it left.
		from: RunLifecycle,
		/// The state it entered.
		to: RunLifecycle,
	},
	/// An interactive user's Path grant registered a Git working tree as a
	/// Project (ADR-0025, ADR-0101).
	#[serde(rename = "project.registered")]
	ProjectRegistered {
		/// The Project that was registered.
		project_id: ProjectId,
		/// The canonical root the grant resolved to.
		root: PathBuf,
	},
	/// An Event this core cannot interpret: a kind or payload version
	/// written by a newer core that shared the store (ADR-0073). It is
	/// retained and forwarded as recorded so a previous release still serves
	/// the whole journal and clients render it generically (ADR-0094).
	#[serde(skip)]
	Unrecognized(EventPayload),
}
