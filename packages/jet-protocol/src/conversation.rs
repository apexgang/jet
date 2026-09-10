//! Wire form of Conversations and Runs, and the Commands clients execute.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::account::{AccountBinding, CredentialReference, CredentialSource};
use crate::import::{ConversationOrigin, ImportedConversation};
use crate::pairing::{
	ClientPublicKey, PairedClient, PairedClientAccess, PairingDisclosure,
	PairingGate, PairingMethod, PendingPairing,
};
use crate::project::Project;
use crate::promotion::{PromotionBinding, WorkspacePromotion};
use crate::setting::{SettingKey, SettingScope, SettingValue};
use crate::workspace::{WorkingTree, WorkingTreeRequest, Workspace};

/// Opaque token for continuing one fenced keyset snapshot page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct PageCursor(pub Uuid);

/// Whether Jet keeps a Conversation after its final Run.
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Conversation {
	/// Durable identity.
	pub conversation_id: Uuid,
	/// Current version for conflict-sensitive Commands. Absent before minor 20.
	#[serde(
		default,
		skip_serializing_if = "Option::is_none",
		with = "crate::decimal::optional"
	)]
	#[cfg_attr(feature = "schema", schemars(with = "crate::OptionalDecimal"))]
	pub revision: Option<u64>,
	/// Retention choice.
	pub retention: RetentionPolicy,
	/// Where it does its work. Absent before protocol minor 9.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub working_tree: Option<WorkingTree>,
	/// Where it came from. Absent before protocol minor 13.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub origin: Option<ConversationOrigin>,
	/// Resolved name and its authority. Absent before protocol minor 20.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub name: Option<crate::Name>,
	/// When it was created, in signed Unix milliseconds.
	pub created_at_unix_ms: i64,
}

/// One Run of a Conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Run {
	/// Durable identity.
	pub run_id: Uuid,
	/// The Conversation it executes.
	pub conversation_id: Uuid,
	/// Monotonic version used by conflict-sensitive Commands, carried as a
	/// decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub revision: u64,
	/// Current lifecycle state.
	pub lifecycle: RunLifecycle,
	/// Resolved name and its authority. Absent before protocol minor 20.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub name: Option<crate::Name>,
	/// When it was created, in signed Unix milliseconds.
	pub created_at_unix_ms: i64,
	/// When it reached a terminal state, if it has.
	pub ended_at_unix_ms: Option<i64>,
}

/// One bounded page of Conversations, fenced by a journal cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConversationList {
	/// Newest Event sequence visible when the list was read, carried as a
	/// decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub cursor: u64,
	/// Conversations in creation order.
	pub conversations: Vec<Conversation>,
	/// Opaque continuation token when another page belongs to this snapshot.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub next_page: Option<PageCursor>,
}

/// One Conversation with all of its Runs, fenced by a journal cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConversationSnapshot {
	/// Newest Event sequence visible when the snapshot was read, carried as
	/// a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub cursor: u64,
	/// The Conversation itself.
	pub conversation: Conversation,
	/// The Workspace it owns, when it works in one. Absent before protocol
	/// minor 9.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub workspace: Option<Workspace>,
	/// Its Runs in creation order, terminal ones included.
	pub runs: Vec<Run>,
}

/// Commands a client may execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandRequest {
	/// Configure a bounded Auto-continue policy (Jet 1.34).
	SetAutoContinue {
		/// Policy scope.
		target: crate::AutoContinueTarget,
		/// Explicit policy.
		policy: crate::AutoContinuePolicy,
	},
	/// Authorize one review retry of a stored denied action (Jet 1.32).
	AuthorizeApprovalRetry {
		/// Run that owns the current turn's denial.
		run_id: Uuid,
		/// Original denied review. The action cannot be replaced.
		review_id: Uuid,
	},
	/// Stage one native extension mutation for subsequent Runs (Jet 1.28).
	ChangeExtension {
		/// Complete native consent snapshot.
		confirmation: crate::ExtensionConfirmation,
	},
	/// Reviews one immutable No-Visa action captured by the destination.
	ReviewRemoteTool {
		/// Paired installation that submitted the action.
		client_id: Uuid,
		/// Captured remote operation identity.
		operation_id: Uuid,
		/// Applies only to this exact stored action.
		decision: crate::RemoteToolDecision,
	},
	/// Admit one bounded Utility request.
	RequestUtility {
		/// Purpose-specific input references.
		request: crate::UtilityRequest,
	},
	/// Install only the exact Craft proposal returned by discovery.
	DisableCraft {
		/// Installed Craft identity, never an executable path.
		craft_id: String,
		/// Wait for pinned Runs or stop their Craft immediately.
		mode: crate::CraftDisableMode,
	},
	/// Install a confirmed Artifact as the default for subsequent Runs.
	InstallCraft {
		/// Repository, provenance, Artifact, authority, and trust acceptance.
		confirmation: crate::CraftInstallationConfirmation,
	},
	/// Attach a daily Scheduled task to a retained Conversation.
	CreateSchedule {
		/// Owning Conversation.
		conversation_id: Uuid,
		/// Original IANA zone.
		time_zone: String,
		/// Daily local time in HH:MM:SS form.
		local_time: String,
		/// Scheduled input, 1 to 8192 UTF-8 bytes.
		prompt: String,
	},
	/// Cancel future firings and withdraw this schedule's pending input.
	CancelSchedule {
		/// Immutable schedule identity.
		schedule_id: Uuid,
	},

	/// Apply a bounded UTF-8 edit through a registered root.
	ApplyUserEdit {
		/// Registered Project or Workspace root.
		target: crate::FileTarget,
		/// Path relative to that root.
		path: String,
		/// Exact file state the editor read.
		expected_revision: crate::FileRevision,
		/// Complete replacement content, at most 128 KiB.
		content: String,
	},
	/// Submit a batch of inline comments as one user Turn.
	SubmitReview {
		/// Conversation whose normal queue receives the Turn.
		conversation_id: Uuid,
		/// Structured comments, preserved together in submission order.
		comments: Vec<crate::ReviewComment>,
	},
	/// Set the authoritative manual name of a Conversation.
	SetConversationName {
		/// Conversation to name.
		conversation_id: Uuid,
		/// Revision observed when the Command was prepared.
		#[serde(with = "crate::decimal")]
		#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
		expected_revision: u64,
		/// Validated as 1 to 256 UTF-8 bytes by the trusted core.
		name: String,
	},
	/// Set the authoritative manual name of a Run.
	SetRunName {
		/// Run to name.
		run_id: Uuid,
		/// Revision observed when the Command was prepared.
		#[serde(with = "crate::decimal")]
		#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
		expected_revision: u64,
		/// Validated as 1 to 256 UTF-8 bytes by the trusted core.
		name: String,
	},
	/// Open a PTY under a Workspace-owned helper.
	OpenTerminal {
		/// Registered Workspace identity.
		workspace_id: Uuid,
		/// Height in cells (1–1000).
		rows: u16,
		/// Width in cells (1–1000).
		columns: u16,
	},
	/// Explicitly end a Workspace terminal.
	CloseTerminal {
		/// Terminal identity.
		terminal_id: Uuid,
	},
	/// Withdraw only the caller's own queued user input.
	WithdrawTurn {
		/// Conversation owning the queue.
		conversation_id: Uuid,
		/// Admitted input identity.
		turn_id: Uuid,
	},
	/// Admit input to one Conversation. Source selects a slot, not an Actor.
	SubmitTurn {
		/// Conversation that owns the input.
		conversation_id: Uuid,
		/// Input class; user work is never replaced.
		#[serde(default)]
		source: crate::TurnSource,
		/// Input of 1 to 65536 UTF-8 bytes.
		prompt: String,
	},
	/// Resolve only the helper instance inspected by an interactive user.
	ResolveExecution {
		/// Selected execution.
		execution_id: Uuid,
		/// Expected instance; unavailable metadata permits only Leave.
		instance: Option<Uuid>,
		/// Explicit decision.
		action: crate::ExecutionAction,
	},
	/// Ask the Harness to end the turn it is working on, leaving the Run
	/// able to accept the next one. It is not a transport cancellation and
	/// never withdraws queued input (ADR-0083, ADR-0095).
	InterruptTurn {
		/// The managed Run whose current turn ends.
		run_id: Uuid,
	},
	/// End one managed Run, including the native processes it owns.
	StopRun {
		/// The managed Run to end.
		run_id: Uuid,
	},
	/// Start native execution on the selected Conversation Home Plane (1.25).
	StartVisaRun(crate::VisaRunRequest),
	/// Desktop-only origin execution through paired remote tools (1.26).
	StartNoVisaRun(crate::NoVisaRunRequest),
	/// Start a managed Run with an installed Craft and initial input.
	StartRun {
		/// Conversation whose working tree is used.
		conversation_id: Uuid,
		/// Installed Craft identity.
		craft: String,
		/// Initial Harness input.
		prompt: String,
	},
	/// Create a Conversation with no Runs. Retained unless told otherwise,
	/// and in no Project unless a working tree is asked for.
	CreateConversation {
		/// Retention choice.
		#[serde(default)]
		retention: RetentionPolicy,
		/// Where the Conversation does its work. A Workspace request
		/// creates the Workspace with the Conversation.
		#[serde(
			default,
			skip_serializing_if = "WorkingTreeRequest::is_no_project"
		)]
		working_tree: WorkingTreeRequest,
	},
	/// Continue through another Harness with a new Conversation and Workspace.
	HandoffConversation(crate::HandoffRequest),
	/// Fork from an immutable checkpoint rather than a current Handoff package.
	ForkConversation {
		/// Run that owns the selected checkpoint.
		source_run_id: Uuid,
		/// One-based turn boundary to fork from.
		checkpoint_turn: u32,
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
		#[cfg_attr(feature = "schema", schemars(with = "crate::Hex<64>"))]
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
		#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
		expected_revision: u64,
		/// The state to enter.
		lifecycle: RunLifecycle,
	},
	/// Register the Git working tree at an absolute path as a Project. This
	/// is a Path grant: the interactive user's explicit authorization for
	/// the Plane to work inside that directory. The Plane resolves the path
	/// and refuses anything that is not an ordinary working tree.
	RegisterProject {
		/// The absolute path the user granted.
		path: String,
	},
	/// Promote a Workspace exactly as a preview showed. The Plane computes
	/// the preview again and refuses a binding the Workspace or the
	/// destination has moved past.
	PromoteWorkspace {
		/// What the preview bound and the user confirmed.
		binding: PromotionBinding,
	},
	/// Register a Harness-native Conversation the Plane can see outside its
	/// management, so a managed Run may later continue it. The Plane looks
	/// for the identity again and refuses one no supported Harness reports.
	ImportConversation {
		/// The Harness whose identity it is, such as `codex`.
		harness: String,
		/// The identity as the Harness spells it.
		native_conversation: String,
	},
	/// Continue an Imported conversation as a new Conversation in a
	/// Workspace or the Local checkout of a registered Project. A request
	/// with no Project is refused: the user registers or maps one first.
	ResumeImportedConversation {
		/// The import to continue.
		import_id: Uuid,
		/// Retention choice.
		#[serde(default)]
		retention: RetentionPolicy,
		/// Where the Conversation does its work.
		working_tree: WorkingTreeRequest,
	},
}

/// Durable Command outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandResponse {
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
		#[serde(with = "crate::hex")]
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
	/// The authority epoch the Security audit now records in.
	AuditEpochBegun {
		/// The epoch that holds the chain the Plane vouches for, carried as
		/// a decimal string (ADR-0089).
		#[serde(with = "crate::decimal")]
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

/// Structured state returned when a Revision precondition is stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RevisionConflict {
	/// Revision that is authoritative now, carried as a decimal string
	/// (ADR-0089).
	#[serde(with = "crate::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub current_revision: u64,
	/// Safe current state with which the caller can refresh.
	pub safe_state: ConflictState,
}

/// Safe resource state attached to a Revision conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConflictState {
	/// The current Conversation.
	Conversation {
		/// Complete safe Conversation state.
		conversation: Conversation,
	},
	/// The current Run.
	Run {
		/// Complete safe Run state.
		run: Run,
	},
}
