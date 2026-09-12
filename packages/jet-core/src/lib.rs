//! Jet core: the deep Module behind `jetd` (ADR-0047).
//!
//! The core exposes three conceptual entry points: execute an authenticated
//! Command, run a Query returning a snapshot, and subscribe from an Event
//! journal cursor. This slice implements Commands and fenced Queries; live
//! subscriptions arrive with the Run execution work.
//!
//! Domain types here never double as wire types (ADR-0049); `jetd`
//! translates at the transport seam.

mod startup;

mod account;
mod artifact;
pub use artifact::{
	ArtifactDescriptor, ArtifactDownload, ArtifactLimits, ArtifactUpload,
};

pub use remote::no_visa_run::{
	NoVisaDestination, NoVisaRunRequest, NoVisaSelection,
};
pub use remote::tool_types::{
	NoVisaOrigin, RemoteEnvironment, RemoteGitOperation, RemoteToolAction,
	RemoteToolDecision, RemoteToolRequest, RemoteToolResult,
};
pub use remote::visa::{VisaRunRequest, VisaSelection};
pub use remote::work::RemoteWork;
mod audit;
pub use audit::actor::AuditActor;
mod capability;
mod checkpoint;
mod disk_pressure;

pub use checkpoint::{
	ArtifactAvailability, ChangeArtifact, ChangeArtifactChunk,
	ChangeCheckpoint, ChangeDiff, ChangeEvidence, ChangeOrigin, ChangeSnapshot,
	ChangedFile, DiffScope, OmittedFile, TurnOutcome,
};
mod clock;
mod command;
mod conversation;
pub use craft::lifecycle::CraftDisableMode;
mod effect;
mod energy;
mod error;
mod event;
mod maintenance;
pub use energy::ChildWork;
mod filesystem;
pub use conversation::handoff::{HandoffProvenance, HandoffRequest};
mod pairing;
mod project;
mod promotion;
mod query;
mod remote;
mod review;
pub use review::{
	ApprovalRequest, ApprovalReview, AutomaticReviewPolicy,
	ReviewAuthorization, ReviewDecision, ReviewHost, ReviewInput,
	ReviewOutcome, ReviewReply, ReviewRisk, ReviewVerdict, Reviewer,
	ReviewerSelection,
};
mod run;
mod turn;
mod usage;
mod user_input;
pub use run::execution_control::{
	ExecutionSignal, RunControl, RunTermination, TerminationStage,
};
pub use run::orphan::{
	ExecutionAction, ExecutionMetadata, ExecutionResolution, ExecutionRole,
	OrphanedExecution, OrphanedExecutions,
};
pub use turn::{Turn, TurnQueue, TurnSource, TurnState};
pub use user_input::{
	EditableFile, FileRevision, FileTarget, ReviewComment, UserEdit,
};
mod schedule;
mod search;
mod security;
mod setting;
pub use schedule::{
	ScheduleFiring, ScheduleFiringOutcome, ScheduledTask, ScheduledTasks,
};
mod status;
mod store_recovery;
pub use store_recovery::{
	IntegrityFailureReason, RecoveryMode, RecoverySnapshot, RecoveryStatus,
	RestoredStore, SnapshotReason,
};
mod terminal;
#[cfg(test)]
mod test_support;
mod workspace;
pub use run::command::{ForkLaunchSource, LaunchFork, LaunchPlan};
pub use run::craft::PinnedCraft;
pub use run::host::{
	RunConnection, RunFuture, RunHost, RunRecoveryCursor, RunRecoveryError,
	RunStartError,
};
pub use run::observation::Observation as RunObservation;
pub use run::{ManagedProcess, ManagedProcessRole, RunActivity, RunExecution};
pub use terminal::{
	TerminalHost, TerminalId, TerminalOperation, TerminalOutput, TerminalPlan,
	TerminalState, WorkspaceTerminal,
};

use capability::CapabilityProbe;
use clock::Clock;
use conversation::discovery::ConversationDiscovery;
use craft::repository::CraftRepository;
use jet_store::{ActorRecord, Store};
use serde::{Deserialize, Serialize};
use std::{
	sync::Arc,
	time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub use account::{
	AccountBinding, AccountBindingId, AccountBindingList, AccountBindingStatus,
	CredentialItem, CredentialReference, CredentialSource, CredentialState,
	ProviderAccount, ProviderId,
};
pub use audit::{
	AuditDecision, AuditEntry, AuditEpoch, AuditPage, AuditRecordId,
	AuditSequence, AuditTarget,
};
pub use capability::{
	CapabilityObservation, CapabilitySnapshot, CraftId, CredentialStoreKind,
	CredentialStoreStatus, DegradedCondition, ExternalTool, ExternalToolStatus,
	HarnessId, InstalledCraft, Platform, ToolAvailability,
};
pub use command::{Command, CommandEnvelope, CommandId, CommandOutcome};
pub use conversation::import::{
	DiscoveredConversation, ExternalConversation, ExternalConversationList,
	ExternalOrigin, ExternalProcess, ImportId, ImportedConversation,
	NativeConversationId,
};
pub use conversation::name::{MAX_NAME_BYTES, Name, NameSource};
pub use conversation::{
	Conversation, ConversationId, ConversationList, ConversationOrigin,
	ConversationSnapshot, PageCursor, Revision, Run, RunId,
};
pub use craft::installation::{
	BrokerPermission, CraftHostAccess, CraftInstallationConfirmation,
	CraftInstallationPreview, CraftSource, CraftTrust,
};
pub use error::{
	ConflictState, CoreError, ErrorCategory, RecoveryAction, RestartMetadata,
	RevisionConflict,
};
pub use event::{
	Event, EventActor, EventId, EventKind, EventPage, EventPayload,
	EventSequence,
};
pub use filesystem::relative_path::RelativePath;
pub use jet_store::{AuditBreach, AuditHead};
pub use jet_store::{
	AuditEntryHash, AuditOutcome, AuditRisk, AuditTargetRef,
	PairedClientAccess, PairingGate, PairingKeyAlgorithm, PairingMethod,
	RetentionPolicy, RunLifecycle,
};
pub use pairing::{
	AuthenticationString, ClientPublicKey, PairedClient, PairingChallenge,
	PairingDisclosure, PairingEnd, PairingOfferId, PairingProgress,
	PairingSecret, PairingSignature, PairingSnapshot, PendingPairing,
};
pub use project::entry::{EntryKind, ProjectEntry};
pub use project::{
	Checkout, GitLink, PathGrant, Project, ProjectList, ProjectPreview,
	Registrability, Repository, Worktree,
};
pub use promotion::{
	ChangeKind, ConflictKind, PromotedChange, PromotionBinding,
	PromotionConflict, PromotionDestination, PromotionId, PromotionPreview,
	PromotionState, WorkspacePromotion,
};
pub use query::{Query, QueryResult};
pub use remote::RemoteSession;
pub use search::{SearchField, SearchHit, SearchResult, SearchTerms};
pub use security::{SecurityDegradation, SecurityState};
pub use setting::{
	ResolvedSetting, SettingKey, SettingScope, SettingSelection,
	SettingSnapshot, SettingSource, SettingValue,
};
pub use status::PlaneStatus;
pub use usage::{
	ModelConsumption, ModelId, ObservedConsumption, ObservedUsage, PlaneUsage,
	QuotaMeasure, QuotaReport, QuotaScope, QuotaUnit, QuotaWindow,
	UsageEstimation, UsageFinality, UsageFreshness, UsageMeasurement,
	UsageReport, UsageSelection, UsageSource, UsageTokens,
};
pub use workspace::seed::{SeedSelection, WorkspaceSeed};
pub use workspace::{
	BaseSelection, WorkingTree, WorkingTreeRequest, Workspace, WorkspaceBase,
	WorkspaceHome, WorkspaceId,
};

/// Version of the running core, reported in status snapshots.
pub(crate) const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Durable identity of one Jet installation (see `Client identity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClientId(pub Uuid);

/// Durable identity of one Plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlaneId(pub Uuid);

/// Durable identity of one registered Project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectId(pub Uuid);

/// The authenticated origin of a Command or Query (ADR-0063).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
	/// An interactive client authorized by a fresh Paired-client signature.
	RemoteClient {
		/// Live revocable authority, issued only by this core's authentication.
		session: RemoteSession,
	},
	/// An interactive GUI client authorized through owner-only local IPC.
	InteractiveClient {
		/// The client's durable identity.
		client_id: ClientId,
	},
}

impl Actor {
	/// Checks that the Actor may drive and read this Plane's Conversations.
	/// Every authenticated local Actor may in this slice; remote and
	/// automation Actors arrive with their own rules (ADR-0063).
	fn authorize(
		&self,
		sessions: &remote::RemoteSessions,
	) -> Result<(), CoreError> {
		match self {
			Self::RemoteClient { session } => sessions.authorize(session),
			Self::InteractiveClient { .. } => Ok(()),
		}
	}

	/// The Client identity this Actor acts through.
	pub fn client_id(&self) -> ClientId {
		match self {
			Self::RemoteClient { session } => session.client_id(),
			Self::InteractiveClient { client_id } => *client_id,
		}
	}

	fn record(&self) -> ActorRecord {
		ActorRecord::InteractiveClient {
			client_id: self.client_id().0,
		}
	}

	fn from_record(record: ActorRecord) -> Self {
		match record {
			ActorRecord::InteractiveClient { client_id } => {
				Self::InteractiveClient {
					client_id: ClientId(client_id),
				}
			}
		}
	}
}

/// One running core bound to one Plane store.
#[derive(Debug)]
pub struct Core {
	github_host: Arc<dyn GitHubHost>,
	artifact_limits: ArtifactLimits,
	artifact_publication: tokio::sync::Mutex<artifact::collection::Publication>,
	extension_host: Option<Arc<dyn ExtensionHost>>,
	remote_worker: Option<std::path::PathBuf>,
	remote_tool_slots: tokio::sync::Semaphore,
	utility_host: Option<Arc<dyn UtilityHost>>,
	review_host: Option<Arc<dyn ReviewHost>>,
	utility_work: tokio::sync::Mutex<()>,
	run_work: tokio::sync::Notify,
	maintenance_work: tokio::sync::Notify,
	utility_wake: tokio::sync::Notify,
	turn_wake: tokio::sync::watch::Sender<()>,
	run_host: Option<Arc<dyn run::host::RunHost>>,
	terminal_host: Option<Arc<dyn terminal::TerminalHost>>,
	run_recovery: run::recovery::Recovery,
	// Serialize authority publication with Commands and fence concurrent reads.
	remote_access: tokio::sync::Semaphore,
	remote_sessions: remote::RemoteSessions,
	store: Store,
	clock: Arc<dyn Clock>,
	probe: Arc<dyn CapabilityProbe>,
	/// What the Plane could do when it was last observed. Nothing refreshes
	/// it on a timer: a Query or a Command that depends on a Capability
	/// observes the Plane again and leaves the result here (ADR-0086).
	capabilities: tokio::sync::RwLock<CapabilitySnapshot>,
	/// Whether the Plane can vouch for its own Security audit. It is
	/// decided once, when the daemon starts, and changes only when an owner
	/// begins a new audit epoch (ADR-0105).
	security: tokio::sync::RwLock<SecurityState>,
	/// Whether the store serves or answers reads only. Decided when the
	/// store is opened, and changed only by restoring a snapshot
	/// (ADR-0077). Workers wait on it before touching the store.
	recovery: tokio::sync::watch::Sender<store_recovery::RecoveryMode>,
	started_at: SystemTime,
	/// Serializes every Effect decision, so two workers never perform the
	/// same durable request at once (ADR-0067).
	effect_reconciliation: tokio::sync::Mutex<()>,
	/// Serializes pre-transaction Craft publication with orphan collection.
	craft_artifact_publication: tokio::sync::Mutex<()>,
	conversation_pages: conversation::pagination::ConversationPages,
	checkpoint_pages: checkpoint::pages::Pages,
	/// Where this core creates Workspaces (ADR-0025).
	workspace_home: workspace::WorkspaceHome,
	/// How this core sees the Harness-native Conversations outside its
	/// management (ADR-0010).
	discovery: Arc<dyn ConversationDiscovery>,
	/// How this core reads versioned Craft releases from their repository.
	craft_repository: Arc<dyn CraftRepository>,
}

/// Converts a stored wall-clock stamp back into a [`SystemTime`].
fn system_time(unix_ms: i64) -> SystemTime {
	UNIX_EPOCH + Duration::from_millis(u64::try_from(unix_ms).unwrap_or(0))
}

fn unix_ms(time: SystemTime) -> i64 {
	match time.duration_since(UNIX_EPOCH) {
		Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
		Err(behind) => i64::try_from(behind.duration().as_millis())
			.map_or(i64::MIN, |ms| -ms),
	}
}

mod utility;
pub use utility::{UtilityJob, UtilityOutcome, UtilityPurpose, UtilityRequest};

pub use utility::UtilityPolicy;
pub use utility::host::{
	UtilityHost, UtilityInput, UtilityModel, UtilityReply,
};

mod extension;
pub use extension::{
	ExtensionAction, ExtensionCatalog, ExtensionChange, ExtensionChangeState,
	ExtensionConfirmation, ExtensionHost, ExtensionScope, ExtensionTrust,
};

mod auto_continue;
pub use auto_continue::{
	AutoContinuePolicy, AutoContinueRetry, AutoContinueSnapshot,
	AutoContinueStatus, AutoContinueTarget,
};

mod git_delivery;
pub use git_delivery::{
	GitCheckpoint, GitDelivery, GitDeliveryOutcome, GitDeliveryPolicy,
	GitMessage, GitOperation,
};

pub use git_delivery::github_host::{GitHubHost, GitHubRequest};

mod craft;
