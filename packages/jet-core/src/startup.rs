//! Initializes and observes the Core bound to one Plane store.

use super::{
	ArtifactLimits, CapabilitySnapshot, Core, CoreError, GitHubHost,
	SecurityState, WorkspaceHome, audit,
	capability::{CapabilityProbe, probe::SystemCapabilityProbe},
	checkpoint,
	clock::{Clock, SystemClock},
	conversation::{
		self,
		discovery::{ConversationDiscovery, SystemConversationDiscovery},
	},
	craft::{self, repository::SystemCraftRepository},
	git_delivery, remote, run, unix_ms,
};
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
			// What the sweep and the collection are about to remove is
			// copied first, at most once a day (ADR-0097).
			store
				.snapshot_if_due(
					jet_store::SnapshotReason::Maintenance,
					unix_ms(started_at),
				)
				.await?;
			audit::sweep_retention(&store, unix_ms(started_at)).await?;
			craft::artifact_collection::collect_unreferenced(
				&store,
				craft_home.clone(),
				started_at,
			)
			.await?;
		}
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
	use crate::{CORE_VERSION, PlaneStatus, Query, QueryResult, SecurityState};

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
				},
				&PlaneStatus {
					cursor: crate::EventSequence(0),
					plane_id: after.plane_id,
					daemon_starts: 2,
					started_at: after.started_at,
					core_version: CORE_VERSION,
					security: SecurityState::Trusted,
				}
			)
		);
		assert!(after.started_at >= before.started_at);
	}
}
