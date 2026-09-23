//! Recovery snapshots and the Recovery purge (Wave 3.3 §4.4) for the
//! Settings window's Safety › Recovery section.
//!
//! Snapshot names are server file names and never cross IPC: each health
//! read hands out opaque tokens, reused for a name that is still listed, and
//! a restore review names its snapshot only through a token read earlier.
//!
//! A restore or a purge is prepared as a native review after a fresh status
//! read shows it can run. Neither Command is deduplicated by a receipt
//! (`docs/recovery.md`: a second restore meets `recovery.not_read_only`, a
//! second purge takes another snapshot), so every review has its own
//! Command ID and is sent at most once. An outcome the shell cannot confirm
//! is recorded as `unconfirmed`; the webview then re-reads the Plane and
//! offers a new review, never an automatic resend.
//!
//! A new audit epoch (ADR-0105) is reviewed here too, after this app saved
//! the evidence of the degraded epoch. Unlike restore and purge it is
//! receipt-deduplicated: its review ID is its Command ID, an uncertain send
//! stays unresolved, and "Try again" resends the same ID.
use std::{collections::HashMap, sync::Mutex};

use jet_client::{Client, ClientError};
use jet_protocol::{
    DeletionLedgerStatus, PlaneStatus, RecoverySnapshot, RecoveryState as StoreState,
    SecurityState, SnapshotReason, DELETION_LEDGER_MINOR, SECURITY_AUDIT_MINOR,
    STORE_RECOVERY_MINOR,
};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use super::super::{
    errors::PublicError,
    ledger::{Attempt, Ledger, RECOVERY_CODES},
    planes::{PlaneBinding, PlaneId},
    retention::definite,
    settings::plane_client,
    JetBridge,
};

/// Snapshots listed per health read. Rotation keeps about a dozen
/// (`docs/recovery.md`, Retention); anything past this is counted only.
pub(super) const MAX_LISTED_SNAPSHOTS: usize = 64;
/// Longest Plane handle accepted before it reaches the registry.
const MAX_PLANE_ID_BYTES: usize = 64;
/// Longest name of the file a restore moved aside that is shown.
const MAX_REPLACED_NAME: usize = 96;

/// The ledger scope of every restore review of a Plane: one unresolved
/// restore at a time.
const RESTORE_SCOPE: Uuid = Uuid::from_u128(0x7265_7374_6f72_6500_0000_0000_0000_0001);
/// The ledger scope of every purge review of a Plane.
const PURGE_SCOPE: Uuid = Uuid::from_u128(0x7075_7267_6500_0000_0000_0000_0000_0002);
/// The ledger scope of every new-audit-epoch review of a Plane.
const EPOCH_SCOPE: Uuid = Uuid::from_u128(0x6570_6f63_6800_0000_0000_0000_0000_0003);

pub(crate) struct RecoveryState {
    actions: Ledger<RecoveryReview, RecoveryOutcome>,
    /// Per Plane: the snapshot tokens of the last read, for the Plane
    /// identity that read reported.
    snapshot_tokens: Mutex<HashMap<PlaneId, PlaneSnapshots>>,
}

impl Default for RecoveryState {
    fn default() -> Self {
        Self {
            actions: Ledger::new(RECOVERY_CODES),
            snapshot_tokens: Mutex::new(HashMap::new()),
        }
    }
}

#[derive(Debug)]
struct PlaneSnapshots {
    identity: Uuid,
    /// Token → snapshot name.
    names: HashMap<Uuid, String>,
}

/// One listed snapshot. `snapshotId` is an opaque token, never the name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotView {
    snapshot_id: String,
    taken_at_unix_ms: String,
    reason: &'static str,
    bytes: String,
}

impl RecoveryState {
    /// Rebuilds a Plane's tokens from the snapshots a status read listed,
    /// reusing the token of every name that is still listed. A different
    /// Plane identity starts over, so no token outlives the Plane it named.
    pub(super) fn refresh(
        &self,
        plane: PlaneId,
        identity: Uuid,
        snapshots: &[RecoverySnapshot],
    ) -> Result<Vec<SnapshotView>, PublicError> {
        let mut all = self
            .snapshot_tokens
            .lock()
            .map_err(|_| PublicError::internal())?;
        let previous: HashMap<String, Uuid> = all
            .remove(&plane)
            .filter(|held| held.identity == identity)
            .map(|held| {
                held.names
                    .into_iter()
                    .map(|(token, name)| (name, token))
                    .collect()
            })
            .unwrap_or_default();
        let mut names = HashMap::new();
        let mut views = Vec::new();
        for snapshot in snapshots.iter().take(MAX_LISTED_SNAPSHOTS) {
            if names.values().any(|name| name == &snapshot.name) {
                // A name listed twice is one snapshot; show it once.
                continue;
            }
            let token = previous
                .get(&snapshot.name)
                .copied()
                .unwrap_or_else(Uuid::new_v4);
            names.insert(token, snapshot.name.clone());
            views.push(SnapshotView {
                snapshot_id: token.to_string(),
                taken_at_unix_ms: snapshot.taken_at_unix_ms.to_string(),
                reason: reason_name(snapshot.reason),
                bytes: snapshot.bytes.to_string(),
            });
        }
        all.insert(plane, PlaneSnapshots { identity, names });
        Ok(views)
    }

    /// The snapshot name a token stood for on this Plane identity.
    fn name(
        &self,
        plane: PlaneId,
        identity: Uuid,
        token: Uuid,
    ) -> Result<Option<String>, PublicError> {
        Ok(self
            .snapshot_tokens
            .lock()
            .map_err(|_| PublicError::internal())?
            .get(&plane)
            .filter(|held| held.identity == identity)
            .and_then(|held| held.names.get(&token).cloned()))
    }

    /// Drops a Plane's tokens: its snapshots changed.
    fn clear(&self, plane: PlaneId) {
        if let Ok(mut all) = self.snapshot_tokens.lock() {
            all.remove(&plane);
        }
    }
}

pub(super) fn reason_name(reason: SnapshotReason) -> &'static str {
    match reason {
        SnapshotReason::Daily => "daily",
        SnapshotReason::Migration => "migration",
        SnapshotReason::Maintenance => "maintenance",
    }
}

// ---------------------------------------------------------------------------
// Reviews
// ---------------------------------------------------------------------------

/// What the webview asks to review. A snapshot is named only by a token.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RecoveryAction {
    RestoreSnapshot { snapshot_id: String },
    // Struct variants, so `deny_unknown_fields` applies to them too.
    PurgeSnapshots {},
    BeginAuditEpoch {},
}

/// The immutable native request a review stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecoveryReview {
    Restore {
        snapshot: String,
        taken_at_unix_ms: i64,
        reason: SnapshotReason,
    },
    Purge,
    /// Begin a new audit epoch; the Command takes no argument.
    Epoch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum RecoveryReviewView {
    RestoreSnapshot {
        review_id: String,
        plane_label: String,
        taken_at_unix_ms: String,
        reason: &'static str,
        bytes: String,
    },
    PurgeSnapshots {
        review_id: String,
        plane_label: String,
        snapshot_count: usize,
        total_bytes: String,
        deletions_recorded: String,
        /// A rollback copy for the previous release may be removed too.
        includes_rollback: bool,
    },
    BeginAuditEpoch {
        review_id: String,
        plane_label: String,
        /// The epoch that failed to validate.
        degraded_epoch: String,
        breach: &'static str,
        /// The audit position the saved evidence reaches.
        exported_through: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum RecoveryOutcome {
    /// The store is now the reviewed snapshot.
    Restored {
        taken_at_unix_ms: String,
        reason: &'static str,
        /// The file the damaged store was moved to, beside the store, when
        /// its name is plain enough to show.
        replaced_name: Option<String>,
    },
    /// A new snapshot was taken and this many older ones were removed.
    Purged { removed_count: usize },
    /// The audit now records under this new epoch.
    EpochBegun { epoch: String },
    /// The Plane or this app refused it; nothing changed.
    Refused { error: PublicError },
    /// The request may or may not have run. Re-read the Plane.
    Unconfirmed { error: PublicError },
}

fn snapshot_gone() -> PublicError {
    PublicError::conflict(
        "recovery.snapshot_gone",
        "This snapshot is no longer listed. Reload Recovery.",
    )
}

fn not_read_only() -> PublicError {
    PublicError::conflict(
        "recovery.not_read_only_local",
        "This Plane isn't in read-only recovery. Reload Recovery.",
    )
}

fn ledger_corrupt() -> PublicError {
    PublicError::conflict(
        "recovery.deletion_ledger_corrupt",
        "Jet can't verify its record of deleted data, so it won't restore a snapshot.",
    )
}

fn purge_unavailable() -> PublicError {
    PublicError::conflict(
        "recovery.purge_unavailable",
        "Old snapshots can't be removed right now. Reload Recovery.",
    )
}

fn not_degraded() -> PublicError {
    PublicError::conflict(
        "audit.not_degraded_local",
        "This Plane's security audit isn't degraded. Reload the audit.",
    )
}

fn export_required() -> PublicError {
    PublicError::conflict(
        "audit.export_required",
        "Save the audit evidence before starting a new audit period.",
    )
}

fn review_expired() -> PublicError {
    PublicError::invalid_input(
        RECOVERY_CODES.review_expired,
        "This review expired. Review the request again.",
    )
}

/// Reviews a restore, a purge or a new audit epoch on one Plane after a
/// fresh status read.
#[tauri::command]
pub(crate) async fn prepare_recovery_action(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    action: RecoveryAction,
) -> Result<RecoveryReviewView, PublicError> {
    prepare(&bridge, &plane_id, action).await
}

/// Sends a reviewed restore or purge at most once, or a reviewed audit
/// epoch until its outcome is known.
#[tauri::command]
pub(crate) async fn execute_recovery_action(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    review_id: String,
) -> Result<RecoveryOutcome, PublicError> {
    execute(&bridge, &plane_id, &review_id).await
}

pub(super) async fn prepare(
    bridge: &JetBridge,
    plane_id: &str,
    action: RecoveryAction,
) -> Result<RecoveryReviewView, PublicError> {
    // Validated before any I/O: a token is a UUID this shell issued.
    let token = match &action {
        RecoveryAction::RestoreSnapshot { snapshot_id } => {
            Some(Uuid::parse_str(snapshot_id).map_err(|_| snapshot_gone())?)
        }
        RecoveryAction::PurgeSnapshots {} | RecoveryAction::BeginAuditEpoch {} => None,
    };
    let (binding, client) = plane_client(bridge, plane_id)?;
    let plane = binding.plane;
    async {
        let connection = client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let status = connection
            .status()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        bridge.planes.observe_status(plane, &status);
        // The review runs only against the Plane this read described.
        let reviewed = PlaneBinding {
            plane,
            identity: Some(status.plane_id),
        };
        bridge.bound(&reviewed)?;
        let plane_label = bridge.planes.label(plane).unwrap_or_default();
        match action {
            RecoveryAction::RestoreSnapshot { .. } => {
                let token = token.ok_or_else(snapshot_gone)?;
                prepare_restore(bridge, reviewed, &status, token, plane_label)
            }
            RecoveryAction::PurgeSnapshots {} => {
                prepare_purge(bridge, reviewed, &status, plane_label)
            }
            RecoveryAction::BeginAuditEpoch {} => {
                prepare_epoch(bridge, reviewed, &status, plane_label)
            }
        }
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

fn prepare_restore(
    bridge: &JetBridge,
    binding: PlaneBinding,
    status: &PlaneStatus,
    token: Uuid,
    plane_label: String,
) -> Result<RecoveryReviewView, PublicError> {
    let state = &bridge.system.recovery;
    let name = state.name(binding.plane, status.plane_id, token)?;
    let Some(recovery) = status.recovery.as_ref() else {
        return Err(not_read_only());
    };
    state.refresh(binding.plane, status.plane_id, &recovery.snapshots)?;
    let name = name.ok_or_else(snapshot_gone)?;
    if recovery.state != StoreState::ReadOnly {
        return Err(not_read_only());
    }
    if recovery.deletion_ledger == Some(DeletionLedgerStatus::Corrupt) {
        return Err(ledger_corrupt());
    }
    let snapshot = recovery
        .snapshots
        .iter()
        .find(|snapshot| snapshot.name == name)
        .ok_or_else(snapshot_gone)?;
    let review_id = state.actions.issue(
        binding,
        RESTORE_SCOPE,
        RecoveryReview::Restore {
            snapshot: name,
            taken_at_unix_ms: snapshot.taken_at_unix_ms,
            reason: snapshot.reason,
        },
    )?;
    Ok(RecoveryReviewView::RestoreSnapshot {
        review_id: review_id.to_string(),
        plane_label,
        taken_at_unix_ms: snapshot.taken_at_unix_ms.to_string(),
        reason: reason_name(snapshot.reason),
        bytes: snapshot.bytes.to_string(),
    })
}

/// A purge runs only on a serving Plane whose Deletion ledger is verified
/// and whose audit is trusted (`docs/recovery.md`, Recovery purge).
fn prepare_purge(
    bridge: &JetBridge,
    binding: PlaneBinding,
    status: &PlaneStatus,
    plane_label: String,
) -> Result<RecoveryReviewView, PublicError> {
    let Some(recovery) = status.recovery.as_ref() else {
        return Err(purge_unavailable());
    };
    let state = &bridge.system.recovery;
    state.refresh(binding.plane, status.plane_id, &recovery.snapshots)?;
    let Some(DeletionLedgerStatus::Verified { deletions }) = recovery.deletion_ledger else {
        return Err(purge_unavailable());
    };
    if recovery.state != StoreState::Serving
        || !matches!(status.security, Some(SecurityState::Trusted))
    {
        return Err(purge_unavailable());
    }
    let total_bytes = recovery.snapshots.iter().fold(0_u64, |total, snapshot| {
        total.saturating_add(snapshot.bytes)
    });
    let includes_rollback = recovery
        .snapshots
        .iter()
        .any(|snapshot| snapshot.reason == SnapshotReason::Migration);
    let review_id = state
        .actions
        .issue(binding, PURGE_SCOPE, RecoveryReview::Purge)?;
    Ok(RecoveryReviewView::PurgeSnapshots {
        review_id: review_id.to_string(),
        plane_label,
        snapshot_count: recovery.snapshots.len(),
        total_bytes: total_bytes.to_string(),
        deletions_recorded: deletions.to_string(),
        includes_rollback,
    })
}

/// A new audit epoch needs a degraded audit whose evidence this app saved
/// for that same epoch and Plane identity (ADR-0105; a UI rule, not policy).
fn prepare_epoch(
    bridge: &JetBridge,
    binding: PlaneBinding,
    status: &PlaneStatus,
    plane_label: String,
) -> Result<RecoveryReviewView, PublicError> {
    let Some(SecurityState::Degraded { breach, epoch, .. }) = &status.security else {
        return Err(not_degraded());
    };
    let mark = bridge
        .audit
        .mark(binding.plane, status.plane_id)?
        .filter(|mark| mark.epoch == Some(*epoch))
        .ok_or_else(export_required)?;
    let review_id =
        bridge
            .system
            .recovery
            .actions
            .issue(binding, EPOCH_SCOPE, RecoveryReview::Epoch)?;
    Ok(RecoveryReviewView::BeginAuditEpoch {
        review_id: review_id.to_string(),
        plane_label,
        degraded_epoch: epoch.to_string(),
        breach: super::breach_name(breach).0,
        exported_through: mark.through.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

pub(super) async fn execute(
    bridge: &JetBridge,
    plane_id: &str,
    review_id: &str,
) -> Result<RecoveryOutcome, PublicError> {
    if plane_id.len() > MAX_PLANE_ID_BYTES {
        return Err(super::super::planes::unknown_plane());
    }
    let plane = PlaneId::parse(plane_id)?;
    let refused = |error: PublicError| RecoveryOutcome::Refused {
        error: error.with_plane(plane.to_string()),
    };
    let Ok(id) = Uuid::parse_str(review_id) else {
        return Ok(refused(review_expired()));
    };
    let (binding, attempt) = match bridge
        .system
        .recovery
        .actions
        .attempt(id, plane, |_| Ok(()))
    {
        Ok(attempt) => attempt,
        // Refused before anything was sent; the review is unchanged.
        Err(error) => return Ok(refused(error)),
    };
    let (review, fresh) = match attempt {
        Attempt::Known(outcome) => return Ok(outcome),
        // The epoch Command is deduplicated by its receipt: resend its ID.
        Attempt::Retry(RecoveryReview::Epoch) => (RecoveryReview::Epoch, false),
        // Sent once already and its outcome was never recorded. Neither
        // Command is replayed from a receipt, so it is never sent again.
        Attempt::Retry(_) => {
            return Ok(record(
                bridge,
                id,
                refused(PublicError::invalid_input(
                    "client.review_used",
                    "This review was already used. Check the Plane and review again.",
                )),
            ))
        }
        Attempt::Fresh(review) => (review, true),
    };
    let client = match bridge.bound(&binding) {
        Ok(client) => client,
        Err(error) if fresh => return Ok(record(bridge, id, refused(error))),
        // An epoch resend whose Plane moved: the first send's outcome is
        // still unknown, so nothing is recorded.
        Err(error) => return Err(error.with_plane(plane.to_string())),
    };
    let connection = match client.connect().await {
        Ok(connection) => connection,
        Err(error) => {
            let error = bridge.settle(&binding, PublicError::from_client(&error));
            if !fresh {
                return Err(error);
            }
            // Nothing was sent: a definite refusal, and a new review is safe.
            return Ok(record(bridge, id, RecoveryOutcome::Refused { error }));
        }
    };
    let outcome = match review {
        RecoveryReview::Restore {
            snapshot,
            taken_at_unix_ms,
            reason,
        } => {
            restore(
                bridge,
                id,
                binding,
                &connection,
                snapshot,
                taken_at_unix_ms,
                reason,
            )
            .await
        }
        RecoveryReview::Purge => purge(bridge, id, binding, &connection).await,
        RecoveryReview::Epoch => return begin_epoch(bridge, id, binding, &connection).await,
    };
    Ok(record(bridge, id, outcome))
}

/// Sends the epoch Command under the review ID. A definite outcome is
/// recorded; an uncertain one is returned as `Err` and leaves the review
/// attempted, so "Try again" resends the same Command ID.
async fn begin_epoch(
    bridge: &JetBridge,
    id: Uuid,
    binding: PlaneBinding,
    connection: &Client,
) -> Result<RecoveryOutcome, PublicError> {
    match connection.begin_audit_epoch(id).await {
        Ok(epoch) => {
            bridge
                .planes
                .observe_success(binding.plane, SECURITY_AUDIT_MINOR);
            // The saved evidence belonged to the epoch that just ended.
            bridge.audit.clear(binding.plane);
            // Best effort: the registry's Security state is current before
            // anything this app gates on it runs.
            if let Ok(status) = connection.status().await {
                bridge.planes.observe_status(binding.plane, &status);
            }
            Ok(record(
                bridge,
                id,
                RecoveryOutcome::EpochBegun {
                    epoch: epoch.to_string(),
                },
            ))
        }
        Err(error) => {
            let public = PublicError::from_client(&error);
            let definite = definite(&error, &public);
            let error = bridge.settle(&binding, public);
            if definite {
                Ok(record(bridge, id, RecoveryOutcome::Refused { error }))
            } else {
                Err(error)
            }
        }
    }
}

async fn restore(
    bridge: &JetBridge,
    id: Uuid,
    binding: PlaneBinding,
    connection: &Client,
    snapshot: String,
    taken_at_unix_ms: i64,
    reason: SnapshotReason,
) -> RecoveryOutcome {
    let result = connection
        .restore_recovery_snapshot(id, snapshot.clone())
        .await;
    match result {
        Ok(restored) => {
            if restored.snapshot != snapshot {
                // The Plane answered about another snapshot: what it did
                // is unknown, so the webview re-reads it.
                return RecoveryOutcome::Unconfirmed {
                    error: bridge.settle(&binding, PublicError::internal()),
                };
            }
            bridge
                .planes
                .observe_success(binding.plane, STORE_RECOVERY_MINOR);
            store_replaced(bridge, binding.plane);
            // Best effort: the registry's health (read-only, audit) is
            // current before any other request of this app is gated on it.
            if let Ok(status) = connection.status().await {
                bridge.planes.observe_status(binding.plane, &status);
            }
            RecoveryOutcome::Restored {
                taken_at_unix_ms: taken_at_unix_ms.to_string(),
                reason: reason_name(reason),
                replaced_name: replaced_name(&restored.replaced),
            }
        }
        Err(error) => unsettled(bridge, binding, &error),
    }
}

async fn purge(
    bridge: &JetBridge,
    id: Uuid,
    binding: PlaneBinding,
    connection: &Client,
) -> RecoveryOutcome {
    match connection.purge_recovery_snapshots(id).await {
        Ok(purged) => {
            bridge
                .planes
                .observe_success(binding.plane, DELETION_LEDGER_MINOR);
            // The listed snapshots changed; the next read issues tokens anew.
            bridge.system.recovery.clear(binding.plane);
            RecoveryOutcome::Purged {
                removed_count: purged.removed.len(),
            }
        }
        Err(error) => unsettled(bridge, binding, &error),
    }
}

/// A failed send: a definite refusal, or an outcome nobody can confirm.
fn unsettled(bridge: &JetBridge, binding: PlaneBinding, error: &ClientError) -> RecoveryOutcome {
    let public = PublicError::from_client(error);
    let definite = definite(error, &public);
    let error = bridge.settle(&binding, public);
    if definite {
        RecoveryOutcome::Refused { error }
    } else {
        RecoveryOutcome::Unconfirmed { error }
    }
}

/// The store was replaced by an older one: state moves backwards, so every
/// native cache of that Plane's store is dropped (wave 3.3 §4.4).
fn store_replaced(bridge: &JetBridge, plane: PlaneId) {
    bridge.system.recovery.clear(plane);
    bridge.audit.clear(plane);
    bridge.retention.plane_restored(plane);
}

fn record(bridge: &JetBridge, id: Uuid, outcome: RecoveryOutcome) -> RecoveryOutcome {
    bridge.system.recovery.actions.record(id, outcome.clone());
    outcome
}

/// The moved-aside file's own name (`plane.sqlite3.damaged-<unix_ms>`),
/// shown only when it is a plain file name.
fn replaced_name(value: &str) -> Option<String> {
    let plain = !value.is_empty()
        && value.len() <= MAX_REPLACED_NAME
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    plain.then(|| value.to_owned())
}

#[cfg(test)]
mod tests;
