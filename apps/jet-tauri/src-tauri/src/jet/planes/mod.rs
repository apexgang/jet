//! The Plane registry: opaque native Plane handles, what each Plane last
//! reported, and the bindings that tie prepared native authority to the Plane
//! it was prepared against. Slice 1 registers only the local Plane; remote
//! entries arrive with the SSH transport.
pub(crate) mod knowledge;

use std::{
    fmt,
    sync::{Arc, Mutex, RwLock},
};

use jet_protocol::{
    CapabilityObservation, CapabilitySnapshot, DegradedCondition, ExternalTool, PlaneStatus,
    RecoveryState, SecurityState, ToolAvailability, SETTINGS_AND_CAPABILITIES_MINOR,
};
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use self::knowledge::{FeatureView, ProtocolKnowledge, ProtocolView};
use super::{client::PlaneClient, errors::PublicError, JetBridge};

/// Linux counterpart of the design language's "This Mac".
pub(crate) const LOCAL_LABEL: &str = "This computer";
const LOCAL_ID: &str = "local";
const MAXIMUM_REMOTE_PLANES: u32 = 16;

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

/// What the Plane's latest status said about its own trustworthiness.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PlaneHealth {
    security: Security,
    store: Store,
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HealthView {
    security: &'static str,
    store: &'static str,
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
/// bounded facts live here; nothing is authority on its own.
#[derive(Default)]
struct Observed {
    identity: Option<Uuid>,
    core_version: Option<String>,
    knowledge: ProtocolKnowledge,
    health: PlaneHealth,
    connection: ConnectionView,
}

struct PlaneEntry {
    id: PlaneId,
    label: String,
    client: PlaneClient,
    observed: Mutex<Observed>,
}

/// Native registry of reachable Planes. The local Plane is always first.
pub(crate) struct PlaneRegistry {
    local: PlaneClient,
    entries: RwLock<Vec<Arc<PlaneEntry>>>,
}

impl PlaneRegistry {
    pub(crate) fn new(local: PlaneClient) -> Self {
        let entry = Arc::new(PlaneEntry {
            id: PlaneId::Local,
            label: LOCAL_LABEL.into(),
            client: local.clone(),
            observed: Mutex::new(Observed::default()),
        });
        Self {
            local,
            entries: RwLock::new(vec![entry]),
        }
    }

    /// The local Plane. Project setup and new tasks stay local in Wave 3.1.
    pub(crate) fn local(&self) -> &PlaneClient {
        &self.local
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
        let moved = || {
            PublicError::conflict(
                "plane.review_moved",
                "This Plane changed since the request was prepared. Review it again.",
            )
            .with_plane(binding.plane.to_string())
        };
        let entry = self.entry(binding.plane)?.ok_or_else(moved)?;
        let current = entry
            .observed
            .lock()
            .map_err(|_| PublicError::internal())?
            .identity;
        if let (Some(prepared), Some(current)) = (binding.identity, current) {
            if prepared != current {
                return Err(moved());
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
    /// status read. Every status read in the shell goes through here.
    pub(crate) fn observe_status(&self, plane: PlaneId, status: &PlaneStatus) {
        self.update(plane, |observed| {
            observed.identity = Some(status.plane_id);
            observed.core_version = Some(bounded_text(&status.core_version, 48, "Unknown"));
            observed.knowledge.observe_status(status);
            observed.health = PlaneHealth::from_status(status);
        });
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
            credential: None,
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

    #[cfg(test)]
    pub(crate) fn insert_for_test(&self, id: Uuid, label: &str, client: PlaneClient) {
        self.entries.write().unwrap().push(Arc::new(PlaneEntry {
            id: PlaneId::Remote(id),
            label: label.into(),
            client,
            observed: Mutex::new(Observed::default()),
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

fn unknown_plane() -> PublicError {
    PublicError::invalid_input(
        "plane.unknown",
        "That Plane is not registered on this computer.",
    )
}

/// Registry snapshot plus identity state. It never connects to a Plane and
/// never touches the secret store.
#[tauri::command]
pub(crate) fn list_planes(bridge: State<'_, JetBridge>) -> Result<PlanesView, PublicError> {
    let restored_selection = bridge
        .conversations
        .restored_selection()?
        .filter(|(_, plane)| bridge.planes.contains(*plane))
        .map(|(conversation_id, plane)| SelectionView {
            plane_id: plane.to_string(),
            conversation_id: conversation_id.to_string(),
        });
    Ok(PlanesView {
        planes: bridge.planes.views()?,
        identity: IdentityView {
            client_id: bridge.planes.local().client_id().to_string(),
            key: "unknown",
            fingerprint: None,
        },
        restored_selection,
        notice: None,
        maximum_remote_planes: MAXIMUM_REMOTE_PLANES,
    })
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
