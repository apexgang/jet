//! Versioned Jet protocol wire types and bounded framing.
//!
//! This crate owns the wire representation shared by `jetd` and every Jet
//! client. It knows nothing about the core domain model: `jetd` translates
//! between domain types and these DTOs at the transport seam (ADR-0049).
//!
//! The v1 protocol is a small length-prefixed envelope carrying strict JSON
//! control frames and raw binary data frames; sequences and Revisions cross
//! the wire as decimal strings (ADR-0089). Every connection
//! starts with a fixed preface and a restricted handshake (ADR-0090).

mod account;
mod artifact;
pub use artifact::transfer::{ArtifactControl, ArtifactDescriptor};
pub use execution::resource::{PowerState, ResourceBudgets, SubagentControl};
pub use transport::handshake::{
	ARTIFACTS_MINOR, DISK_PRESSURE_MINOR, ENERGY_MINOR,
};
mod audit;
pub use conversation::checkpoint::{
	ArtifactAvailability, ChangeArtifact, ChangeArtifactChunk, ChangeDiff,
	ChangeOrigin, ChangeSnapshot, ChangedFile, DiffScope, TurnOutcome,
};
mod capability;
mod conversation;
mod craft;
pub use conversation::handoff::HandoffRequest;
pub use craft::change::CraftFileChange;
pub use craft::lifecycle::CraftDisableMode;
pub use craft::usage::{
	CraftObservedUsage, CraftQuotaScope, CraftQuotaUnit, CraftQuotaWindow,
	CraftUsage, CraftUsageEstimation, CraftUsageFinality,
	CraftUsageMeasurement, CraftUsageTokens, QUOTA_SHARE_LIMIT,
};
pub use execution::control::{RunControl, RunTermination, TerminationStage};
pub use execution::recovery::{
	ExecutionAction, ExecutionMetadata, ExecutionRole, OrphanedExecution,
	OrphanedExecutions,
};
#[cfg(feature = "schema")]
pub(crate) use transport::decimal::{Decimal, optional::OptionalDecimal};
pub use transport::handshake::CRAFT_LIFECYCLE_MINOR;
pub use transport::handshake::HANDOFFS_MINOR;
#[cfg(feature = "schema")]
pub(crate) use transport::hex::Hex;
mod message;
mod pairing;
mod presentation;
mod project;
mod search;
mod setting;
pub use conversation::schedule::{
	ScheduleFiring, ScheduledTask, ScheduledTasks,
};
pub use conversation::turn::{Turn, TurnQueue, TurnSource, TurnState};
pub use conversation::user_input::{
	EditableFile, FileRevision, FileTarget, ReviewComment,
};
pub use execution::terminal::{
	TerminalConfig, TerminalDescriptor, TerminalHelperAction,
	TerminalHelperReply, TerminalHelperRequest, TerminalState,
	WorkspaceTerminal,
};

pub use conversation::run::{
	ManagedProcess, ManagedProcessRole, RunActivity, RunExecution,
};
pub use execution::helper::{
	HelperCommand, HelperConfig, HelperDescriptor, HelperEvent, HelperHello,
	HelperReady, HelperRecord, HelperReplay, HelperSignalled, HelperTerminated,
	NativeInputMode, NativeSignal, NativeStream,
};

pub use account::{
	AccountBinding, AccountBindingList, AccountBindingStatus, CredentialItem,
	CredentialReference, CredentialSource, CredentialState,
};
pub use artifact::{
	ArtifactError, ArtifactVerifier, DigestError, Sha256Digest,
};
pub use audit::{
	AuditActor, AuditBreach, AuditEntry, AuditHead, AuditOutcome, AuditRisk,
	AuditTarget, SecurityAudit, SecurityState,
};
pub use capability::{
	CapabilityObservation, CapabilitySnapshot, CredentialStoreKind,
	CredentialStoreStatus, DegradedCondition, ExternalTool, ExternalToolStatus,
	InstalledCraft, Platform, ToolAvailability,
};
pub use conversation::event::{Actor, Event, EventOrigin};
pub use conversation::{
	CommandRequest, CommandResponse, ConflictState, Conversation,
	ConversationList, ConversationSnapshot, PageCursor, RetentionPolicy,
	RevisionConflict, Run, RunLifecycle,
};
pub use craft::handshake::{CraftFork, CraftHello, CraftReady, CraftResume};
pub use craft::installation::{
	CraftInstallationConfirmation, CraftInstallationPreview,
	CraftInstallationQueued, CraftSource, CraftTrust,
};
pub use craft::spec::{
	BrokerPermission, CraftFeature, CraftHostAccess, CraftSpecification,
};
pub use craft::{
	CraftAction, CraftApprovalDecision, CraftApprovalRequest, CraftCommand,
	CraftEvent,
};
pub use transport::compatibility::{
	IncompatibleProtocol, NegotiatedProtocol, Negotiation, ProtocolFamily,
	ProtocolOffer, ProtocolVersion,
};
pub use transport::connection_auth::{
	ConnectionProof, RemotePairingRequest, RemotePairingResponse,
	connection_signing_bytes,
};
pub use transport::control::{
	ControlError, MAX_COLLECTION_ITEMS, MAX_CONTROL_ITEMS, MAX_NESTING_DEPTH,
	decode_control, encode_control,
};
pub use transport::frame::{
	CONNECTION_STREAM, Frame, FrameError, FrameKind, FrameLimits, FrameReader,
	FrameWriter, MAX_CONTROL_FRAME, MAX_DATA_FRAME, StreamId,
};
pub use transport::handshake::{
	ACCOUNT_BINDINGS_MINOR, CHANGE_CHECKPOINTS_MINOR, CODEC_JSON_V1,
	CONVERSATION_FORKS_MINOR, CRAFT_INSTALLATION_MINOR, ClientHello,
	EXECUTION_CONTROL_MINOR, EXECUTION_RECOVERY_MINOR, FENCED_READS_MINOR,
	IMPORTED_CONVERSATIONS_MINOR, MANAGED_RUNS_MINOR,
	MULTIPLEXED_STREAMS_MINOR, NAMES_MINOR, PAIRING_MINOR, PREFACE,
	PROJECTS_MINOR, PROTOCOL_MINOR, PROTOCOL_VERSION, REMOTE_AUTH_MINOR,
	SCHEDULES_MINOR, SEARCH_MINOR, SECURITY_AUDIT_MINOR,
	SEEDED_WORKSPACES_MINOR, SETTINGS_AND_CAPABILITIES_MINOR, ServerHello,
	TURN_QUEUE_MINOR, USAGE_RECORDS_MINOR, USER_INPUT_MINOR, UTILITY_MINOR,
	VersionRange, WORKSPACE_PROMOTION_MINOR, WORKSPACE_TERMINALS_MINOR,
	WORKSPACES_MINOR,
};
mod usage;
pub use conversation::import::{
	ConversationOrigin, ExternalConversation, ExternalConversationList,
	ExternalOrigin, ExternalProcess, ImportedConversation,
};
pub use conversation::name::{Name, NameSource};
pub use conversation::promotion::{
	ChangeKind, ConflictKind, PromotedChange, PromotionBinding,
	PromotionConflict, PromotionDestination, PromotionPreview, PromotionState,
	WorkspacePromotion,
};
pub use conversation::workspace::{
	BaseSelection, SeedSelection, WorkingTree, WorkingTreeRequest, Workspace,
	WorkspaceBase, WorkspaceSeed,
};
pub use message::{
	ClientMessage, ErrorCategory, EventPage, MAX_QUERY_TIMEOUT_MS, PlaneStatus,
	QueryRequest, QueryResponse, RecoveryAction, RequestId, RestartMetadata,
	ServerMessage, WireError, raw_command,
};
pub use pairing::{
	ClientPublicKey, PairedClient, PairedClientAccess, PairingDisclosure,
	PairingEnd, PairingGate, PairingKeyAlgorithm, PairingMethod,
	PairingProgress, PairingSnapshot, PendingPairing,
};
pub use presentation::{Presentation, PresentationAction, PresentationBlock};
pub use project::{
	Checkout, EntryKind, GitLink, Project, ProjectEntry, ProjectList,
	ProjectPreview, Registrability, Repository, Worktree,
};
pub use remote::no_visa::{
	CraftRemoteTool, NoVisaOrigin, RemoteEnvironment, RemoteGitOperation,
	RemoteToolAction, RemoteToolDecision, RemoteToolOutcome, RemoteToolRequest,
	RemoteToolResult,
};
pub use remote::no_visa_run::{
	NoVisaDestination, NoVisaRunRequest, NoVisaSelection,
};
pub use remote::visa::{VisaRunRequest, VisaSelection};
pub use search::{SearchField, SearchHit, SearchResult};
pub use setting::{
	ResolvedSetting, SettingKey, SettingScope, SettingSelection,
	SettingSnapshot, SettingSource, SettingValue,
};
pub use transport::handshake::VISA_RUNS_MINOR;
pub use transport::handshake::{
	APPROVAL_RETRY_MINOR, AUTOMATIC_REVIEW_MINOR, NO_VISA_MINOR,
};
pub use transport::stream::{
	BinaryStreamKind, DataQueueOutcome, MAX_BINARY_QUEUE_BYTES,
	MAX_CONTROL_QUEUE_BYTES, MAX_EVENT_WINDOW_BYTES, MAX_EVENT_WINDOW_EVENTS,
	MAX_OPEN_BINARY_STREAMS, OutboundLimits, OutboundQueue, StreamQueueError,
};
pub use transport::stream_control::StreamControl;
pub use usage::{
	ModelConsumption, ObservedConsumption, PlaneUsage, QuotaMeasure,
	QuotaScope, QuotaUnit, QuotaWindow, UsageEstimation, UsageFinality,
	UsageFreshness, UsageSelection, UsageTokens,
};

mod utility;
pub use craft::review::{
	CraftReviewInput, CraftReviewModel, CraftReviewReply, CraftReviewRequest,
	CraftReviewer,
};
pub use craft::utility::{
	CraftUtilityModel, CraftUtilityReply, CraftUtilityRequest, UtilityInput,
};
pub use utility::{
	UtilityJob, UtilityOutcome, UtilityPolicy, UtilityPurpose, UtilityRequest,
};

mod extension;
pub use extension::{
	CraftExtensionReply, CraftExtensionRequest, ExtensionAction,
	ExtensionCatalog, ExtensionChange, ExtensionChangeState,
	ExtensionConfirmation, ExtensionScope, ExtensionTrust,
};

pub use transport::handshake::EXTENSIONS_MINOR;

pub use conversation::auto_continue::{
	AutoContinuePolicy, AutoContinueRetry, AutoContinueSnapshot,
	AutoContinueStatus, AutoContinueTarget, AutoContinueUsage,
};
pub use transport::handshake::AUTO_CONTINUE_MINOR;

pub use transport::handshake::GIT_DELIVERY_MINOR;

pub use conversation::git_delivery::{
	GitCheckpoint, GitDelivery, GitDeliveryOutcome, GitDeliveryPolicy,
	GitMessage, GitOperation,
};

mod transport;

mod remote;

mod execution;
