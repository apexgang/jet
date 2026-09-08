//! Jet core: the deep Module behind `jetd` (ADR-0047).
//!
//! The core exposes three conceptual entry points: execute an authenticated
//! Command, run a Query returning a snapshot, and subscribe from an Event
//! journal cursor. This slice implements Commands and fenced Queries; live
//! subscriptions arrive with the Run execution work.
//!
//! Domain types here never double as wire types (ADR-0049); `jetd`
//! translates at the transport seam.

mod account;
mod audit;
mod capability;
mod capability_probe;
mod change_artifact;
mod change_artifact_budget;
mod change_evidence;
mod checkpoint;
mod checkpoint_capture;
mod checkpoint_omissions;
mod checkpoint_pages;
mod checkpoint_query;
mod checkpoint_state;
pub use checkpoint::{
	ArtifactAvailability, ChangeArtifact, ChangeArtifactChunk,
	ChangeCheckpoint, ChangeDiff, ChangeEvidence, ChangeOrigin, ChangeSnapshot,
	ChangedFile, DiffScope, OmittedFile, TurnOutcome,
};
mod clock;
mod command;
mod command_receipt;
mod conversation;
mod craft_artifact_collection;
mod craft_installation;
mod craft_local_source;
mod craft_publication;
mod craft_repository;
mod craft_specification;
mod discovery;
mod effect;
mod error;
mod event;
mod event_query;
mod execution_control;
mod execution_control_effect;
mod filesystem;
mod fork;
mod import;
mod lifecycle;
mod name;
mod orphan;
mod pagination;
mod paired_client;
mod pairing;
mod pairing_completion;
mod pairing_identity;
mod pairing_offer;
mod pairing_secret;
mod preparation;
mod project;
mod project_entry;
mod promotion;
mod promotion_apply;
mod promotion_command;
mod promotion_effect;
mod promotion_merge;
mod query;
mod queued_run;
mod relative_path;
mod remote;
mod remote_pairing;
mod repository;
mod run;
mod run_command;
mod run_craft;
mod run_effect;
mod run_host;
mod run_observation;
mod run_recovery;
mod run_state;
mod run_state_storage;
mod turn;
mod turn_dispatch;
mod turn_queue;
mod user_input;
mod user_input_files;
pub use execution_control::{
	ExecutionSignal, RunControl, RunTermination, TerminationStage,
};
pub use orphan::{
	ExecutionAction, ExecutionMetadata, ExecutionResolution, ExecutionRole,
	OrphanedExecution, OrphanedExecutions,
};
pub use turn::{Turn, TurnQueue, TurnSource, TurnState};
pub use user_input::{
	EditableFile, FileRevision, FileTarget, ReviewComment, UserEdit,
};
mod schedule;
mod schedule_clock;
mod schedule_work;
mod search;
mod search_index;
mod security;
mod seed;
mod seed_capture;
mod setting;
pub use schedule::{
	ScheduleFiring, ScheduleFiringOutcome, ScheduledTask, ScheduledTasks,
};
mod status;
mod terminal;
mod terminal_command;
mod terminal_effect;
#[cfg(test)]
mod test_support;
mod tree_capture;
mod workspace;
pub use terminal::{
	TerminalHost, TerminalId, TerminalOperation, TerminalOutput, TerminalPlan,
	TerminalState, WorkspaceTerminal,
};
mod worktree;
pub use run::{ManagedProcess, ManagedProcessRole, RunActivity, RunExecution};
pub use run_command::{ForkLaunchSource, LaunchFork, LaunchPlan};
pub use run_craft::PinnedCraft;
pub use run_host::{
	RunConnection, RunFuture, RunHost, RunRecoveryCursor, RunRecoveryError,
	RunStartError,
};
pub use run_observation::Observation as RunObservation;

#[cfg(test)]
#[path = "run_tests.rs"]
mod run_tests;

#[cfg(test)]
#[path = "checkpoint_tests.rs"]
mod checkpoint_tests;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jet_store::{ActorRecord, Store};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use capability::CapabilityProbe;
use capability_probe::SystemCapabilityProbe;
use clock::{Clock, SystemClock};
use craft_repository::{CraftRepository, SystemCraftRepository};
use discovery::{ConversationDiscovery, SystemConversationDiscovery};

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
pub use conversation::{
	Conversation, ConversationId, ConversationList, ConversationOrigin,
	ConversationSnapshot, PageCursor, Revision, Run, RunId,
};
pub use craft_installation::{
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
pub use import::{
	DiscoveredConversation, ExternalConversation, ExternalConversationList,
	ExternalOrigin, ExternalProcess, ImportId, ImportedConversation,
	NativeConversationId,
};
pub use jet_store::{AuditBreach, AuditHead};
pub use jet_store::{
	AuditEntryHash, AuditOutcome, AuditRisk, AuditTargetRef,
	PairedClientAccess, PairingGate, PairingKeyAlgorithm, PairingMethod,
	RetentionPolicy, RunLifecycle,
};
pub use name::{MAX_NAME_BYTES, Name, NameSource};
pub use pairing::{
	AuthenticationString, ClientPublicKey, PairedClient, PairingChallenge,
	PairingDisclosure, PairingEnd, PairingOfferId, PairingProgress,
	PairingSecret, PairingSignature, PairingSnapshot, PendingPairing,
};
pub use project::{
	Checkout, GitLink, PathGrant, Project, ProjectList, ProjectPreview,
	Registrability, Repository, Worktree,
};
pub use project_entry::{EntryKind, ProjectEntry};
pub use promotion::{
	ChangeKind, ConflictKind, PromotedChange, PromotionBinding,
	PromotionConflict, PromotionDestination, PromotionId, PromotionPreview,
	PromotionState, WorkspacePromotion,
};
pub use query::{Query, QueryResult};
pub use relative_path::RelativePath;
pub use remote::RemoteSession;
pub use search::{SearchField, SearchHit, SearchResult, SearchTerms};
pub use security::{SecurityDegradation, SecurityState};
pub use seed::{SeedSelection, WorkspaceSeed};
pub use setting::{
	ResolvedSetting, SettingKey, SettingScope, SettingSelection,
	SettingSnapshot, SettingSource, SettingValue,
};
pub use status::PlaneStatus;
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
	utility_host: Option<Arc<dyn UtilityHost>>,
	utility_work: tokio::sync::Mutex<()>,
	run_work: tokio::sync::Notify,
	utility_wake: tokio::sync::Notify,
	turn_wake: tokio::sync::watch::Sender<()>,
	run_host: Option<Arc<dyn run_host::RunHost>>,
	terminal_host: Option<Arc<dyn terminal::TerminalHost>>,
	run_recovery: run_recovery::Recovery,
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
	started_at: SystemTime,
	/// Serializes every Effect decision, so two workers never perform the
	/// same durable request at once (ADR-0067).
	effect_reconciliation: tokio::sync::Mutex<()>,
	/// Serializes pre-transaction Craft publication with orphan collection.
	craft_artifact_publication: tokio::sync::Mutex<()>,
	conversation_pages: pagination::ConversationPages,
	checkpoint_pages: checkpoint_pages::Pages,
	/// Where this core creates Workspaces (ADR-0025).
	workspace_home: workspace::WorkspaceHome,
	/// How this core sees the Harness-native Conversations outside its
	/// management (ADR-0010).
	discovery: Arc<dyn ConversationDiscovery>,
	/// How this core reads versioned Craft releases from their repository.
	craft_repository: Arc<dyn CraftRepository>,
}

impl Core {
	/// Installs the trusted host Adapter before accepting managed Runs.
	pub fn with_run_host(mut self, host: Arc<dyn run_host::RunHost>) -> Self {
		self.run_host = Some(host);
		self
	}

	#[cfg(test)]
	pub(crate) fn with_craft_repository(
		mut self,
		repository: Arc<dyn CraftRepository>,
	) -> Self {
		self.craft_repository = repository;
		self
	}

	/// Starts the core on `store`, durably recording this daemon start.
	///
	/// # Errors
	///
	/// Returns [`CoreError`] with an `unavailable` or `internal` category
	/// when the start cannot be committed.
	pub async fn start(
		store: Store,
		workspace_home: WorkspaceHome,
	) -> Result<Self, CoreError> {
		Self::start_with(
			store,
			workspace_home,
			Arc::new(SystemClock),
			Arc::new(SystemCapabilityProbe),
			Arc::new(SystemConversationDiscovery),
		)
		.await
	}

	/// Starts the core with an injected wall clock, Capability probe, and
	/// Conversation discovery, observing the Plane once so its first report
	/// needs no waiting.
	///
	/// # Errors
	///
	/// Returns [`CoreError`] with an `unavailable` or `internal` category
	/// when the start cannot be committed.
	pub(crate) async fn start_with(
		store: Store,
		workspace_home: WorkspaceHome,
		clock: Arc<dyn Clock>,
		probe: Arc<dyn CapabilityProbe>,
		discovery: Arc<dyn ConversationDiscovery>,
	) -> Result<Self, CoreError> {
		store.record_daemon_start().await?;
		let started_at = clock.now();
		// Retention runs only behind a whole chain. A store that moved
		// backwards keeps every record it still has until an owner has
		// seen the evidence and decided what to do (ADR-0105).
		let security = SecurityState::of(store.validate_audit().await?);
		let craft_home = workspace_home
			.0
			.parent()
			.expect("Workspace home has a parent")
			.join("crafts");
		if security == SecurityState::Trusted {
			audit::sweep_retention(&store, unix_ms(started_at)).await?;
			craft_artifact_collection::collect_unreferenced(
				&store,
				craft_home.clone(),
				started_at,
			)
			.await?;
		}
		let mut observed = probe.observe().await;
		observed
			.crafts
			.extend(craft_publication::installed_crafts(craft_home).await);
		let capabilities =
			CapabilitySnapshot::from_observation(observed, started_at);
		let core = Self {
			utility_host: None,
			utility_work: tokio::sync::Mutex::new(()),
			run_work: tokio::sync::Notify::new(),
			utility_wake: tokio::sync::Notify::new(),
			turn_wake: tokio::sync::watch::channel(()).0,
			run_host: None,
			terminal_host: None,
			run_recovery: run_recovery::Recovery::default(),
			remote_access: tokio::sync::Semaphore::new(
				remote::AUTHORITY_READERS as usize,
			),
			remote_sessions: remote::RemoteSessions::default(),
			store,
			clock,
			probe,
			capabilities: tokio::sync::RwLock::new(capabilities),
			security: tokio::sync::RwLock::new(security),
			started_at,
			effect_reconciliation: tokio::sync::Mutex::new(()),
			craft_artifact_publication: tokio::sync::Mutex::new(()),
			conversation_pages: pagination::ConversationPages::default(),
			checkpoint_pages: checkpoint_pages::Pages::default(),
			workspace_home,
			discovery,
			craft_repository: Arc::new(SystemCraftRepository),
		};
		// The index follows the journal; a daemon that stopped between a
		// Command and its indexing catches up here (ADR-0036).
		core.index_search().await?;
		Ok(core)
	}

	/// What the Plane could do when it was last observed. `jetd` reports
	/// this at startup, before any client has connected to ask for it
	/// (ADR-0086).
	pub async fn capabilities(&self) -> CapabilitySnapshot {
		self.capabilities.read().await.clone()
	}

	/// Whether the Plane can vouch for its own Security audit right now
	/// (ADR-0105).
	pub async fn security(&self) -> SecurityState {
		*self.security.read().await
	}

	/// Observes the Plane again and keeps the result as its latest
	/// snapshot.
	pub(crate) async fn observe_capabilities(&self) -> CapabilitySnapshot {
		let mut observed = self.probe.observe().await;
		observed.crafts.extend(
			craft_publication::installed_crafts(self.run_home().join("crafts"))
				.await,
		);
		let snapshot =
			CapabilitySnapshot::from_observation(observed, self.clock.now());
		*self.capabilities.write().await = snapshot.clone();
		snapshot
	}

	/// The core clock's current time as the store records it. Every stamp
	/// written by one Command comes from this one reading.
	pub(crate) fn now_unix_ms(&self) -> i64 {
		unix_ms(self.clock.now())
	}

	/// Closes the Plane store, letting SQLite finish its write-ahead log
	/// checkpoint before the process exits. The core answers nothing
	/// afterwards, so only a daemon that has stopped serving calls this.
	pub async fn close(&self) {
		self.store.close().await;
	}
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

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "effect_tests.rs"]
mod effect_tests;

#[cfg(test)]
#[path = "setting_tests.rs"]
mod setting_tests;

#[cfg(test)]
#[path = "capability_tests.rs"]
mod capability_tests;

#[cfg(test)]
#[path = "craft_installation_tests.rs"]
mod craft_installation_tests;

#[cfg(test)]
#[path = "account_tests.rs"]
mod account_tests;

#[cfg(test)]
#[path = "audit_tests.rs"]
mod audit_tests;

#[cfg(test)]
#[path = "security_tests.rs"]
mod security_tests;

#[cfg(test)]
#[path = "pairing_tests.rs"]
mod pairing_tests;

#[cfg(test)]
#[path = "pairing_offer_tests.rs"]
mod pairing_offer_tests;

#[cfg(test)]
#[path = "pairing_completion_tests.rs"]
mod pairing_completion_tests;

#[cfg(test)]
#[path = "paired_client_tests.rs"]
mod paired_client_tests;

#[cfg(test)]
#[path = "search_tests.rs"]
mod search_tests;

mod terminal_orphan;

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod schedule_tests;

#[cfg(test)]
#[path = "utility_tests.rs"]
mod utility_tests;

mod utility;
pub use utility::{UtilityJob, UtilityOutcome, UtilityPurpose, UtilityRequest};

mod utility_host;
mod utility_work;
pub use utility::UtilityPolicy;
pub use utility_host::{UtilityHost, UtilityInput, UtilityModel, UtilityReply};
mod utility_input;
mod utility_output;
