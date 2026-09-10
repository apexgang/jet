//! Client Commands and their strict wire input types.

use super::{RetentionPolicy, RunLifecycle};
use crate::{
	account::CredentialSource,
	conversation::{
		promotion::PromotionBinding, workspace::WorkingTreeRequest,
	},
	pairing::{
		ClientPublicKey, PairedClientAccess, PairingGate, PairingMethod,
	},
	setting::{SettingKey, SettingScope, SettingValue},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Commands a client may execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandRequest {
	/// Acknowledge a reviewed uncertain outcome without retrying its Git operation.
	AcknowledgeGitDelivery {
		/// Exact uncertain Effect the user reviewed.
		delivery_id: Uuid,
	},
	/// Queue one explicit non-destructive Git operation.
	DeliverGit {
		/// Owning Conversation.
		conversation_id: Uuid,
		/// Retained content for commits and generated PR text.
		checkpoint: Option<crate::GitCheckpoint>,
		/// Allowlisted operation.
		operation: crate::GitOperation,
	},
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
		#[serde(with = "crate::transport::decimal")]
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
		#[serde(with = "crate::transport::decimal")]
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
		#[serde(with = "crate::transport::hex")]
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
		#[serde(with = "crate::transport::decimal")]
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
