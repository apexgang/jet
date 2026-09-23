//! The Plane registry: opaque native Plane handles, what each Plane last
//! reported, and the bindings that tie prepared native authority to the Plane
//! it was prepared against. Remote Planes are reached only over SSH
//! (`remote`, `spawner`) and persisted in `planes.json` (`registry_file`)
//! once their identity is proven.
pub(crate) mod knowledge;
pub(crate) mod registry_file;
pub(crate) mod remote;
pub(crate) mod spawner;

use std::{
    fmt,
    path::Path,
    sync::{Arc, Mutex, RwLock},
};

use jet_protocol::{
    CapabilityObservation, CapabilitySnapshot, DegradedCondition, DeletionLedgerStatus,
    ExternalTool, PlaneStatus, RecoveryState, SecurityState, ToolAvailability, REMOTE_AUTH_MINOR,
    SETTINGS_AND_CAPABILITIES_MINOR,
};
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use jet_client::SshEndpoint;

use self::{
    knowledge::{FeatureView, ProtocolKnowledge, ProtocolView},
    registry_file::{
        validate_destination, DestinationKey, RegistryFile, Stored, StoredPlane,
        MAXIMUM_REMOTE_PLANES,
    },
    remote::{ConnectorParts, RemoteConnector, RemoteSession},
    spawner::SshSpawner,
};
use super::{
    client::PlaneClient,
    errors::PublicError,
    keystore::{session_ended, Credential, IdentityKeys},
    JetBridge,
};

/// Linux counterpart of the design language's "This Mac".
pub(crate) const LOCAL_LABEL: &str = "This computer";
const LOCAL_ID: &str = "local";

/// Opaque native Plane handle. The webview never learns sockets, paths or SSH
/// arguments through it: it is the literal `local` or a native-issued UUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PlaneId {
    Local,
    Remote(Uuid),
}

impl PlaneId {
    /// Parses a webview-supplied handle. Only the canonical forms the shell
    /// itself issues are accepted; resolution against the registry follows.
    pub(crate) fn parse(value: &str) -> Result<Self, PublicError> {
        if value == LOCAL_ID {
            return Ok(Self::Local);
        }
        // ASVS 2.2.1: bounded, canonical identifier only.
        if value.len() == 36 {
            if let Ok(id) = Uuid::parse_str(value) {
                if id.to_string() == value {
                    return Ok(Self::Remote(id));
                }
            }
        }
        Err(unknown_plane())
    }

    pub(crate) fn kind(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote(_) => "remote",
        }
    }
}

impl fmt::Display for PlaneId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => formatter.write_str(LOCAL_ID),
            Self::Remote(id) => write!(formatter, "{id}"),
        }
    }
}

/// The Plane a native review, grant or binding was prepared against, and the
/// Plane identity known at that moment. Follow-up commands execute only
/// through `PlaneRegistry::bound`, which refuses a Plane that was forgotten
/// or whose identity changed since.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PlaneBinding {
    pub(crate) plane: PlaneId,
    pub(crate) identity: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Security {
    Trusted,
    Degraded,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Store {
    Serving,
    ReadOnly,
    #[default]
    Unknown,
}

/// The Deletion ledger a restore or purge depends on (ADR-0102).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Ledger {
    Verified,
    Corrupt,
    /// The status reported Recovery but its minor does not name the ledger.
    Unsupported,
    /// No status yet, or no Recovery section in it.
    #[default]
    Unknown,
}

/// What the Plane's latest status said about its own trustworthiness.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PlaneHealth {
    security: Security,
    store: Store,
    ledger: Ledger,
}

impl PlaneHealth {
    pub(crate) fn from_status(status: &PlaneStatus) -> Self {
        Self {
            security: match status.security {
                Some(SecurityState::Trusted) => Security::Trusted,
                Some(SecurityState::Degraded { .. }) => Security::Degraded,
                None => Security::Unknown,
            },
            store: match status.recovery.as_ref().map(|recovery| recovery.state) {
                Some(RecoveryState::Serving) => Store::Serving,
                Some(RecoveryState::ReadOnly) => Store::ReadOnly,
                None => Store::Unknown,
            },
            ledger: match status.recovery.as_ref() {
                None => Ledger::Unknown,
                Some(recovery) => match recovery.deletion_ledger {
                    Some(DeletionLedgerStatus::Verified { .. }) => Ledger::Verified,
                    Some(DeletionLedgerStatus::Corrupt) => Ledger::Corrupt,
                    None => Ledger::Unsupported,
                },
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(security: Security, store: Store) -> Self {
        Self {
            security,
            store,
            ledger: Ledger::Unknown,
        }
    }

    /// Why trust-changing Commands are paused on this Plane, if they are.
    /// This only disables controls in the view; jetd still decides and its
    /// refusal is surfaced as-is when the health read was stale.
    pub(crate) fn mutations_paused(self) -> Option<&'static str> {
        if self.security == Security::Degraded {
            Some("security_degraded")
        } else if self.store == Store::ReadOnly {
            Some("store_read_only")
        } else {
            None
        }
    }

    pub(crate) fn view(self) -> HealthView {
        HealthView {
            security: match self.security {
                Security::Trusted => "trusted",
                Security::Degraded => "degraded",
                Security::Unknown => "unknown",
            },
            store: match self.store {
                Store::Serving => "serving",
                Store::ReadOnly => "read_only",
                Store::Unknown => "unknown",
            },
            ledger: match self.ledger {
                Ledger::Verified => "verified",
                Ledger::Corrupt => "corrupt",
                Ledger::Unsupported => "unsupported",
                Ledger::Unknown => "unknown",
            },
        }
    }
}

/// The compact health summary a feed snapshot carries (Wave 3.3 §4.7). The
/// main window derives its Plane-health notice from it; no counts or gauges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HealthView {
    pub(crate) security: &'static str,
    pub(crate) store: &'static str,
    pub(crate) ledger: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum ConnectionView {
    #[default]
    Idle,
    Connecting,
    Online,
    Reconnecting {
        error: PublicError,
    },
    Failed {
        error: PublicError,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlaneView {
    plane_id: String,
    kind: &'static str,
    label: String,
    plane_identity: Option<String>,
    connection: ConnectionView,
    core_version: Option<String>,
    credential: Option<&'static str>,
    security: &'static str,
    store: &'static str,
    features: Vec<FeatureView>,
    protocol: ProtocolView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IdentityView {
    client_id: String,
    key: &'static str,
    fingerprint: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectionView {
    plane_id: String,
    conversation_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanesView {
    planes: Vec<PlaneView>,
    identity: IdentityView,
    restored_selection: Option<SelectionView>,
    notice: Option<&'static str>,
    maximum_remote_planes: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IssueView {
    section: &'static str,
    error: PublicError,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlaneDetailView {
    plane: PlaneView,
    platform: Option<String>,
    harnesses: Vec<String>,
    crafts: Vec<(String, String)>,
    degraded: Vec<String>,
    missing_tools: Vec<String>,
    issues: Vec<IssueView>,
}

/// Everything the shell last learned about one Plane. Only Plane-reported,
/// bounded facts live here; nothing is authority on its own. A remote
/// Plane's connector shares this record and writes its connection state.
#[derive(Default)]
pub(crate) struct Observed {
    identity: Option<Uuid>,
    core_version: Option<String>,
    knowledge: ProtocolKnowledge,
    health: PlaneHealth,
    connection: ConnectionView,
}

impl Observed {
    /// Seeds identity, health, core version and protocol knowledge from a
    /// status read.
    pub(crate) fn apply_status(&mut self, status: &PlaneStatus) {
        self.identity = Some(status.plane_id);
        self.core_version = Some(bounded_text(&status.core_version, 48, "Unknown"));
        self.knowledge.observe_status(status);
        self.health = PlaneHealth::from_status(status);
    }

    /// A signed remote login succeeded: it proves the remote-auth minor as
    /// well as what its status reports.
    pub(crate) fn logged_in(&mut self, status: &PlaneStatus) {
        self.apply_status(status);
        self.knowledge.observe_success(REMOTE_AUTH_MINOR);
        self.connection = ConnectionView::Online;
    }
}

/// What the shell keeps about a remote Plane. The SSH endpoint stays
/// native; the webview sees only the destination text as the label.
struct RemoteMeta {
    endpoint: SshEndpoint,
    key: DestinationKey,
    /// Proven by a login or a completed pairing; checked on every login.
    plane_identity: Uuid,
    added_at_unix_ms: i64,
    connector: Arc<RemoteConnector>,
}

struct PlaneEntry {
    id: PlaneId,
    label: String,
    client: PlaneClient,
    observed: Arc<Mutex<Observed>>,
    remote: Option<RemoteMeta>,
}

/// Everything needed to pair a registered remote Plane again in place.
pub(crate) struct RemoteTarget {
    pub(crate) binding: PlaneBinding,
    pub(crate) destination: String,
    pub(crate) endpoint: SshEndpoint,
    pub(crate) client: PlaneClient,
    pub(crate) credential: Credential,
}

/// A remote Plane to register once its identity is proven.
pub(crate) struct NewRemote {
    pub(crate) id: Uuid,
    pub(crate) destination: String,
    pub(crate) endpoint: SshEndpoint,
    pub(crate) plane_identity: Uuid,
    pub(crate) credential: Credential,
    /// A login made while adding, adopted so ssh is not spawned twice.
    pub(crate) session: Option<(RemoteSession, PlaneStatus)>,
}

/// Native registry of reachable Planes. The local Plane is always first,
/// then remote Planes in the order they were added.
pub(crate) struct PlaneRegistry {
    local: PlaneClient,
    entries: RwLock<Vec<Arc<PlaneEntry>>>,
    file: Option<RegistryFile>,
    /// The local Plane identity as last persisted, for the duplicate check
    /// while local jetd is offline.
    local_identity: Mutex<Option<Uuid>>,
    notice: Option<&'static str>,
    spawner: Arc<dyn SshSpawner>,
    keys: Arc<IdentityKeys>,
    /// Serializes registry changes with their file writes.
    changes: Mutex<()>,
}

impl fmt::Debug for PlaneRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let planes: Vec<String> = self
            .all()
            .unwrap_or_default()
            .iter()
            .map(|entry| entry.id.to_string())
            .collect();
        formatter
            .debug_struct("PlaneRegistry")
            .field("planes", &planes)
            .finish_non_exhaustive()
    }
}

impl PlaneRegistry {
    /// Loads the persisted remote Planes. Without `directory` nothing is
    /// persisted (tests).
    pub(crate) fn open(
        local: PlaneClient,
        directory: Option<&Path>,
        spawner: Arc<dyn SshSpawner>,
        keys: Arc<IdentityKeys>,
    ) -> Self {
        let file = directory.map(RegistryFile::new);
        let (stored, notice) = file.as_ref().map(RegistryFile::load).unwrap_or_default();
        let entry = Arc::new(PlaneEntry {
            id: PlaneId::Local,
            label: LOCAL_LABEL.into(),
            client: local.clone(),
            observed: Arc::new(Mutex::new(Observed::default())),
            remote: None,
        });
        let registry = Self {
            local,
            entries: RwLock::new(vec![entry]),
            file,
            local_identity: Mutex::new(stored.local_identity),
            notice,
            spawner,
            keys,
            changes: Mutex::new(()),
        };
        for plane in stored.planes {
            let Ok((destination, endpoint)) = validate_destination(&plane.destination) else {
                continue;
            };
            let entry = registry.remote_entry(
                plane.id,
                destination,
                endpoint,
                plane.plane_identity,
                plane.credential,
                plane.added_at_unix_ms,
                None,
            );
            if plane.credential == Credential::Session {
                // The session key died with the previous process.
                if let Some(meta) = &entry.remote {
                    // No login can be in flight before the registry exists.
                    let _ = meta.connector.fail_now(session_ended());
                }
            }
            if let Ok(mut entries) = registry.entries.write() {
                entries.push(entry);
            }
        }
        if let Some(identity) = stored.local_identity {
            registry.mark_local_duplicates(identity);
        }
        registry
    }

    #[cfg(test)]
    pub(crate) fn new(local: PlaneClient) -> Self {
        Self::open(
            local,
            None,
            Arc::new(spawner::SystemSsh),
            Arc::new(IdentityKeys::new(Arc::new(
                crate::jet::keystore::SessionKeyStore::default(),
            ))),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn remote_entry(
        &self,
        id: Uuid,
        destination: String,
        endpoint: SshEndpoint,
        plane_identity: Uuid,
        credential: Credential,
        added_at_unix_ms: i64,
        session: Option<(RemoteSession, PlaneStatus)>,
    ) -> Arc<PlaneEntry> {
        let plane = PlaneId::Remote(id);
        let mut observed = Observed {
            identity: Some(plane_identity),
            ..Observed::default()
        };
        let session = session.map(|(session, status)| {
            observed.logged_in(&status);
            session
        });
        let observed = Arc::new(Mutex::new(observed));
        let connector = Arc::new(RemoteConnector::new(
            ConnectorParts {
                plane,
                endpoint: endpoint.clone(),
                client_id: self.local.client_id(),
                expected_identity: plane_identity,
                credential,
                spawner: Arc::clone(&self.spawner),
                keys: Arc::clone(&self.keys),
                observed: Arc::clone(&observed),
            },
            session,
        ));
        Arc::new(PlaneEntry {
            id: plane,
            label: destination.clone(),
            client: PlaneClient::remote(Arc::clone(&connector), self.local.client_id()),
            observed,
            remote: Some(RemoteMeta {
                key: DestinationKey::of(&destination),
                endpoint,
                plane_identity,
                added_at_unix_ms,
                connector,
            }),
        })
    }

    /// The local Plane. Project setup and new tasks stay local in Wave 3.1.
    pub(crate) fn local(&self) -> &PlaneClient {
        &self.local
    }

    pub(crate) fn keys(&self) -> &Arc<IdentityKeys> {
        &self.keys
    }

    pub(crate) fn spawner(&self) -> &Arc<dyn SshSpawner> {
        &self.spawner
    }

    fn entry(&self, plane: PlaneId) -> Result<Option<Arc<PlaneEntry>>, PublicError> {
        Ok(self
            .entries
            .read()
            .map_err(|_| PublicError::internal())?
            .iter()
            .find(|entry| entry.id == plane)
            .cloned())
    }

    fn all(&self) -> Result<Vec<Arc<PlaneEntry>>, PublicError> {
        Ok(self
            .entries
            .read()
            .map_err(|_| PublicError::internal())?
            .clone())
    }

    /// Resolves an optional webview handle (absent means `local`) to a
    /// registered Plane and the binding a prepared request must keep.
    pub(crate) fn resolve(
        &self,
        plane_id: Option<&str>,
    ) -> Result<(PlaneBinding, PlaneClient), PublicError> {
        let plane = match plane_id {
            None => PlaneId::Local,
            Some(value) => PlaneId::parse(value)?,
        };
        let entry = self.entry(plane)?.ok_or_else(unknown_plane)?;
        let identity = entry
            .observed
            .lock()
            .map_err(|_| PublicError::internal())?
            .identity;
        Ok((PlaneBinding { plane, identity }, entry.client.clone()))
    }

    /// The client of the Plane a request was prepared against, or
    /// `plane.review_moved` when that Plane is gone or is now a different
    /// Plane. No bytes are sent to any Plane on refusal.
    pub(crate) fn bound(&self, binding: &PlaneBinding) -> Result<PlaneClient, PublicError> {
        let entry = self
            .entry(binding.plane)?
            .ok_or_else(|| review_moved(binding.plane))?;
        let current = entry
            .observed
            .lock()
            .map_err(|_| PublicError::internal())?
            .identity;
        if let (Some(prepared), Some(current)) = (binding.identity, current) {
            if prepared != current {
                return Err(review_moved(binding.plane));
            }
        }
        Ok(entry.client.clone())
    }

    fn update(&self, plane: PlaneId, change: impl FnOnce(&mut Observed)) {
        if let Ok(Some(entry)) = self.entry(plane) {
            if let Ok(mut observed) = entry.observed.lock() {
                change(&mut observed);
            }
        }
    }

    /// Seeds identity, health, core version and protocol knowledge from a
    /// status read. Every status read in the shell goes through here. When
    /// the local identity is learned, remote entries that are really this
    /// computer's own Plane are failed.
    pub(crate) fn observe_status(&self, plane: PlaneId, status: &PlaneStatus) {
        self.update(plane, |observed| observed.apply_status(status));
        if plane == PlaneId::Local {
            self.learn_local_identity(status.plane_id);
        }
    }

    fn learn_local_identity(&self, identity: Uuid) {
        let changed = match self.local_identity.lock() {
            Ok(mut known) if *known != Some(identity) => {
                *known = Some(identity);
                true
            }
            _ => false,
        };
        if changed {
            if let Ok(_guard) = self.changes.lock() {
                let _ = self.persist();
            }
            self.mark_local_duplicates(identity);
        }
    }

    fn mark_local_duplicates(&self, identity: Uuid) {
        for entry in self.all().unwrap_or_default() {
            if let Some(meta) = &entry.remote {
                if meta.plane_identity == identity && !meta.connector.fail_now(duplicates_local()) {
                    // A login is in flight; record the failure once it ends.
                    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                        let connector = Arc::clone(&meta.connector);
                        runtime.spawn(async move { connector.fail(duplicates_local()).await });
                    }
                }
            }
        }
    }

    /// A gated call succeeded, so the Plane speaks at least `required`.
    pub(crate) fn observe_success(&self, plane: PlaneId, required: u32) {
        self.update(plane, |observed| {
            observed.knowledge.observe_success(required);
        });
    }

    pub(crate) fn set_connection(&self, plane: PlaneId, connection: ConnectionView) {
        self.update(plane, |observed| observed.connection = connection);
    }

    /// Records a failure the shell concluded itself (for example this
    /// computer lost access after changing its own Pairing). On a remote
    /// Plane it is sticky until Retry, Pair again or Forget.
    pub(crate) async fn fail(&self, plane: PlaneId, error: PublicError) {
        if let Some(entry) = self.entry(plane).ok().flatten() {
            match &entry.remote {
                Some(meta) => meta.connector.fail(error).await,
                None => self.set_connection(
                    plane,
                    ConnectionView::Failed {
                        error: error.with_plane(plane.to_string()),
                    },
                ),
            }
        }
    }

    /// Records what a failure proves about the Plane and names the Plane on
    /// the error, so the webview binds recovery to it.
    pub(crate) fn settle(&self, plane: PlaneId, error: PublicError) -> PublicError {
        if let Some(limit) = error.protocol_limit.as_deref() {
            self.update(plane, |observed| {
                observed
                    .knowledge
                    .observe_negotiated(limit.negotiated_minor);
            });
        }
        error.with_plane(plane.to_string())
    }

    pub(crate) fn identity(&self, plane: PlaneId) -> Option<Uuid> {
        self.entry(plane)
            .ok()
            .flatten()
            .and_then(|entry| entry.observed.lock().ok().and_then(|value| value.identity))
    }

    pub(crate) fn health(&self, plane: PlaneId) -> PlaneHealth {
        self.entry(plane)
            .ok()
            .flatten()
            .and_then(|entry| entry.observed.lock().ok().map(|value| value.health))
            .unwrap_or_default()
    }

    /// The bounded label a Plane is presented with.
    pub(crate) fn label(&self, plane: PlaneId) -> Option<String> {
        self.entry(plane)
            .ok()
            .flatten()
            .map(|entry| entry.label.clone())
    }

    /// Every registered Plane with its bounded label, in registry order.
    /// Never connects.
    pub(crate) fn labels(&self) -> Vec<(PlaneId, String)> {
        self.all()
            .unwrap_or_default()
            .iter()
            .map(|entry| (entry.id, entry.label.clone()))
            .collect()
    }

    pub(crate) fn contains(&self, plane: PlaneId) -> bool {
        matches!(self.entry(plane), Ok(Some(_)))
    }

    /// The Plane label to name in notification copy, only when more than one
    /// Plane is registered. With only this computer the copy is unchanged.
    pub(crate) fn notification_label(&self, plane: PlaneId) -> Option<String> {
        let entries = self.entries.read().ok()?;
        if entries.len() < 2 {
            return None;
        }
        entries
            .iter()
            .find(|entry| entry.id == plane)
            .map(|entry| entry.label.clone())
    }

    /// Planes whose notification fence should be read now: the local Plane
    /// always, remote Planes only while online. The flag says whether the
    /// Plane counted as online for the enable rule.
    pub(crate) fn fence_targets(&self) -> Vec<(PlaneId, PlaneClient, bool)> {
        self.all()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entry| {
                let online = entry
                    .observed
                    .lock()
                    .map(|observed| observed.connection == ConnectionView::Online)
                    .unwrap_or(false);
                (entry.id == PlaneId::Local || online)
                    .then(|| (entry.id, entry.client.clone(), online))
            })
            .collect()
    }

    // -- Remote entries ----------------------------------------------------

    pub(crate) fn remote_count(&self) -> usize {
        self.all()
            .unwrap_or_default()
            .iter()
            .filter(|entry| entry.remote.is_some())
            .count()
    }

    /// Refuses an SSH address another entry already uses. `skip` is the
    /// entry being paired again, and only that entry.
    pub(crate) fn check_destination(
        &self,
        destination: &str,
        skip: Option<Uuid>,
    ) -> Result<(), PublicError> {
        let key = DestinationKey::of(destination);
        let taken = self.all()?.iter().find_map(|entry| {
            (entry.id != skip.map_or(PlaneId::Local, PlaneId::Remote)
                && entry.remote.as_ref().is_some_and(|meta| meta.key == key))
            .then_some(entry.id)
        });
        match taken {
            Some(existing) => Err(already_registered(existing)),
            None => Ok(()),
        }
    }

    /// Refuses a Plane identity that another entry, or the local Plane,
    /// already has. `ssh localhost` and host aliases are caught here.
    pub(crate) fn check_identity(
        &self,
        identity: Uuid,
        skip: Option<Uuid>,
    ) -> Result<(), PublicError> {
        if self.known_local_identity() == Some(identity) {
            return Err(already_registered(PlaneId::Local));
        }
        let taken = self.all()?.iter().find_map(|entry| {
            (entry.id != skip.map_or(PlaneId::Local, PlaneId::Remote)
                && entry
                    .remote
                    .as_ref()
                    .is_some_and(|meta| meta.plane_identity == identity))
            .then_some(entry.id)
        });
        match taken {
            Some(existing) => Err(already_registered(existing)),
            None => Ok(()),
        }
    }

    /// The local Plane's identity: observed now, or remembered from before.
    fn known_local_identity(&self) -> Option<Uuid> {
        self.identity(PlaneId::Local)
            .or_else(|| self.local_identity.lock().ok().and_then(|value| *value))
    }

    pub(crate) fn check_capacity(&self) -> Result<(), PublicError> {
        if self.remote_count() >= MAXIMUM_REMOTE_PLANES {
            return Err(PublicError::invalid_input(
                "plane.limit_reached",
                "This computer already has the maximum of 16 remote Planes. Forget one first.",
            ));
        }
        Ok(())
    }

    /// A registered remote Plane, for Pair again. `local` is not removable
    /// and cannot be paired again.
    pub(crate) fn remote_target(&self, plane_id: &str) -> Result<RemoteTarget, PublicError> {
        let plane = PlaneId::parse(plane_id)?;
        if plane == PlaneId::Local {
            return Err(local_not_removable());
        }
        let entry = self.entry(plane)?.ok_or_else(unknown_plane)?;
        let meta = entry.remote.as_ref().ok_or_else(unknown_plane)?;
        if self.known_local_identity() == Some(meta.plane_identity) {
            // This computer's own Plane reached over SSH: only Forget.
            return Err(duplicates_local().with_plane(plane.to_string()));
        }
        Ok(RemoteTarget {
            binding: PlaneBinding {
                plane,
                identity: Some(meta.plane_identity),
            },
            destination: entry.label.clone(),
            endpoint: meta.endpoint.clone(),
            client: entry.client.clone(),
            credential: meta.connector.credential(),
        })
    }

    /// Registers a remote Plane whose identity is proven, then persists the
    /// registry. The duplicate and capacity checks run again here, under the
    /// same lock as the write.
    pub(crate) fn add_remote(
        &self,
        plane: NewRemote,
        added_at_unix_ms: i64,
    ) -> Result<PlaneId, PublicError> {
        let _guard = self.changes.lock().map_err(|_| PublicError::internal())?;
        self.check_capacity()?;
        self.check_destination(&plane.destination, None)?;
        self.check_identity(plane.plane_identity, None)?;
        let id = PlaneId::Remote(plane.id);
        if self.contains(id) {
            return Err(PublicError::internal());
        }
        let entry = self.remote_entry(
            plane.id,
            plane.destination,
            plane.endpoint,
            plane.plane_identity,
            plane.credential,
            added_at_unix_ms,
            plane.session,
        );
        self.entries
            .write()
            .map_err(|_| PublicError::internal())?
            .push(entry);
        if self.persist().is_err() {
            self.entries
                .write()
                .map_err(|_| PublicError::internal())?
                .retain(|entry| entry.id != id);
            return Err(registry_unsaved());
        }
        Ok(id)
    }

    /// Pair again replaced this computer's key on the Plane: same entry,
    /// same `PlaneId`, new credential. The identity must be the stored one.
    pub(crate) async fn repaired(
        &self,
        binding: &PlaneBinding,
        credential: Credential,
    ) -> Result<PlaneClient, PublicError> {
        let entry = self
            .entry(binding.plane)?
            .ok_or_else(|| review_moved(binding.plane))?;
        let meta = entry
            .remote
            .as_ref()
            .ok_or_else(|| review_moved(binding.plane))?;
        if Some(meta.plane_identity) != binding.identity {
            return Err(review_moved(binding.plane));
        }
        let previous = meta.connector.credential();
        meta.connector.set_credential(credential);
        let saved = {
            let _guard = self.changes.lock().map_err(|_| PublicError::internal())?;
            self.persist()
        };
        if saved.is_err() {
            meta.connector.set_credential(previous);
            return Err(registry_unsaved());
        }
        meta.connector.reset().await;
        meta.connector.shutdown().await;
        Ok(entry.client.clone())
    }

    /// Removes a remote Plane from this computer and kills its ssh. Nothing
    /// is sent to the Plane.
    pub(crate) async fn forget(&self, plane: PlaneId) -> Result<(), PublicError> {
        if plane == PlaneId::Local {
            return Err(local_not_removable());
        }
        let removed = {
            let _guard = self.changes.lock().map_err(|_| PublicError::internal())?;
            let removed = {
                let mut entries = self.entries.write().map_err(|_| PublicError::internal())?;
                let index = entries
                    .iter()
                    .position(|entry| entry.id == plane)
                    .ok_or_else(unknown_plane)?;
                entries.remove(index)
            };
            if self.persist().is_err() {
                if let Ok(mut entries) = self.entries.write() {
                    entries.push(Arc::clone(&removed));
                }
                return Err(registry_unsaved());
            }
            removed
        };
        if let Some(meta) = &removed.remote {
            meta.connector.close();
        }
        Ok(())
    }

    /// Writes `planes.json` from the current entries. Callers hold `changes`.
    fn persist(&self) -> std::io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let planes = self
            .all()
            .map_err(|_| std::io::Error::other("registry unavailable"))?
            .iter()
            .filter_map(|entry| {
                let meta = entry.remote.as_ref()?;
                let PlaneId::Remote(id) = entry.id else {
                    return None;
                };
                Some(StoredPlane {
                    id,
                    destination: entry.label.clone(),
                    plane_identity: meta.plane_identity,
                    credential: meta.connector.credential(),
                    added_at_unix_ms: meta.added_at_unix_ms,
                })
            })
            .collect();
        let local_identity = self.local_identity.lock().ok().and_then(|value| *value);
        file.save(&Stored {
            local_identity,
            planes,
        })
    }

    fn view_of(entry: &PlaneEntry) -> Result<PlaneView, PublicError> {
        let observed = entry.observed.lock().map_err(|_| PublicError::internal())?;
        let health = observed.health.view();
        Ok(PlaneView {
            plane_id: entry.id.to_string(),
            kind: entry.id.kind(),
            label: entry.label.clone(),
            plane_identity: observed.identity.map(|id| id.to_string()),
            connection: observed.connection.clone(),
            core_version: observed.core_version.clone(),
            credential: entry
                .remote
                .as_ref()
                .map(|meta| meta.connector.credential().name()),
            security: health.security,
            store: health.store,
            features: observed.knowledge.features(),
            protocol: observed.knowledge.view(),
        })
    }

    pub(crate) fn views(&self) -> Result<Vec<PlaneView>, PublicError> {
        self.all()?
            .iter()
            .map(|entry| Self::view_of(entry))
            .collect()
    }

    pub(crate) fn view(&self, plane: PlaneId) -> Result<PlaneView, PublicError> {
        let entry = self.entry(plane)?.ok_or_else(unknown_plane)?;
        Self::view_of(&entry)
    }

    /// Registry snapshot plus identity state. Never connects, never touches
    /// the secret store.
    pub(crate) fn snapshot(
        &self,
        restored_selection: Option<(Uuid, PlaneId)>,
    ) -> Result<PlanesView, PublicError> {
        let key = self.keys.view();
        Ok(PlanesView {
            planes: self.views()?,
            identity: IdentityView {
                client_id: self.local.client_id().to_string(),
                key: key.key,
                fingerprint: key.fingerprint,
            },
            restored_selection: restored_selection
                .filter(|(_, plane)| self.contains(*plane))
                .map(|(conversation_id, plane)| SelectionView {
                    plane_id: plane.to_string(),
                    conversation_id: conversation_id.to_string(),
                }),
            notice: self.notice,
            maximum_remote_planes: MAXIMUM_REMOTE_PLANES as u32,
        })
    }

    #[cfg(test)]
    pub(crate) fn insert_for_test(&self, id: Uuid, label: &str, client: PlaneClient) {
        self.entries.write().unwrap().push(Arc::new(PlaneEntry {
            id: PlaneId::Remote(id),
            label: label.into(),
            client,
            observed: Arc::new(Mutex::new(Observed::default())),
            remote: None,
        }));
    }

    #[cfg(test)]
    pub(crate) fn remove_for_test(&self, id: Uuid) {
        self.entries
            .write()
            .unwrap()
            .retain(|entry| entry.id != PlaneId::Remote(id));
    }
}

fn review_moved(plane: PlaneId) -> PublicError {
    PublicError::conflict(
        "plane.review_moved",
        "This Plane changed since the request was prepared. Review it again.",
    )
    .with_plane(plane.to_string())
}

/// Names the entry the new Plane duplicates (`local` for this computer), so
/// the copy can say which one it is.
pub(crate) fn already_registered(existing: PlaneId) -> PublicError {
    PublicError::conflict(
        "plane.already_registered",
        "This is the same Plane as one already on this computer.",
    )
    .with_plane(existing.to_string())
}

pub(crate) const DUPLICATES_LOCAL: &str = "plane.duplicates_local";

fn duplicates_local() -> PublicError {
    PublicError::conflict(
        DUPLICATES_LOCAL,
        "This is this computer's own Plane. Forget it.",
    )
}

pub(crate) fn local_not_removable() -> PublicError {
    PublicError::invalid_input(
        "plane.local_not_removable",
        "This computer's own Plane is always listed.",
    )
}

fn registry_unsaved() -> PublicError {
    PublicError::internal()
}

pub(crate) fn unknown_plane() -> PublicError {
    PublicError::invalid_input(
        "plane.unknown",
        "That Plane is not registered on this computer.",
    )
}

/// Registry snapshot plus identity state. It never connects to a Plane and
/// never touches the secret store.
#[tauri::command]
pub(crate) fn list_planes(bridge: State<'_, JetBridge>) -> Result<PlanesView, PublicError> {
    bridge.planes.snapshot(bridge.restorable_selection()?)
}

/// Status, capabilities and protocol knowledge for one Plane. Each part fails
/// independently into `issues`, so a failed Plane still shows what is known.
#[tauri::command]
pub(crate) async fn load_plane_detail(
    bridge: State<'_, JetBridge>,
    plane_id: String,
) -> Result<PlaneDetailView, PublicError> {
    let (binding, client) = bridge.planes.resolve(Some(&plane_id))?;
    let plane = binding.plane;
    if let Some(connector) = client.connector() {
        // Key presence only; nothing is unlocked here.
        bridge
            .planes
            .keys()
            .has_seed(client.client_id(), connector.credential())
            .await;
    }
    let mut issues = Vec::new();
    let mut capabilities = None;
    match client.connect().await {
        Ok(connection) => {
            match connection.status().await {
                Ok(status) => bridge.planes.observe_status(plane, &status),
                Err(error) => issues.push(IssueView {
                    section: "status",
                    error: bridge
                        .planes
                        .settle(plane, PublicError::from_client(&error)),
                }),
            }
            match connection
                .capabilities(CapabilityObservation::LastObserved)
                .await
            {
                Ok(snapshot) => {
                    bridge
                        .planes
                        .observe_success(plane, SETTINGS_AND_CAPABILITIES_MINOR);
                    capabilities = Some(snapshot);
                }
                Err(error) => issues.push(IssueView {
                    section: "capabilities",
                    error: bridge
                        .planes
                        .settle(plane, PublicError::from_client(&error)),
                }),
            }
        }
        Err(error) => issues.push(IssueView {
            section: "connection",
            error: bridge
                .planes
                .settle(plane, PublicError::from_client(&error)),
        }),
    }
    Ok(detail_view(
        bridge.planes.view(plane)?,
        capabilities,
        issues,
    ))
}

fn detail_view(
    plane: PlaneView,
    capabilities: Option<CapabilitySnapshot>,
    issues: Vec<IssueView>,
) -> PlaneDetailView {
    let Some(capabilities) = capabilities else {
        return PlaneDetailView {
            plane,
            platform: None,
            harnesses: Vec::new(),
            crafts: Vec::new(),
            degraded: Vec::new(),
            missing_tools: Vec::new(),
            issues,
        };
    };
    PlaneDetailView {
        plane,
        platform: Some(format!(
            "{} · {}",
            bounded_text(
                &capabilities.platform.operating_system,
                32,
                "Unknown system"
            ),
            bounded_text(&capabilities.platform.architecture, 32, "Unknown CPU")
        )),
        harnesses: capabilities
            .harnesses
            .iter()
            .take(64)
            .map(|harness| bounded_text(harness, 64, "Unknown Harness"))
            .collect(),
        crafts: capabilities
            .crafts
            .iter()
            .take(64)
            .map(|craft| {
                (
                    bounded_text(&craft.craft_id, 128, "Unknown Craft"),
                    bounded_text(&craft.version, 64, "Unknown version"),
                )
            })
            .collect(),
        degraded: capabilities
            .degraded
            .iter()
            .take(32)
            .map(degraded_label)
            .map(str::to_owned)
            .collect(),
        missing_tools: capabilities
            .external_tools
            .iter()
            .filter(|tool| tool.availability == ToolAvailability::Missing)
            .map(|tool| tool_name(tool.tool).to_owned())
            .collect(),
        issues,
    }
}

fn tool_name(tool: ExternalTool) -> &'static str {
    match tool {
        ExternalTool::Git => "Git",
        ExternalTool::GitLfs => "Git LFS",
        ExternalTool::Ssh => "SSH",
        ExternalTool::Tailscale => "Tailscale",
    }
}

fn degraded_label(condition: &DegradedCondition) -> &'static str {
    match condition {
        DegradedCondition::MissingExternalTool { tool } => match tool {
            ExternalTool::Git => "Git is not available",
            ExternalTool::GitLfs => "Git LFS is not available",
            ExternalTool::Ssh => "SSH is not available",
            ExternalTool::Tailscale => "Tailscale is not available",
        },
        DegradedCondition::NoHarnessAvailable => "No Harness is installed",
        DegradedCondition::CredentialStoreUnavailable { .. } => "Secure storage is unavailable",
        DegradedCondition::CredentialStoreLocked { .. } => "Secure storage is locked",
    }
}

fn bounded_text(value: &str, maximum_bytes: usize, fallback: &str) -> String {
    if !value.is_empty()
        && value.len() <= maximum_bytes
        && value
            .chars()
            .all(|character| !character.is_control() && character != '<' && character != '>')
    {
        value.to_owned()
    } else {
        fallback.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use jet_protocol::{PlaneStatus, RecoveryState, RecoveryStatus, SecurityState};
    use uuid::Uuid;

    use super::{ConnectionView, PlaneBinding, PlaneId, PlaneRegistry};
    use crate::jet::{client::PlaneClient, errors::PublicError};

    fn client(socket: &str) -> PlaneClient {
        PlaneClient::new(
            socket.into(),
            Uuid::from_u128(7),
            [Duration::from_millis(1)],
            Duration::from_millis(1),
        )
    }

    fn status(identity: u128) -> PlaneStatus {
        PlaneStatus {
            cursor: Some(3),
            plane_id: Uuid::from_u128(identity),
            daemon_starts: 1,
            started_at_unix_ms: 1,
            core_version: "0.2.0".into(),
            security: Some(SecurityState::Trusted),
            recovery: Some(RecoveryStatus {
                state: RecoveryState::ReadOnly,
                reason: None,
                snapshots: Vec::new(),
                deletion_ledger: None,
            }),
        }
    }

    #[test]
    fn plane_handles_are_canonical_and_opaque() {
        assert_eq!(PlaneId::parse("local").unwrap(), PlaneId::Local);
        let id = Uuid::from_u128(0xabcd_ef01);
        assert_eq!(
            PlaneId::parse(&id.to_string()).unwrap(),
            PlaneId::Remote(id)
        );
        for invalid in [
            "",
            "LOCAL",
            "/run/jetd.sock",
            "user@host",
            &id.to_string().to_uppercase(),
            &id.simple().to_string(),
            &format!("{{{id}}}"),
        ] {
            assert_eq!(PlaneId::parse(invalid).unwrap_err().code, "plane.unknown");
        }
    }

    #[test]
    fn absent_plane_means_local_and_unregistered_planes_are_unknown() {
        let registry = PlaneRegistry::new(client("/local.sock"));
        let (binding, resolved) = registry.resolve(None).unwrap();
        assert_eq!(binding.plane, PlaneId::Local);
        assert_eq!(resolved.socket_for_test(), "/local.sock");
        let error = registry
            .resolve(Some(&Uuid::from_u128(9).to_string()))
            .err()
            .unwrap();
        assert_eq!(error.code, "plane.unknown");
    }

    #[test]
    fn bindings_execute_only_on_the_plane_they_were_prepared_against() {
        let registry = PlaneRegistry::new(client("/local.sock"));
        let remote = Uuid::from_u128(2);
        registry.insert_for_test(remote, "build-box", client("/remote.sock"));
        registry.observe_status(PlaneId::Remote(remote), &status(20));
        registry.observe_status(PlaneId::Local, &status(10));

        let (binding, resolved) = registry.resolve(Some(&remote.to_string())).unwrap();
        assert_eq!(
            binding,
            PlaneBinding {
                plane: PlaneId::Remote(remote),
                identity: Some(Uuid::from_u128(20)),
            }
        );
        assert_eq!(resolved.socket_for_test(), "/remote.sock");
        assert_eq!(
            registry.bound(&binding).unwrap().socket_for_test(),
            "/remote.sock"
        );
        let (local, _) = registry.resolve(Some("local")).unwrap();
        assert_eq!(
            registry.bound(&local).unwrap().socket_for_test(),
            "/local.sock"
        );

        // The same destination now answers as a different Plane.
        registry.observe_status(PlaneId::Remote(remote), &status(21));
        let moved = registry.bound(&binding).err().unwrap();
        assert_eq!(moved.code, "plane.review_moved");
        assert_eq!(moved.category, "conflict");
        assert!(!moved.retryable);
        assert_eq!(moved.plane_id.as_deref(), Some(&*remote.to_string()));

        // A forgotten Plane can never execute an older review.
        let (fresh, _) = registry.resolve(Some(&remote.to_string())).unwrap();
        assert!(registry.bound(&fresh).is_ok());
        registry.remove_for_test(remote);
        assert_eq!(
            registry.bound(&fresh).err().unwrap().code,
            "plane.review_moved"
        );
    }

    #[test]
    fn a_local_binding_prepared_before_the_first_status_stays_valid() {
        let registry = PlaneRegistry::new(client("/local.sock"));
        let (binding, _) = registry.resolve(None).unwrap();
        assert_eq!(binding.identity, None);
        registry.observe_status(PlaneId::Local, &status(10));
        assert!(registry.bound(&binding).is_ok());
    }

    #[test]
    fn status_seeds_health_and_views_carry_no_native_paths() {
        let registry = PlaneRegistry::new(client("/home/user/.jet/runtime/jetd.sock"));
        registry.observe_status(PlaneId::Local, &status(10));
        registry.set_connection(PlaneId::Local, ConnectionView::Online);
        let health = registry.health(PlaneId::Local).view();
        assert_eq!(health.security, "trusted");
        assert_eq!(health.store, "read_only");
        let view = serde_json::to_string(&registry.views().unwrap()).unwrap();
        assert!(view.contains(r#""planeId":"local""#));
        assert!(view.contains(r#""label":"This computer""#));
        assert!(view.contains(r#""connection":{"state":"online"}"#));
        assert!(view.contains(r#""store":"read_only""#));
        assert!(!view.contains("jetd.sock"));
    }

    #[test]
    fn failures_record_the_exact_minor_and_name_the_plane() {
        let registry = PlaneRegistry::new(client("/local.sock"));
        let error = registry.settle(
            PlaneId::Local,
            PublicError::from_client(&jet_client::ClientError::FeatureUnavailable {
                required_minor: 12,
                negotiated_minor: 9,
            }),
        );
        assert_eq!(error.plane_id.as_deref(), Some("local"));
        let view = registry.view(PlaneId::Local).unwrap();
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["protocol"]["exact"], 9);
        let search = json["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|feature| feature["feature"] == "search")
            .unwrap();
        assert_eq!(search["support"], "unsupported");
    }

    #[test]
    fn notification_copy_names_the_plane_only_with_several_planes() {
        let registry = PlaneRegistry::new(client("/local.sock"));
        assert_eq!(registry.notification_label(PlaneId::Local), None);
        registry.insert_for_test(Uuid::from_u128(2), "build-box", client("/remote.sock"));
        assert_eq!(
            registry.notification_label(PlaneId::Local).as_deref(),
            Some("This computer")
        );
        let targets = registry.fence_targets();
        assert_eq!(targets.len(), 1, "offline remote Planes fence on connect");
        registry.set_connection(PlaneId::Remote(Uuid::from_u128(2)), ConnectionView::Online);
        assert_eq!(registry.fence_targets().len(), 2);
    }
}
