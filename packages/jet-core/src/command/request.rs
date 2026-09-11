//! Command inputs and admission requirements.

use crate::{
	ClientId,
	account::{
		AccountBindingId, CredentialSource, ProviderAccount, ProviderId,
	},
	audit,
	capability::{Capability, ExternalTool, HarnessId},
	conversation::{
		ConversationId, Revision, RunId,
		import::{ImportId, NativeConversationId},
	},
	pairing::{
		AuthenticationString, ClientPublicKey, PairingOfferId, PairingSecret,
		PairingSignature,
	},
	project::PathGrant,
	promotion::PromotionBinding,
	security::SecurityClass,
	setting::{SettingKey, SettingScope, SettingValue},
	workspace::WorkingTreeRequest,
};
use jet_store::{
	PairedClientAccess, PairingGate, PairingMethod, RetentionPolicy,
	RunLifecycle,
};
use serde::Serialize;
use uuid::Uuid;

/// Automatic Git delivery is carried out with the Git the core invokes, so
/// turning it on depends on that tool being installed, and a Project is
/// registered only after that Git has looked at it (ADR-0029, ADR-0056,
/// ADR-0103).
pub(super) const GIT: &[Capability] =
	&[Capability::ExternalTool(ExternalTool::Git)];

/// A binding that resolves through the platform credential store depends on
/// there being one. Jet keeps no secret of its own instead, so a Plane
/// without a store refuses the binding rather than falling back to
/// plaintext (ADR-0076).
pub(super) const CREDENTIAL_STORE: &[Capability] =
	&[Capability::CredentialStore];

/// A state-changing request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Command {
	/// Acknowledge a reviewed uncertain outcome without retrying its Git operation.
	AcknowledgeGitDelivery {
		/// Exact uncertain Effect the user reviewed.
		delivery_id: Uuid,
	},
	/// Queue one explicit non-destructive Git operation.
	DeliverGit {
		/// Owning Conversation.
		conversation_id: ConversationId,
		/// Retained content for commits and generated PR text.
		checkpoint: Option<crate::GitCheckpoint>,
		/// Allowlisted operation.
		operation: crate::GitOperation,
	},
	/// Configure an Account-binding default or a one-shot Conversation override.
	SetAutoContinue {
		/// Policy scope.
		target: crate::AutoContinueTarget,
		/// Explicit bounded retry policy.
		policy: crate::AutoContinuePolicy,
	},
	/// Authorize one review retry of the exact stored denied action.
	AuthorizeApprovalRetry {
		/// Live Run that owns the denied review.
		run_id: RunId,
		/// Denial from the current turn; no replacement action is accepted.
		review_id: Uuid,
	},
	/// Stage a confirmed native extension change for subsequent Runs.
	ChangeExtension {
		/// Exact native metadata and accepted same-user authority.
		confirmation: crate::ExtensionConfirmation,
	},
	/// Reviews one immutable No-Visa action captured by the destination.
	ReviewRemoteTool {
		/// Paired installation that submitted the action.
		client_id: ClientId,
		/// Captured remote operation identity.
		operation_id: Uuid,
		/// Applies only to this exact stored action.
		decision: crate::RemoteToolDecision,
	},
	/// Admit one bounded Utility request.
	RequestUtility {
		/// Purpose-specific input.
		request: crate::UtilityRequest,
	},
	/// Install only the exact Craft release whose complete consent surface
	/// was returned by a preceding discovery Query.
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
		conversation_id: ConversationId,
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
		/// Validated relative path.
		path: crate::RelativePath,
		/// Exact file state the editor read.
		expected_revision: crate::FileRevision,
		/// Complete replacement content.
		content: String,
	},
	/// Submit structured inline comments as one queued user Turn.
	SubmitReview {
		/// Conversation whose queue receives the Turn.
		conversation_id: ConversationId,
		/// Validated comments in submission order.
		comments: Vec<crate::ReviewComment>,
	},
	/// Set the authoritative manual name of a Conversation (ADR-0044).
	SetConversationName {
		/// Conversation to name.
		conversation_id: ConversationId,
		/// Revision observed when the Command was prepared.
		expected_revision: Revision,
		/// Validated manual name.
		name: crate::Name,
	},
	/// Set the authoritative manual name of a Run (ADR-0044).
	SetRunName {
		/// Run to name.
		run_id: RunId,
		/// Revision observed when the Command was prepared.
		expected_revision: Revision,
		/// Validated manual name.
		name: crate::Name,
	},
	/// Open a Workspace-owned terminal.
	OpenTerminal {
		/// Registered Workspace.
		workspace_id: crate::WorkspaceId,
		/// Height.
		rows: u16,
		/// Width.
		columns: u16,
	},
	/// Close a Workspace terminal explicitly.
	CloseTerminal {
		/// Terminal to close.
		terminal_id: crate::TerminalId,
	},
	/// Withdraw only the caller's own queued user input.
	WithdrawTurn {
		/// Conversation owning the queue.
		conversation_id: ConversationId,
		/// Admitted input identity.
		turn_id: Uuid,
	},
	/// Admit input to the authoritative Conversation queue.
	SubmitTurn {
		/// Conversation that owns the input.
		conversation_id: ConversationId,
		/// Independently coalesced input class; not Actor authority.
		source: crate::TurnSource,
		/// Bounded Harness input.
		prompt: String,
	},
	/// Resolve only the Orphaned execution instance inspected by the user.
	ResolveExecution(crate::ExecutionResolution),
	/// Ask one managed Run to end its current turn or its whole execution.
	/// Withdrawing queued input is a different Command with a different
	/// effect, and neither is a transport cancellation (ADR-0083,
	/// ADR-0095).
	ControlRun {
		/// The managed Run being controlled.
		run_id: RunId,
		/// What is asked of it.
		control: crate::RunControl,
	},
	/// Start native execution using explicit destination-local selections.
	StartVisaRun(crate::VisaRunRequest),
	/// Starts a desktop origin Run with paired destination tools.
	StartNoVisaRun(crate::NoVisaRunRequest),
	/// Start a managed Run with one installed Craft and its initial input.
	StartRun {
		/// The Conversation whose registered working tree is used.
		conversation_id: ConversationId,
		/// Installed Craft identity, never an executable path.
		craft: String,
		/// Initial Harness input.
		prompt: String,
	},
	/// Create a Conversation with no Runs, and the Workspace it works in
	/// when it asks for one (ADR-0025).
	CreateConversation {
		/// Whether Jet keeps the Conversation after its final Run.
		retention: RetentionPolicy,
		/// Where it does its work.
		working_tree: WorkingTreeRequest,
	},
	/// Continue through another Harness with a new Conversation and Workspace.
	HandoffConversation(crate::HandoffRequest),
	/// Fork from an immutable checkpoint rather than a current Handoff package.
	ForkConversation {
		/// Run that owns the selected checkpoint.
		source_run_id: RunId,
		/// One-based turn boundary to fork from.
		checkpoint_turn: u32,
	},
	/// Record a new Run of a Conversation that has no live Run.
	CreateRun {
		/// The Conversation to execute.
		conversation_id: ConversationId,
	},
	/// Store a Setting value at one scope (ADR-0085).
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
	/// Bind a Provider account to this Plane, storing only non-secret
	/// metadata and an opaque Credential reference (ADR-0016, ADR-0076).
	BindAccount {
		/// The Provider the binding authenticates to.
		provider: ProviderId,
		/// The user-facing name of the binding.
		label: String,
		/// The Provider's own account identity, when it supplies one.
		provider_account: Option<ProviderAccount>,
		/// The backend that resolves the binding's Credential.
		credential_source: CredentialSource,
	},
	/// Remove an Account binding from this Plane. The secret it referred to
	/// belongs to its backend, so Jet forgets the reference and leaves the
	/// backend to the client that wrote it.
	UnbindAccount {
		/// The binding to remove.
		binding_id: AccountBindingId,
	},
	/// Begin a new authority epoch of the Security audit, carrying on past
	/// an integrity failure and recording the gap it leaves (ADR-0105).
	BeginAuditEpoch,
	/// Restore a verified Recovery snapshot over the damaged store of a
	/// Plane in read-only Recovery mode (ADR-0077).
	RestoreRecoverySnapshot {
		/// The snapshot, by name.
		snapshot: String,
	},
	/// Open or close this Plane's Pairing gate, which decides whether a new
	/// GUI client may begin Pairing at all (ADR-0017). It does not alter the
	/// clients that are already Paired.
	SetPairingGate {
		/// Where to leave the gate.
		gate: PairingGate,
	},
	/// Issue this Plane's one Pairing offer, replacing whatever it had
	/// open, and disclose its one-time secret to the owner who asked for it
	/// (ADR-0017).
	OpenPairing {
		/// How the secret reaches the person pairing.
		method: PairingMethod,
	},
	/// Claim the open Pairing offer with the secret a person presented and
	/// the public key of the Client identity presenting it.
	ClaimPairing {
		/// The secret as it was presented.
		secret: PairingSecret,
		/// The durable public key that becomes the credential once Pairing
		/// completes.
		key: ClientPublicKey,
	},
	/// Confirm, on the Plane being Paired with, that both screens show the
	/// same authentication string. The client being Paired cannot confirm
	/// its own Pairing (ADR-0017).
	ConfirmPairing {
		/// The offer being confirmed, which must be the one the Plane has
		/// open.
		offer_id: PairingOfferId,
		/// The string as the person confirming reads it.
		authentication_string: AuthenticationString,
	},
	/// Complete the Pairing by signing the transcript of the claim with the
	/// Client identity that made it (ADR-0090).
	CompletePairing {
		/// The offer being completed, which must be the one the Plane has
		/// open.
		offer_id: PairingOfferId,
		/// The signature over the claim's transcript.
		signature: PairingSignature,
	},
	/// Stop a Paired client controlling this Plane, or let it control the
	/// Plane again. The Plane keeps its key either way (ADR-0017).
	SetPairedClientAccess {
		/// The Paired client to decide about.
		client_id: ClientId,
		/// What it may do from now on.
		access: PairedClientAccess,
	},
	/// Forget a Paired client and the key it was Paired with. The
	/// installation has to be Paired again to control this Plane.
	RevokePairedClient {
		/// The client to forget.
		client_id: ClientId,
	},
	/// Move a Run forward through its lifecycle.
	TransitionRun {
		/// The Run to move.
		run_id: RunId,
		/// Revision observed when the Command was prepared.
		expected_revision: Revision,
		/// The state to enter.
		lifecycle: RunLifecycle,
	},
	/// Register the Git working tree an interactive user granted as a
	/// Project (ADR-0025, ADR-0101). The grant is resolved and inspected
	/// before the transaction opens; a directory that is not an ordinary
	/// working tree is refused (ADR-0103).
	RegisterProject {
		/// The user's explicit authorization for one absolute path.
		grant: PathGrant,
	},
	/// Promote a Workspace to the permanent checkout or branch its preview
	/// was made for, exactly as previewed (ADR-0025). The preview is
	/// computed again before the transaction opens, and a Workspace or
	/// destination that moved on makes it stale and refused.
	PromoteWorkspace {
		/// What the preview bound and the user confirmed.
		binding: PromotionBinding,
	},
	/// Register a Harness-native Conversation the Plane can see outside
	/// its management, so a managed Run may later continue it (ADR-0010).
	/// The identity is looked for again before the transaction opens; one
	/// no supported Harness reports is refused.
	ImportConversation {
		/// The Harness whose identity it is.
		harness: HarnessId,
		/// The identity as the Harness spells it.
		native_conversation: NativeConversationId,
	},
	/// Continue an Imported conversation as a new Conversation in a
	/// Workspace or the Local checkout of a registered Project, chosen by
	/// the user (ADR-0010, ADR-0025). Nowhere to work is refused.
	ResumeImportedConversation {
		/// The import to continue.
		import_id: ImportId,
		/// Whether Jet keeps the Conversation after its final Run.
		retention: RetentionPolicy,
		/// Where it does its work.
		working_tree: WorkingTreeRequest,
	},
}

impl Command {
	/// What the Plane must still be able to do when this Command runs. Each
	/// one is checked against a new observation before anything commits
	/// (ADR-0086).
	pub(crate) fn required_capabilities(&self) -> &'static [Capability] {
		match self {
			Self::AcknowledgeGitDelivery { .. }
			| Self::SetAutoContinue { .. }
			| Self::AuthorizeApprovalRetry { .. }
			| Self::ReviewRemoteTool { .. }
			| Self::RequestUtility { .. }
			| Self::ChangeExtension { .. }
			| Self::InstallCraft { .. }
			| Self::DisableCraft { .. }
			| Self::CreateSchedule { .. }
			| Self::CancelSchedule { .. }
			| Self::SetConversationName { .. }
			| Self::SetRunName { .. }
			| Self::SubmitTurn { .. }
			| Self::SubmitReview { .. }
			| Self::WithdrawTurn { .. }
			| Self::ControlRun { .. } => &[],
			Self::DeliverGit { .. }
			| Self::StartRun { .. }
			| Self::StartVisaRun(_)
			| Self::StartNoVisaRun(_) => GIT,
			Self::SetSetting {
				key: SettingKey::GitAutoCommit,
				value: SettingValue::Flag(true),
				..
			}
			| Self::RegisterProject { .. }
			| Self::ApplyUserEdit { .. }
			| Self::CreateConversation {
				working_tree: WorkingTreeRequest::Workspace { .. },
				..
			}
			| Self::ForkConversation { .. }
			| Self::HandoffConversation(_)
			| Self::ResumeImportedConversation {
				working_tree: WorkingTreeRequest::Workspace { .. },
				..
			}
			| Self::PromoteWorkspace { .. } => GIT,
			Self::BindAccount {
				credential_source: CredentialSource::PlatformStore,
				..
			} => CREDENTIAL_STORE,
			Self::BindAccount { .. }
			| Self::UnbindAccount { .. }
			| Self::BeginAuditEpoch
			| Self::RestoreRecoverySnapshot { .. }
			| Self::SetPairingGate { .. }
			| Self::OpenPairing { .. }
			| Self::ClaimPairing { .. }
			| Self::ConfirmPairing { .. }
			| Self::CompletePairing { .. }
			| Self::SetPairedClientAccess { .. }
			| Self::RevokePairedClient { .. }
			| Self::CreateConversation { .. }
			| Self::CreateRun { .. }
			| Self::ImportConversation { .. }
			| Self::ResumeImportedConversation { .. }
			| Self::SetSetting { .. }
			| Self::ClearSetting { .. }
			| Self::OpenTerminal { .. }
			| Self::CloseTerminal { .. }
			| Self::ResolveExecution(_)
			| Self::TransitionRun { .. } => &[],
		}
	}

	/// Whether this Command may run while the Plane cannot vouch for its
	/// Security audit (ADR-0105).
	///
	/// A change the audit exists to record is exactly a change that needs
	/// an audit worth recording it in, so the two answers come from the
	/// same place. Beginning an epoch is the way out and is never guarded.
	pub(crate) fn security_class(&self) -> SecurityClass {
		audit::decision_for(self)
			.map_or(SecurityClass::Ordinary, |_| SecurityClass::Guarded)
	}
}
