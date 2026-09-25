//! System health and safe repairs (Wave 3.3 §4.4): the Settings window's
//! Versions, Storage and Diagnostics sections read one Plane's health here,
//! and "Free disposable space" runs a single bounded Artifact collection
//! pass on one Plane from either window.
//!
//! The health read is one connection: a status read, which alone is fatal,
//! then capabilities and two Setting reads whose failures are reported per
//! section. Every Plane string is bounded here; nothing native crosses as a
//! path or identifier.
//!
//! Recovery snapshot restore and the Recovery purge live in [`recovery`].
use std::{collections::HashSet, sync::Mutex};

use jet_protocol::{
    AuditBreach, CapabilityObservation, CapabilitySnapshot, CredentialStoreKind,
    CredentialStoreStatus, DegradedCondition, DeletionLedgerStatus, ExternalTool, PlaneStatus,
    RecoveryReason, RecoveryState, RecoveryStatus, SecurityState, SettingKey, SettingScope,
    SettingSelection, SettingValue, ToolAvailability, ARTIFACTS_MINOR, PROTOCOL_MINOR,
    PROTOCOL_VERSION, SETTINGS_AND_CAPABILITIES_MINOR,
};
use serde::Serialize;
use tauri::State;

use super::{
    client::Connection,
    errors::PublicError,
    planes::{knowledge::ProtocolView, PlaneId},
    settings::plane_client,
    settings_window::{SettingsPane, SettingsSection},
    setup::{degraded_label, safe_text},
    JetBridge,
};

pub(crate) mod recovery;

/// Crafts listed in one health read; a Plane reporting more is not trusted.
const MAX_CRAFTS: usize = 64;
/// Harnesses named per Craft.
const MAX_CRAFT_HARNESSES: usize = 16;

#[derive(Default)]
pub(crate) struct SystemState {
    collect_in_flight: Mutex<HashSet<PlaneId>>,
    /// Restore and purge reviews, and the snapshot tokens they name.
    recovery: recovery::RecoveryState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CollectView {
    removed: u32,
}

// ---------------------------------------------------------------------------
// Health (Versions, Storage and Diagnostics sections)
// ---------------------------------------------------------------------------

/// One section of the health read that failed while the rest loaded.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SectionIssue {
    section: &'static str,
    error: PublicError,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SystemHealthView {
    plane_id: String,
    plane_label: String,
    service: ServiceView,
    app: AppView,
    /// What this client can prove about the negotiated minor. The exact
    /// minor is known only when a refusal or status named it.
    protocol: ProtocolView,
    /// `None` when capabilities could not be read (see `issues`).
    platform: Option<String>,
    tools: Vec<ToolView>,
    crafts: Vec<CraftView>,
    credential_store: Option<CredentialStoreView>,
    degraded: Vec<DegradedView>,
    recovery: RecoveryView,
    security: SecurityView,
    /// A new-audit-epoch request of this app was sent and its outcome is
    /// still unknown: "Start new audit period…" reopens it for "Try again",
    /// whatever the Security state shows.
    pending_epoch: bool,
    storage: StorageView,
    retention: RetentionView,
    issues: Vec<SectionIssue>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServiceView {
    core_version: String,
    daemon_starts: String,
    started_at_unix_ms: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppView {
    version: &'static str,
    supported_protocol: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolView {
    tool: &'static str,
    label: &'static str,
    /// The tool's own bounded version line; `None` when it is not installed.
    version: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CraftView {
    id: String,
    version: String,
    harnesses: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CredentialStoreView {
    state: &'static str,
    kind: &'static str,
}

/// A Settings section that can address a degraded condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsLinkView {
    pane: SettingsPane,
    section: SettingsSection,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct DegradedView {
    kind: &'static str,
    label: String,
    target: Option<SettingsLinkView>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum LedgerView {
    Verified { deletions: String },
    Corrupt,
    Unsupported,
}

/// The store's Recovery state. `snapshotCount` is every snapshot the Plane
/// reported; `snapshots` lists at most the newest
/// [`recovery::MAX_LISTED_SNAPSHOTS`], each named by an opaque token.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum RecoveryView {
    /// The Plane's minor does not name store Recovery.
    Unsupported,
    Serving {
        snapshot_count: usize,
        snapshots: Vec<recovery::SnapshotView>,
        ledger: LedgerView,
    },
    ReadOnly {
        reason: &'static str,
        snapshot_count: usize,
        snapshots: Vec<recovery::SnapshotView>,
        ledger: LedgerView,
    },
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum SecurityView {
    Trusted,
    Degraded {
        breach: &'static str,
        /// Where validation first disagreed, for the two breaches that name it.
        breach_sequence: Option<String>,
        epoch: String,
        /// This app saved the evidence of this epoch (Save audit evidence),
        /// so a new audit period may be reviewed.
        exported: bool,
    },
    /// Not reported: an older minor, or read-only Recovery whose audit could
    /// not be validated.
    Absent,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageView {
    #[serde(rename = "disposableMiB")]
    disposable_mib: Option<u32>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RetentionView {
    grace_days: Option<u32>,
}

/// Reads one Plane's health for the Settings window. `fresh` asks the Plane
/// to observe its capabilities again (Check again); otherwise the last
/// observation is used.
#[tauri::command]
pub(crate) async fn load_system_health(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    fresh: bool,
) -> Result<SystemHealthView, PublicError> {
    load_health(&bridge, &plane_id, fresh).await
}

pub(super) async fn load_health(
    bridge: &JetBridge,
    plane_id: &str,
    fresh: bool,
) -> Result<SystemHealthView, PublicError> {
    let (binding, client) = plane_client(bridge, plane_id)?;
    let plane = binding.plane;
    async {
        let connection = client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let status = connection
            .query(connection.status())
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        bridge.planes.observe_status(plane, &status);
        let snapshots = match status.recovery.as_ref() {
            Some(recovery) => {
                bridge
                    .system
                    .recovery
                    .refresh(plane, status.plane_id, &recovery.snapshots)?
            }
            None => Vec::new(),
        };

        let mut issues = Vec::new();
        let observation = if fresh {
            CapabilityObservation::Fresh
        } else {
            CapabilityObservation::LastObserved
        };
        let capabilities = match connection.query(connection.capabilities(observation)).await {
            Ok(snapshot) => {
                bridge
                    .planes
                    .observe_success(plane, SETTINGS_AND_CAPABILITIES_MINOR);
                Some(snapshot)
            }
            Err(error) => {
                issues.push(issue(
                    bridge,
                    plane,
                    "capabilities",
                    PublicError::from_client(&error),
                ));
                None
            }
        };
        let disposable = match count_setting(&connection, SettingKey::StorageDisposableMiB).await {
            Ok(value) => value,
            Err(error) => {
                issues.push(issue(bridge, plane, "storage", error));
                None
            }
        };
        let grace = match count_setting(&connection, SettingKey::RetentionTrashGraceDays).await {
            Ok(value) => value,
            Err(error) => {
                issues.push(issue(bridge, plane, "retention", error));
                None
            }
        };

        let exported = bridge
            .audit
            .mark(plane, status.plane_id)?
            .and_then(|mark| mark.epoch);
        let pending_epoch = bridge.system.recovery.epoch_pending(plane)?;
        Ok(health_view(
            plane,
            bridge.planes.label(plane).unwrap_or_default(),
            bridge.planes.protocol(plane),
            &status,
            snapshots,
            capabilities.as_ref(),
            exported,
            pending_epoch,
            disposable,
            grace,
            issues,
        ))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

fn issue(
    bridge: &JetBridge,
    plane: PlaneId,
    section: &'static str,
    error: PublicError,
) -> SectionIssue {
    SectionIssue {
        section,
        error: bridge.planes.settle(plane, error),
    }
}

/// One Plane-scope count Setting. A Plane whose minor does not name the key
/// answers without it, which is `None`, not an error.
async fn count_setting(
    connection: &Connection,
    key: SettingKey,
) -> Result<Option<u32>, PublicError> {
    let snapshot = connection
        .query(connection.settings(SettingScope::Plane, SettingSelection::Key { key }))
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    Ok(snapshot
        .settings
        .iter()
        .find(|setting| setting.key == key)
        .and_then(|setting| match setting.value {
            SettingValue::Count(value) => Some(value),
            SettingValue::Flag(_) | SettingValue::Text(_) => None,
        }))
}

#[allow(clippy::too_many_arguments)]
fn health_view(
    plane: PlaneId,
    plane_label: String,
    protocol: ProtocolView,
    status: &PlaneStatus,
    snapshots: Vec<recovery::SnapshotView>,
    capabilities: Option<&CapabilitySnapshot>,
    exported_epoch: Option<u64>,
    pending_epoch: bool,
    disposable_mib: Option<u32>,
    grace_days: Option<u32>,
    issues: Vec<SectionIssue>,
) -> SystemHealthView {
    SystemHealthView {
        plane_id: plane.to_string(),
        plane_label,
        service: ServiceView {
            core_version: safe_text(&status.core_version, 48, "Unknown"),
            daemon_starts: status.daemon_starts.to_string(),
            started_at_unix_ms: status.started_at_unix_ms.to_string(),
        },
        app: AppView {
            version: env!("CARGO_PKG_VERSION"),
            supported_protocol: format!("{PROTOCOL_VERSION}.{PROTOCOL_MINOR}"),
        },
        protocol,
        platform: capabilities.map(|capabilities| {
            format!(
                "{} · {}",
                safe_text(
                    &capabilities.platform.operating_system,
                    32,
                    "Unknown system"
                ),
                safe_text(&capabilities.platform.architecture, 32, "Unknown CPU")
            )
        }),
        tools: capabilities
            .map(|capabilities| {
                capabilities
                    .external_tools
                    .iter()
                    .map(|status| tool_view(status.tool, &status.availability))
                    .collect()
            })
            .unwrap_or_default(),
        crafts: capabilities
            .map(|capabilities| {
                capabilities
                    .crafts
                    .iter()
                    .take(MAX_CRAFTS)
                    .map(|craft| CraftView {
                        id: safe_text(&craft.craft_id, 128, "unavailable"),
                        version: safe_text(&craft.version, 64, "Unknown"),
                        harnesses: craft
                            .harnesses
                            .iter()
                            .take(MAX_CRAFT_HARNESSES)
                            .map(|harness| safe_text(harness, 64, "Unknown Harness"))
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        credential_store: capabilities
            .map(|capabilities| credential_store_view(capabilities.credential_store)),
        degraded: capabilities
            .map(|capabilities| capabilities.degraded.iter().map(degraded_view).collect())
            .unwrap_or_default(),
        recovery: recovery_view(status.recovery.as_ref(), snapshots),
        security: security_view(status.security.as_ref(), exported_epoch),
        pending_epoch,
        storage: StorageView { disposable_mib },
        retention: RetentionView { grace_days },
        issues,
    }
}

fn tool_view(tool: ExternalTool, availability: &ToolAvailability) -> ToolView {
    let (name, label) = match tool {
        ExternalTool::Git => ("git", "Git"),
        ExternalTool::GitLfs => ("git_lfs", "Git LFS"),
        ExternalTool::Ssh => ("ssh", "SSH"),
        ExternalTool::Tailscale => ("tailscale", "Tailscale"),
    };
    ToolView {
        tool: name,
        label,
        version: match availability {
            ToolAvailability::Present { version } => {
                Some(safe_text(version.trim(), 96, "Installed"))
            }
            ToolAvailability::Missing => None,
        },
    }
}

fn credential_store_view(status: CredentialStoreStatus) -> CredentialStoreView {
    let (state, kind) = match status {
        CredentialStoreStatus::Available { kind } => ("available", kind),
        CredentialStoreStatus::Locked { kind } => ("locked", kind),
        CredentialStoreStatus::Unavailable { kind } => ("unavailable", kind),
    };
    CredentialStoreView {
        state,
        kind: match kind {
            CredentialStoreKind::AppleKeychain => "apple_keychain",
            CredentialStoreKind::SecretService => "secret_service",
        },
    }
}

/// Each degraded condition with the Settings section that addresses it. A
/// missing external tool is installed outside Jet, so it links nowhere.
fn degraded_view(condition: &DegradedCondition) -> DegradedView {
    let link = |section: SettingsSection| {
        Some(SettingsLinkView {
            pane: section.pane(),
            section,
        })
    };
    let (kind, target) = match condition {
        DegradedCondition::MissingExternalTool { .. } => ("missing_external_tool", None),
        DegradedCondition::NoHarnessAvailable => {
            ("no_harness_available", link(SettingsSection::Harnesses))
        }
        DegradedCondition::CredentialStoreUnavailable { .. } => (
            "credential_store_unavailable",
            link(SettingsSection::Accounts),
        ),
        DegradedCondition::CredentialStoreLocked { .. } => {
            ("credential_store_locked", link(SettingsSection::Accounts))
        }
    };
    DegradedView {
        kind,
        label: degraded_label(condition),
        target,
    }
}

fn recovery_view(
    recovery: Option<&RecoveryStatus>,
    snapshots: Vec<recovery::SnapshotView>,
) -> RecoveryView {
    let Some(recovery) = recovery else {
        return RecoveryView::Unsupported;
    };
    let ledger = match recovery.deletion_ledger {
        Some(DeletionLedgerStatus::Verified { deletions }) => LedgerView::Verified {
            deletions: deletions.to_string(),
        },
        Some(DeletionLedgerStatus::Corrupt) => LedgerView::Corrupt,
        None => LedgerView::Unsupported,
    };
    let snapshot_count = recovery.snapshots.len();
    match recovery.state {
        RecoveryState::Serving => RecoveryView::Serving {
            snapshot_count,
            snapshots,
            ledger,
        },
        RecoveryState::ReadOnly => RecoveryView::ReadOnly {
            reason: match recovery.reason {
                Some(RecoveryReason::IntegrityCheckFailed) => "integrity_check_failed",
                Some(RecoveryReason::MigrationFailed) => "migration_failed",
                None => "unknown",
            },
            snapshot_count,
            snapshots,
            ledger,
        },
    }
}

/// The Security state. `exported_epoch` is the degraded epoch of this
/// app's last completed evidence export of the same Plane identity.
fn security_view(security: Option<&SecurityState>, exported_epoch: Option<u64>) -> SecurityView {
    match security {
        None => SecurityView::Absent,
        Some(SecurityState::Trusted) => SecurityView::Trusted,
        Some(SecurityState::Degraded { breach, epoch, .. }) => {
            let (breach, sequence) = breach_name(breach);
            SecurityView::Degraded {
                breach,
                breach_sequence: sequence.map(|sequence| sequence.to_string()),
                epoch: epoch.to_string(),
                exported: exported_epoch == Some(*epoch),
            }
        }
    }
}

/// A breach's stable name, and the position it names, if any.
pub(crate) fn breach_name(breach: &AuditBreach) -> (&'static str, Option<u64>) {
    match breach {
        AuditBreach::HeadMissing => ("head_missing", None),
        AuditBreach::HeadNotInStore => ("head_not_in_store", None),
        AuditBreach::HeadDiverged => ("head_diverged", None),
        AuditBreach::RecordAltered { sequence } => ("record_altered", Some(*sequence)),
        AuditBreach::TargetAltered { sequence } => ("target_altered", Some(*sequence)),
    }
}

// ---------------------------------------------------------------------------
// Free disposable space
// ---------------------------------------------------------------------------

/// Holds one Plane's slot in an in-flight set and releases it on every exit
/// path, including a dropped IPC future.
pub(crate) struct InFlight<'a> {
    set: &'a Mutex<HashSet<PlaneId>>,
    plane: PlaneId,
}

impl<'a> InFlight<'a> {
    pub(crate) fn enter(
        set: &'a Mutex<HashSet<PlaneId>>,
        plane: PlaneId,
        busy: PublicError,
    ) -> Result<Self, PublicError> {
        let mut planes = set.lock().map_err(|_| PublicError::internal())?;
        if !planes.insert(plane) {
            return Err(busy);
        }
        Ok(Self { set, plane })
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if let Ok(mut planes) = self.set.lock() {
            planes.remove(&self.plane);
        }
    }
}

/// Runs one bounded collection of disposable Artifact storage on a Plane.
/// It is available under disk pressure (`docs/disk-pressure.md`) and
/// carries no Command ID, so a retry is simply another pass.
#[tauri::command]
pub(crate) async fn collect_disposable_storage(
    bridge: State<'_, JetBridge>,
    plane_id: String,
) -> Result<CollectView, PublicError> {
    collect(&bridge, &plane_id).await
}

pub(super) async fn collect(
    bridge: &JetBridge,
    plane_id: &str,
) -> Result<CollectView, PublicError> {
    let (binding, client) = bridge.plane(Some(plane_id))?;
    let _flight = InFlight::enter(
        &bridge.system.collect_in_flight,
        binding.plane,
        PublicError::conflict(
            "storage.collect_busy",
            "Jet is already freeing disposable space on this Plane.",
        ),
    )
    .map_err(|error| error.with_plane(binding.plane.to_string()))?;
    async {
        let connection = client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let removed = connection
            .command(connection.collect_artifacts())
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        bridge
            .planes
            .observe_success(binding.plane, ARTIFACTS_MINOR);
        Ok(CollectView { removed })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use uuid::Uuid;

    use super::collect;
    use crate::jet::{
        keystore::{
            tests::{CountingStore, Fail},
            IdentityKeys,
        },
        planes::{spawner::fake::FakeSpawner, PlaneId},
        JetBridge,
    };

    fn bridge(directory: &std::path::Path) -> JetBridge {
        JetBridge::for_test(
            directory,
            Uuid::from_u128(1),
            Arc::new(FakeSpawner::default()),
            Arc::new(IdentityKeys::new(Arc::new(CountingStore::new(
                Fail::Nothing,
            )))),
        )
    }

    #[tokio::test]
    async fn collection_is_single_flight_per_plane_and_releases_its_slot() {
        let directory = tempfile::tempdir().unwrap();
        let bridge = bridge(directory.path());

        bridge
            .system
            .collect_in_flight
            .lock()
            .unwrap()
            .insert(PlaneId::Local);
        let busy = collect(&bridge, "local").await.unwrap_err();
        assert_eq!(busy.code, "storage.collect_busy");
        assert_eq!(busy.category, "conflict");
        assert_eq!(busy.plane_id.as_deref(), Some("local"));
        bridge.system.collect_in_flight.lock().unwrap().clear();

        // The local socket does not exist: the pass fails offline, names its
        // Plane, and leaves no slot behind.
        let offline = collect(&bridge, "local").await.unwrap_err();
        assert_eq!(offline.category, "offline");
        assert_eq!(offline.plane_id.as_deref(), Some("local"));
        assert!(bridge.system.collect_in_flight.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn collection_refuses_an_unknown_plane_before_any_io() {
        let directory = tempfile::tempdir().unwrap();
        let bridge = bridge(directory.path());
        let unknown = collect(&bridge, "../jetd.sock").await.unwrap_err();
        assert_eq!(unknown.category, "invalid_input");
        assert!(bridge.system.collect_in_flight.lock().unwrap().is_empty());
    }

    // -----------------------------------------------------------------------
    // Health
    // -----------------------------------------------------------------------

    use std::time::Duration;

    use jet_protocol::{
        AuditBreach, CapabilitySnapshot, ClientMessage, CredentialStoreKind, CredentialStoreStatus,
        DegradedCondition, DeletionLedgerStatus, ErrorCategory, ExternalTool, ExternalToolStatus,
        InstalledCraft, PlaneStatus, Platform, QueryRequest, QueryResponse, RecoveryReason,
        RecoverySnapshot, RecoveryState, RecoveryStatus, ResolvedSetting, SecurityState,
        ServerMessage, SettingKey, SettingScope, SettingSelection, SettingSnapshot, SettingSource,
        SettingValue, SnapshotReason, ToolAvailability, WireError,
    };
    use serde_json::json;
    use tokio::net::UnixListener;

    use super::{health_view, load_health, recovery_view, security_view, SectionIssue};
    use crate::jet::{
        client::{
            unit_tests::{accept, next_message, reply, request_id},
            PlaneClient,
        },
        enrollment::tests::setup,
        errors::PublicError,
        planes::knowledge::ProtocolKnowledge,
    };

    const CLIENT: Uuid = Uuid::from_u128(0x7a54);

    fn status(recovery: Option<RecoveryStatus>, security: Option<SecurityState>) -> PlaneStatus {
        PlaneStatus {
            cursor: Some(9),
            plane_id: Uuid::from_u128(0xfeed),
            daemon_starts: 3,
            started_at_unix_ms: 1_700_000_000_000,
            core_version: "1.43.0".into(),
            security,
            recovery,
        }
    }

    fn snapshot_file(name: &str) -> RecoverySnapshot {
        RecoverySnapshot {
            name: name.into(),
            taken_at_unix_ms: 1_700_000_000_000,
            reason: SnapshotReason::Daily,
            bytes: 4096,
        }
    }

    fn capabilities() -> CapabilitySnapshot {
        CapabilitySnapshot {
            resource_budgets: None,
            observed_at_unix_ms: 1,
            core_version: "1.43.0".into(),
            platform: Platform {
                operating_system: "linux".into(),
                architecture: "x86_64".into(),
            },
            external_tools: vec![
                ExternalToolStatus {
                    tool: ExternalTool::Git,
                    availability: ToolAvailability::Present {
                        version: "git version 2.45.0\n".into(),
                    },
                },
                ExternalToolStatus {
                    tool: ExternalTool::GitLfs,
                    availability: ToolAvailability::Missing,
                },
            ],
            credential_store: CredentialStoreStatus::Locked {
                kind: CredentialStoreKind::SecretService,
            },
            crafts: vec![InstalledCraft {
                subagent_control: None,
                craft_id: "jet.codex".into(),
                version: "1.0.0".into(),
                harnesses: vec!["codex".into()],
            }],
            harnesses: vec!["codex".into()],
            degraded: vec![
                DegradedCondition::MissingExternalTool {
                    tool: ExternalTool::GitLfs,
                },
                DegradedCondition::NoHarnessAvailable,
                DegradedCondition::CredentialStoreLocked {
                    kind: CredentialStoreKind::SecretService,
                },
            ],
        }
    }

    #[test]
    fn recovery_and_security_map_every_state_without_snapshot_names() {
        assert_eq!(
            serde_json::to_value(recovery_view(None, Vec::new())).unwrap(),
            json!({"kind": "unsupported"})
        );
        let serving = RecoveryStatus {
            state: RecoveryState::Serving,
            reason: None,
            snapshots: vec![snapshot_file("plane-1-daily.sqlite3")],
            deletion_ledger: Some(DeletionLedgerStatus::Verified { deletions: 7 }),
        };
        assert_eq!(
            serde_json::to_value(recovery_view(Some(&serving), Vec::new())).unwrap(),
            json!({"kind": "serving", "snapshotCount": 1, "snapshots": [], "ledger": {"kind": "verified", "deletions": "7"}})
        );
        let read_only = RecoveryStatus {
            state: RecoveryState::ReadOnly,
            reason: Some(RecoveryReason::MigrationFailed),
            snapshots: vec![],
            deletion_ledger: Some(DeletionLedgerStatus::Corrupt),
        };
        assert_eq!(
            serde_json::to_value(recovery_view(Some(&read_only), Vec::new())).unwrap(),
            json!({"kind": "read_only", "reason": "migration_failed", "snapshotCount": 0, "snapshots": [], "ledger": {"kind": "corrupt"}})
        );
        let old_minor = RecoveryStatus {
            state: RecoveryState::ReadOnly,
            reason: Some(RecoveryReason::IntegrityCheckFailed),
            snapshots: vec![],
            deletion_ledger: None,
        };
        assert_eq!(
            serde_json::to_value(recovery_view(Some(&old_minor), Vec::new())).unwrap()["ledger"],
            json!({"kind": "unsupported"})
        );

        assert_eq!(
            serde_json::to_value(security_view(None, None)).unwrap(),
            json!({"kind": "absent"})
        );
        assert_eq!(
            serde_json::to_value(security_view(Some(&SecurityState::Trusted), None)).unwrap(),
            json!({"kind": "trusted"})
        );
        let degraded = SecurityState::Degraded {
            breach: AuditBreach::RecordAltered { sequence: 41 },
            epoch: 2,
            head: None,
            store_sequence: 50,
        };
        assert_eq!(
            serde_json::to_value(security_view(Some(&degraded), None)).unwrap(),
            json!({"kind": "degraded", "breach": "record_altered", "breachSequence": "41", "epoch": "2", "exported": false})
        );
        // Evidence saved under another epoch does not count for this one.
        assert_eq!(
            serde_json::to_value(security_view(Some(&degraded), Some(1))).unwrap()["exported"],
            json!(false)
        );
        assert_eq!(
            serde_json::to_value(security_view(Some(&degraded), Some(2))).unwrap()["exported"],
            json!(true)
        );
        let missing = SecurityState::Degraded {
            breach: AuditBreach::HeadMissing,
            epoch: 1,
            head: None,
            store_sequence: 3,
        };
        assert_eq!(
            serde_json::to_value(security_view(Some(&missing), None)).unwrap()["breachSequence"],
            json!(null)
        );
    }

    #[test]
    fn health_view_bounds_native_text_and_links_degraded_conditions() {
        let mut snapshot = capabilities();
        snapshot.crafts[0].craft_id = "bad\u{7}id".into();
        let view = health_view(
            PlaneId::Local,
            "This computer".into(),
            ProtocolKnowledge::default().view(),
            &status(None, Some(SecurityState::Trusted)),
            Vec::new(),
            Some(&snapshot),
            None,
            true,
            Some(512),
            None,
            vec![SectionIssue {
                section: "retention",
                error: PublicError::internal(),
            }],
        );
        let value = serde_json::to_value(view).unwrap();
        assert_eq!(value["planeId"], "local");
        assert_eq!(
            value["service"],
            json!({"coreVersion": "1.43.0", "daemonStarts": "3", "startedAtUnixMs": "1700000000000"})
        );
        assert_eq!(
            value["app"]["supportedProtocol"],
            format!("1.{}", jet_protocol::PROTOCOL_MINOR)
        );
        assert_eq!(value["app"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(value["platform"], "linux · x86_64");
        assert_eq!(
            value["tools"],
            json!([
                {"tool": "git", "label": "Git", "version": "git version 2.45.0"},
                {"tool": "git_lfs", "label": "Git LFS", "version": null},
            ])
        );
        assert_eq!(value["crafts"][0]["id"], "unavailable");
        assert_eq!(
            value["credentialStore"],
            json!({"state": "locked", "kind": "secret_service"})
        );
        assert_eq!(
            value["degraded"],
            json!([
                {"kind": "missing_external_tool", "label": "Git LFS is not available", "target": null},
                {"kind": "no_harness_available", "label": "No Harness is installed",
                 "target": {"pane": "agents", "section": "harnesses"}},
                {"kind": "credential_store_locked", "label": "Secure storage is locked",
                 "target": {"pane": "agents", "section": "accounts"}},
            ])
        );
        assert_eq!(value["storage"], json!({"disposableMiB": 512}));
        assert_eq!(value["retention"], json!({"graceDays": null}));
        assert_eq!(value["pendingEpoch"], true);
        assert_eq!(value["issues"][0]["section"], "retention");

        // Without capabilities the sections are empty, never invented.
        let bare = serde_json::to_value(health_view(
            PlaneId::Local,
            String::new(),
            ProtocolKnowledge::default().view(),
            &status(None, None),
            Vec::new(),
            None,
            None,
            false,
            None,
            None,
            Vec::new(),
        ))
        .unwrap();
        assert_eq!(bare["platform"], json!(null));
        assert_eq!(bare["credentialStore"], json!(null));
        assert_eq!(bare["tools"], json!([]));
        assert_eq!(bare["degraded"], json!([]));
        assert_eq!(bare["pendingEpoch"], false);
    }

    async fn answer(
        writer: &mut jet_protocol::FrameWriter<tokio::net::unix::OwnedWriteHalf>,
        stream: jet_protocol::StreamId,
        message: &ClientMessage,
        result: QueryResponse,
    ) {
        reply(
            writer,
            stream,
            ServerMessage::QueryResult {
                id: request_id(message),
                result,
            },
        )
        .await;
    }

    async fn refuse(
        writer: &mut jet_protocol::FrameWriter<tokio::net::unix::OwnedWriteHalf>,
        stream: jet_protocol::StreamId,
        message: &ClientMessage,
        category: ErrorCategory,
        code: &str,
    ) {
        reply(
            writer,
            stream,
            ServerMessage::Error {
                id: Some(request_id(message)),
                error: WireError {
                    category,
                    code: code.into(),
                    retryable: false,
                    message: "daemon text never crosses".into(),
                    revision_conflict: None,
                    restart: None,
                    recovery_actions: Vec::new(),
                },
            },
        )
        .await;
    }

    #[tokio::test]
    async fn a_health_read_uses_one_connection_and_reports_failed_sections_as_issues() {
        let setup = setup();
        let socket = setup.directory.path().join("health-jetd.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let id = Uuid::from_u128(0x4ea1);
        setup.bridge.planes.insert_for_test(
            id,
            "Build box",
            PlaneClient::new(
                socket,
                CLIENT,
                [Duration::from_millis(1)],
                Duration::from_millis(1),
            ),
        );
        let plane_id = id.to_string();
        let degraded = SecurityState::Degraded {
            breach: AuditBreach::HeadDiverged,
            epoch: 4,
            head: None,
            store_sequence: 12,
        };
        let (view, ()) = tokio::join!(load_health(&setup.bridge, &plane_id, true), async {
            let (mut reader, mut writer) = accept(&listener, CLIENT).await;
            let (stream, message) = next_message(&mut reader).await;
            assert!(matches!(
                message,
                ClientMessage::Query {
                    query: QueryRequest::Status,
                    ..
                }
            ));
            let recovery = RecoveryStatus {
                state: RecoveryState::Serving,
                reason: None,
                snapshots: vec![snapshot_file("a.sqlite3"), snapshot_file("b.sqlite3")],
                deletion_ledger: Some(DeletionLedgerStatus::Verified { deletions: 0 }),
            };
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::Status(status(Some(recovery), Some(degraded.clone()))),
            )
            .await;
            // Check again asks for a fresh observation.
            let (stream, message) = next_message(&mut reader).await;
            assert!(matches!(
                message,
                ClientMessage::Query {
                    query: QueryRequest::Capabilities {
                        observation: jet_protocol::CapabilityObservation::Fresh
                    },
                    ..
                }
            ));
            refuse(
                &mut writer,
                stream,
                &message,
                ErrorCategory::Unavailable,
                "capability.observation_failed",
            )
            .await;
            let (stream, message) = next_message(&mut reader).await;
            assert!(matches!(
                message,
                ClientMessage::Query {
                    query: QueryRequest::Settings {
                        scope: SettingScope::Plane,
                        selection: SettingSelection::Key {
                            key: SettingKey::StorageDisposableMiB
                        },
                    },
                    ..
                }
            ));
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::Settings(SettingSnapshot {
                    cursor: 9,
                    scope: SettingScope::Plane,
                    settings: vec![ResolvedSetting {
                        key: SettingKey::StorageDisposableMiB,
                        value: SettingValue::Count(2048),
                        source: SettingSource::BuiltIn,
                    }],
                }),
            )
            .await;
            let (stream, message) = next_message(&mut reader).await;
            refuse(
                &mut writer,
                stream,
                &message,
                ErrorCategory::Unauthorized,
                "unauthorized",
            )
            .await;
        });
        let value = serde_json::to_value(view.unwrap()).unwrap();
        assert_eq!(value["planeLabel"], "Build box");
        assert_eq!(value["recovery"]["snapshotCount"], 2);
        // Snapshots cross as opaque tokens, never as their file names.
        let listed = value["recovery"]["snapshots"].as_array().unwrap();
        assert_eq!(listed.len(), 2);
        assert!(!value.to_string().contains("a.sqlite3"));
        assert!(Uuid::parse_str(listed[0]["snapshotId"].as_str().unwrap()).is_ok());
        assert_eq!(listed[0]["reason"], "daily");
        assert_eq!(listed[0]["bytes"], "4096");
        assert_eq!(value["security"]["breach"], "head_diverged");
        assert_eq!(value["platform"], json!(null));
        assert_eq!(value["storage"]["disposableMiB"], 2048);
        assert_eq!(value["retention"]["graceDays"], json!(null));
        assert_eq!(value["issues"][0]["section"], "capabilities");
        assert_eq!(value["issues"][0]["error"]["planeId"], plane_id);
        assert_eq!(value["issues"][1]["section"], "retention");
        assert_eq!(value["issues"][1]["error"]["category"], "unauthorized");
        // The status read seeded the registry's health and protocol knowledge.
        assert!(value["protocol"]["atLeast"].as_u64().unwrap() >= 37);
        assert_eq!(
            setup.bridge.planes.health(PlaneId::Remote(id)),
            crate::jet::planes::PlaneHealth::from_status(&status(
                Some(RecoveryStatus {
                    state: RecoveryState::Serving,
                    reason: None,
                    snapshots: vec![],
                    deletion_ledger: Some(DeletionLedgerStatus::Verified { deletions: 0 }),
                }),
                Some(degraded),
            ))
        );
    }

    #[tokio::test]
    async fn a_failed_status_read_is_fatal_and_names_its_plane() {
        let directory = tempfile::tempdir().unwrap();
        let bridge = bridge(directory.path());
        let offline = load_health(&bridge, "local", false).await.unwrap_err();
        assert_eq!(offline.category, "offline");
        assert_eq!(offline.plane_id.as_deref(), Some("local"));
        let unknown = load_health(&bridge, "../jetd.sock", false)
            .await
            .unwrap_err();
        assert_eq!(unknown.category, "invalid_input");
    }
}
