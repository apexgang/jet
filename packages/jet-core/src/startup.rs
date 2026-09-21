//! Initializes and observes the Core bound to one Plane store.

use super::{
	ArtifactLimits, CapabilitySnapshot, Core, CoreError, GitHubHost,
	SecurityState, WorkspaceHome, audit,
	capability::{
		CapabilityProbe, CredentialStoreVerification,
		probe::SystemCapabilityProbe,
	},
	checkpoint,
	clock::{Clock, SystemClock},
	conversation::{
		self,
		discovery::{ConversationDiscovery, SystemConversationDiscovery},
	},
	craft::{self, repository::SystemCraftRepository},
	git_delivery, remote, run, unix_ms,
};
use crate::store_recovery;
use jet_store::Store;
use std::sync::Arc;

impl Core {
	/// Install the trusted GitHub transport before accepting delivery work.
	pub fn with_github_host(mut self, host: Arc<dyn GitHubHost>) -> Self {
		self.github_host = host;
		self
	}
	/// Installs the trusted host Adapter before accepting managed Runs.
	pub fn with_run_host(mut self, host: Arc<dyn run::host::RunHost>) -> Self {
		self.run_host = Some(host);
		self
	}

	#[cfg(test)]
	pub(crate) fn with_craft_repository(
		mut self,
		repository: Arc<dyn crate::craft::repository::CraftRepository>,
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
		let recovery = store_recovery::RecoveryMode::of(&store.integrity());
		let serving = recovery == store_recovery::RecoveryMode::Serving;
		if serving {
			store.record_daemon_start().await?;
		}
		let started_at = clock.now();
		// Retention runs only behind a whole chain. A store that moved
		// backwards keeps every record it still has until an owner has
		// seen the evidence and decided what to do (ADR-0105). A damaged
		// store is asked too, and may not be able to answer.
		let security = if serving {
			SecurityState::of(store.validate_audit().await?)
		} else {
			store
				.validate_audit()
				.await
				.map_or(SecurityState::Unverified, SecurityState::of)
		};
		let craft_home = workspace_home
			.0
			.parent()
			.expect("Workspace home has a parent")
			.join("crafts");
		let mut observed = probe.observe().await;
		observed
			.crafts
			.extend(craft::publication::installed_crafts(craft_home).await);
		let capabilities =
			CapabilitySnapshot::from_observation(observed, started_at);
		let core = Self {
			artifact_limits: ArtifactLimits::default(),
			artifact_publication: tokio::sync::Mutex::default(),
			remote_worker: None,
			remote_tool_slots: tokio::sync::Semaphore::new(32),
			extension_host: None,
			utility_host: None,
			github_host: Arc::new(git_delivery::github_host::SystemGitHub),
			review_host: None,
			utility_work: tokio::sync::Mutex::new(()),
			run_work: tokio::sync::Notify::new(),
			maintenance_work: tokio::sync::Notify::new(),
			utility_wake: tokio::sync::Notify::new(),
			turn_wake: tokio::sync::watch::channel(()).0,
			run_host: None,
			terminal_host: None,
			run_recovery: run::recovery::Recovery::default(),
			remote_access: tokio::sync::Semaphore::new(
				remote::AUTHORITY_READERS as usize,
			),
			remote_sessions: remote::RemoteSessions::default(),
			store,
			clock,
			probe,
			capabilities: tokio::sync::RwLock::new(capabilities),
			security: tokio::sync::RwLock::new(security),
			recovery: tokio::sync::watch::channel(recovery).0,
			commands_in_flight: std::sync::atomic::AtomicUsize::new(0),
			started_at,
			effect_reconciliation: tokio::sync::Mutex::new(()),
			craft_artifact_publication: tokio::sync::Mutex::new(()),
			conversation_pages:
				conversation::pagination::ConversationPages::default(),
			checkpoint_pages: checkpoint::pages::Pages::default(),
			workspace_home,
			discovery,
			craft_repository: Arc::new(SystemCraftRepository),
		};
		// The index follows the journal; a daemon that stopped between a
		// Command and its indexing catches up here (ADR-0036). A damaged
		// store is not written to; the restoration catches up instead.
		if serving {
			core.index_search().await?;
		}
		Ok(core)
	}

	/// Settles the maintenance a start owes, once the Plane is serving:
	/// the day's Recovery snapshot when the store predates this start and
	/// the day has none, then the Security audit's retention sweep and the
	/// collection of unreferenced Craft Artifacts, which the snapshot
	/// precedes because they remove things (ADR-0097). Nothing runs while
	/// the store is in read-only Recovery mode or the audit is not trusted
	/// (ADR-0077, ADR-0105).
	///
	/// The copy costs what the store weighs, so `jetd` calls this after its
	/// ready line rather than before it (ADR-0022).
	///
	/// # Errors
	///
	/// Returns a store category [`CoreError`] when the snapshot cannot be
	/// taken or verified, or a sweep cannot be committed. The maintenance
	/// is owed again on the next start.
	pub async fn perform_start_maintenance(&self) -> Result<(), CoreError> {
		if self.recovery_mode() != store_recovery::RecoveryMode::Serving
			|| self.security().await != SecurityState::Trusted
		{
			return Ok(());
		}
		let now = self.clock.now();
		self.store
			.snapshot_if_due(
				jet_store::SnapshotReason::Maintenance,
				unix_ms(now),
			)
			.await?;
		audit::sweep_retention(&self.store, unix_ms(now)).await?;
		craft::artifact_collection::collect_unreferenced(
			&self.store,
			self.run_home().join("crafts"),
			now,
		)
		.await
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
			craft::publication::installed_crafts(
				self.run_home().join("crafts"),
			)
			.await,
		);
		let snapshot =
			CapabilitySnapshot::from_observation(observed, self.clock.now());
		*self.capabilities.write().await = snapshot.clone();
		snapshot
	}

	/// Proves the platform credential store round-trips a Credential
	/// (ADR-0076). The probe writes into the store, so this runs only when
	/// a caller asks; the latest snapshot is left as it was.
	pub(crate) async fn verify_credential_store(
		&self,
	) -> CredentialStoreVerification {
		self.probe.verify_credential_store().await
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

#[cfg(test)]
mod tests {
	use pretty_assertions::assert_eq;

	use crate::test_support::{actor, start_core};
	use crate::{
		CORE_VERSION, DeletionLedger, PlaneStatus, Query, QueryResult,
		RecoveryMode, RecoveryStatus, SecurityState,
	};

	#[tokio::test]
	async fn status_reports_the_persisted_plane_across_core_restarts() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");

		let first = start_core(&path).await;
		let QueryResult::Status(before) =
			first.query(&actor(), Query::Status).await.unwrap()
		else {
			panic!("expected a status snapshot");
		};
		drop(first);

		let second = start_core(&path).await;
		let QueryResult::Status(after) =
			second.query(&actor(), Query::Status).await.unwrap()
		else {
			panic!("expected a status snapshot");
		};

		// A start takes no snapshot of its own: the restart's maintenance,
		// and the copy that precedes it, wait for the Plane to serve
		// (ADR-0097, ADR-0022).
		assert_eq!(
			(&before, &after),
			(
				&PlaneStatus {
					cursor: crate::EventSequence(0),
					plane_id: after.plane_id,
					daemon_starts: 1,
					started_at: before.started_at,
					core_version: CORE_VERSION,
					security: SecurityState::Trusted,
					recovery: RecoveryStatus {
						mode: RecoveryMode::Serving,
						snapshots: vec![],
						deletions: DeletionLedger::Verified(vec![]),
					},
				},
				&PlaneStatus {
					cursor: crate::EventSequence(0),
					plane_id: after.plane_id,
					daemon_starts: 2,
					started_at: after.started_at,
					core_version: CORE_VERSION,
					security: SecurityState::Trusted,
					recovery: RecoveryStatus {
						mode: RecoveryMode::Serving,
						snapshots: vec![],
						deletions: DeletionLedger::Verified(vec![]),
					},
				}
			)
		);
		assert!(after.started_at >= before.started_at);
	}
}
