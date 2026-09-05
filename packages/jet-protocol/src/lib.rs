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
mod audit;
mod capability;
mod connection_auth;
mod control;
mod conversation;
mod decimal;
mod event;
mod frame;
mod handshake;
mod hex;
mod message;
mod pairing;
mod project;
mod setting;
mod stream;
mod stream_control;
mod stream_error;

pub use account::{
	AccountBinding, AccountBindingList, AccountBindingStatus, CredentialItem,
	CredentialReference, CredentialSource, CredentialState,
};
pub use artifact::{
	ArtifactError, ArtifactVerifier, DigestError, Sha256Digest,
};
pub use audit::{
	AuditBreach, AuditEntry, AuditHead, AuditOutcome, AuditRisk, AuditTarget,
	SecurityAudit, SecurityState,
};
pub use capability::{
	CapabilityObservation, CapabilitySnapshot, CredentialStoreKind,
	CredentialStoreStatus, DegradedCondition, ExternalTool, ExternalToolStatus,
	InstalledCraft, Platform, ToolAvailability,
};
pub use connection_auth::{
	ConnectionProof, RemotePairingRequest, RemotePairingResponse,
	connection_signing_bytes,
};
pub use control::{
	ControlError, MAX_COLLECTION_ITEMS, MAX_CONTROL_ITEMS, MAX_NESTING_DEPTH,
	decode_control, encode_control,
};
pub use conversation::{
	CommandRequest, CommandResponse, ConflictState, Conversation,
	ConversationList, ConversationSnapshot, PageCursor, RetentionPolicy,
	RevisionConflict, Run, RunLifecycle,
};
pub use event::{Actor, Event};
pub use frame::{
	CONNECTION_STREAM, Frame, FrameError, FrameKind, FrameLimits, FrameReader,
	FrameWriter, MAX_CONTROL_FRAME, MAX_DATA_FRAME, StreamId,
};
pub use handshake::{
	ACCOUNT_BINDINGS_MINOR, CODEC_JSON_V1, ClientHello, FENCED_READS_MINOR,
	MULTIPLEXED_STREAMS_MINOR, PAIRING_MINOR, PREFACE, PROJECTS_MINOR,
	PROTOCOL_MINOR, PROTOCOL_VERSION, REMOTE_AUTH_MINOR, SECURITY_AUDIT_MINOR,
	SETTINGS_AND_CAPABILITIES_MINOR, ServerHello, VersionRange,
};
pub use message::{
	ClientMessage, ErrorCategory, EventPage, PlaneStatus, QueryRequest,
	QueryResponse, RecoveryAction, RequestId, RestartMetadata, ServerMessage,
	WireError, raw_command,
};
pub use pairing::{
	ClientPublicKey, PairedClient, PairedClientAccess, PairingDisclosure,
	PairingEnd, PairingGate, PairingKeyAlgorithm, PairingMethod,
	PairingProgress, PairingSnapshot, PendingPairing,
};
pub use project::{
	Checkout, GitLink, Project, ProjectList, ProjectPreview, Registrability,
	Repository, Worktree,
};
pub use setting::{
	ResolvedSetting, SettingKey, SettingScope, SettingSelection,
	SettingSnapshot, SettingSource, SettingValue,
};
pub use stream::{
	BinaryStreamKind, DataQueueOutcome, MAX_BINARY_QUEUE_BYTES,
	MAX_CONTROL_QUEUE_BYTES, MAX_EVENT_WINDOW_BYTES, MAX_EVENT_WINDOW_EVENTS,
	MAX_OPEN_BINARY_STREAMS, OutboundLimits, OutboundQueue, StreamQueueError,
};
pub use stream_control::StreamControl;
