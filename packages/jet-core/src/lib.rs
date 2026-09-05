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
mod clock;
mod command;
mod command_receipt;
mod conversation;
#[allow(dead_code, reason = "wired to the Harness by follow-up issue #20")]
mod effect;
mod error;
mod event;
mod lifecycle;
mod pagination;
mod paired_client;
mod pairing;
mod pairing_completion;
mod pairing_identity;
mod pairing_offer;
mod pairing_secret;
mod query;
#[allow(
	dead_code,
	reason = "used by Query::ProjectEntry in the next stage of #15"
)]
mod relative_path;
mod remote;
mod remote_pairing;
mod security;
mod setting;
mod status;
#[cfg(test)]
mod test_support;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jet_store::{ActorRecord, Store};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use capability::CapabilityProbe;
use capability_probe::SystemCapabilityProbe;
use clock::{Clock, SystemClock};

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
	Conversation, ConversationId, ConversationList, ConversationSnapshot,
	PageCursor, Revision, Run, RunId,
};
pub use error::{
	ConflictState, CoreError, ErrorCategory, RecoveryAction, RestartMetadata,
	RevisionConflict,
};
pub use event::{
	Event, EventId, EventKind, EventPage, EventPayload, EventSequence,
};
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
pub use query::{Query, QueryResult};
pub use remote::RemoteSession;
pub use security::{SecurityDegradation, SecurityState};
pub use setting::{
	ResolvedSetting, SettingKey, SettingScope, SettingSelection,
	SettingSnapshot, SettingSource, SettingValue,
};
pub use status::PlaneStatus;

/// Version of the running core, reported in status snapshots.
pub(crate) const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Durable identity of one Jet installation (see `Client identity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClientId(pub Uuid);

/// Durable identity of one Plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlaneId(pub Uuid);

/// Durable identity of one registered Project. The Project registry itself
/// arrives with Project registration; Settings already resolve through the
/// scope it names (ADR-0085).
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
	#[allow(dead_code, reason = "used by Effect reconciliation in issue #20")]
	effect_reconciliation: tokio::sync::Mutex<()>,
	conversation_pages: pagination::ConversationPages,
}

impl Core {
	/// Starts the core on `store`, durably recording this daemon start.
	///
	/// # Errors
	///
	/// Returns [`CoreError`] with an `unavailable` or `internal` category
	/// when the start cannot be committed.
	pub async fn start(store: Store) -> Result<Self, CoreError> {
		Self::start_with(
			store,
			Arc::new(SystemClock),
			Arc::new(SystemCapabilityProbe),
		)
		.await
	}

	/// Starts the core with an injected wall clock and Capability probe,
	/// observing the Plane once so its first report needs no waiting.
	///
	/// # Errors
	///
	/// Returns [`CoreError`] with an `unavailable` or `internal` category
	/// when the start cannot be committed.
	pub(crate) async fn start_with(
		store: Store,
		clock: Arc<dyn Clock>,
		probe: Arc<dyn CapabilityProbe>,
	) -> Result<Self, CoreError> {
		store.record_daemon_start().await?;
		let started_at = clock.now();
		// Retention runs only behind a whole chain. A store that moved
		// backwards keeps every record it still has until an owner has
		// seen the evidence and decided what to do (ADR-0105).
		let security = SecurityState::of(store.validate_audit().await?);
		if security == SecurityState::Trusted {
			audit::sweep_retention(&store, unix_ms(started_at)).await?;
		}
		let capabilities = CapabilitySnapshot::from_observation(
			probe.observe().await,
			started_at,
		);
		Ok(Self {
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
			conversation_pages: pagination::ConversationPages::default(),
		})
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
		let observed = self.probe.observe().await;
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
