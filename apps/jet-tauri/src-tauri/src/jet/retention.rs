//! Jet Trash in the main window (Wave 3.3 §4.3): per-Plane Trash lists, the
//! selected task's Trash status, reviewed Forget in Jet and Delete
//! everywhere, and Restore.
//!
//! Forget and Delete everywhere go through a native review ledger whose
//! review ID is the daemon Command ID. The first attempt locks the mode, a
//! Delete everywhere that stops work needs an explicit acknowledgement, and a
//! fresh Delete everywhere re-reads the task first so activity that started
//! after the review is never stopped unseen. Restore Command IDs are keyed by
//! the Trash entry the shell itself read, so a task trashed again gets a new
//! ID and can never replay an old receipt.
//!
//! Auto-delete rules for the Settings window live in [`autodelete`].
use std::{collections::HashMap, sync::Mutex};

use jet_client::{Client, ClientError};
use jet_protocol::{
    ConversationTrash, RetentionPreview, RetentionProtection, SettingKey, SettingScope,
    SettingSelection, SettingValue, TrashEntry, TrashReason, RETENTION_MINOR,
};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use super::{
    conversations::conversation_title,
    errors::PublicError,
    ledger::{Attempt, Ledger, RETENTION_CODES},
    planes::{PlaneBinding, PlaneId},
    JetBridge,
};

pub(crate) mod autodelete;

/// `conversation_trash` answers at most this many entries, soonest expiry
/// first, without a cursor (`jet-store/src/conversation/trash.rs`).
pub(crate) const TRASH_LIMIT: usize = 256;
/// Trash entries remembered per Plane: a full list plus entries learned
/// from status reads of tasks outside it.
const INDEX_CAPACITY: usize = 2 * TRASH_LIMIT;
/// Restore Command IDs kept while their outcome is unknown.
const RESTORE_CAPACITY: usize = 256;
/// Names resolved in one call.
const NAMES_LIMIT: usize = 32;
const WORKSPACE_UNREADABLE: &str = "retention.workspace_unreadable";

type RestoreKey = (PlaneId, Uuid, i64);

pub(crate) struct RetentionState {
    trash_reviews: Ledger<TrashReview, TrashOutcome>,
    /// Per Plane: Conversation → `trashed_at_unix_ms` of the Trash entry the
    /// shell last read. Restore IDs are keyed from here, never from input.
    trash_index: Mutex<HashMap<PlaneId, HashMap<Uuid, i64>>>,
    restores: Mutex<HashMap<RestoreKey, Uuid>>,
    /// Auto-delete rule slots, approval tokens and attribution.
    rules: autodelete::RuleState,
}

impl Default for RetentionState {
    fn default() -> Self {
        Self {
            trash_reviews: Ledger::new(RETENTION_CODES),
            trash_index: Mutex::new(HashMap::new()),
            restores: Mutex::new(HashMap::new()),
            rules: autodelete::RuleState::default(),
        }
    }
}

/// How a reviewed task leaves the task list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TrashMode {
    Forget,
    DeleteEverywhere,
}

/// What the user reviewed. Immutable except for the mode, which the first
/// attempt locks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrashReview {
    conversation: Uuid,
    recorded_active_run: bool,
    recorded_pending_turn: bool,
    workspace_unchecked: bool,
    mode: Option<TrashMode>,
}

impl TrashReview {
    /// Delete everywhere stops work the user must have acknowledged: work
    /// the review showed, or work it could not rule out.
    fn stop_required(&self) -> bool {
        self.recorded_active_run || self.recorded_pending_turn || self.workspace_unchecked
    }
}

/// The attempt check: the first attempt locks the mode, and Delete
/// everywhere needs the stop acknowledgement when the review requires it.
fn lock_mode(
    review: &mut TrashReview,
    mode: TrashMode,
    acknowledge_stop: bool,
) -> Result<(), PublicError> {
    if review.mode.is_some_and(|locked| locked != mode) {
        return Err(PublicError::conflict(
            "retention.mode_locked",
            "Jet must confirm the earlier request before you choose differently.",
        ));
    }
    if mode == TrashMode::DeleteEverywhere && review.stop_required() && !acknowledge_stop {
        return Err(PublicError::invalid_input(
            "retention.stop_unacknowledged",
            "Confirm that the activity in this task stops.",
        ));
    }
    review.mode = Some(mode);
    Ok(())
}

impl RetentionState {
    /// A Recovery snapshot replaced the Plane's store with an older one:
    /// everything read from the old store, every open Trash review and every
    /// pending Command ID of that Plane is dropped, so nothing is replayed
    /// against state that moved backwards (wave 3.3 §4.4).
    pub(crate) fn plane_restored(&self, plane: PlaneId) {
        if let Ok(mut index) = self.trash_index.lock() {
            index.remove(&plane);
        }
        if let Ok(mut restores) = self.restores.lock() {
            restores.retain(|(held, _, _), _| *held != plane);
        }
        self.trash_reviews.clear_plane(plane);
        self.rules.plane_restored(plane);
    }

    /// Replaces a Plane's index with a full Trash list.
    fn replace_index(&self, plane: PlaneId, entries: &[TrashEntry]) -> Result<(), PublicError> {
        let index: HashMap<Uuid, i64> = entries
            .iter()
            .map(|entry| (entry.conversation_id, entry.trashed_at_unix_ms))
            .collect();
        self.prune_restores(plane, &index)?;
        self.trash_index
            .lock()
            .map_err(|_| PublicError::internal())?
            .insert(plane, index);
        Ok(())
    }

    /// Records one task's Trash state from a status read or a Command.
    fn remember(
        &self,
        plane: PlaneId,
        conversation: Uuid,
        trashed_at: Option<i64>,
    ) -> Result<(), PublicError> {
        {
            let mut index = self
                .trash_index
                .lock()
                .map_err(|_| PublicError::internal())?;
            let entries = index.entry(plane).or_default();
            match trashed_at {
                Some(at) => {
                    if !entries.contains_key(&conversation) && entries.len() >= INDEX_CAPACITY {
                        // Any other entry may go: forgetting one only makes its
                        // restore ask for a reload (`retention.trash_unknown`).
                        if let Some(victim) = entries.keys().next().copied() {
                            entries.remove(&victim);
                        }
                    }
                    entries.insert(conversation, at);
                }
                None => {
                    entries.remove(&conversation);
                }
            }
        }
        let mut restores = self.restores.lock().map_err(|_| PublicError::internal())?;
        restores.retain(|(p, c, at), _| {
            !(*p == plane && *c == conversation && Some(*at) != trashed_at)
        });
        Ok(())
    }

    /// Drops restore IDs of entries that are no longer in the Trash as read.
    fn prune_restores(
        &self,
        plane: PlaneId,
        index: &HashMap<Uuid, i64>,
    ) -> Result<(), PublicError> {
        self.restores
            .lock()
            .map_err(|_| PublicError::internal())?
            .retain(|(p, c, at), _| *p != plane || index.get(c) == Some(at));
        Ok(())
    }

    fn trashed_at(&self, plane: PlaneId, conversation: Uuid) -> Result<Option<i64>, PublicError> {
        Ok(self
            .trash_index
            .lock()
            .map_err(|_| PublicError::internal())?
            .get(&plane)
            .and_then(|entries| entries.get(&conversation))
            .copied())
    }

    /// The Command ID for restoring one staging of a task: stable across
    /// retries, new for a new staging.
    fn restore_id(&self, key: RestoreKey) -> Result<Uuid, PublicError> {
        let mut restores = self.restores.lock().map_err(|_| PublicError::internal())?;
        if let Some(id) = restores.get(&key) {
            return Ok(*id);
        }
        if restores.len() >= RESTORE_CAPACITY {
            return Err(PublicError::invalid_input(
                "client.review_limit",
                "Too many requests are waiting for confirmation.",
            ));
        }
        Ok(*restores.entry(key).or_insert_with(Uuid::new_v4))
    }

    fn settle_restore(&self, key: &RestoreKey) -> Result<(), PublicError> {
        self.restores
            .lock()
            .map_err(|_| PublicError::internal())?
            .remove(key);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrashEntryView {
    conversation_id: String,
    reason: &'static str,
    trashed_at_unix_ms: String,
    expires_at_unix_ms: String,
    /// A Transfer tombstone cannot be restored (`docs/plane-transfer.md`).
    restorable: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrashView {
    plane_id: String,
    plane_label: String,
    cursor: String,
    grace_days: Option<u32>,
    /// The list holds the Plane's limit, so more may be in Jet Trash.
    capped: bool,
    entries: Vec<TrashEntryView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrashStatusView {
    trash: Option<TrashEntryView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationNameView {
    conversation_id: String,
    title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProtectionView {
    kind: &'static str,
    label: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrashPreviewView {
    /// Empty when the task is already in Jet Trash: nothing to review.
    review_id: String,
    conversation_id: String,
    title: String,
    plane_label: String,
    /// `None` when the Plane could not inspect the task's workspace.
    protections: Option<Vec<ProtectionView>>,
    workspace_unchecked: bool,
    trash: Option<TrashEntryView>,
    audit_records: Option<String>,
    grace_days: Option<u32>,
    active_run: bool,
    pending_turn: bool,
    stop_acknowledgement_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum TrashOutcome {
    Trashed { entry: TrashEntryView },
    Refused { error: PublicError },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum RestoreOutcome {
    Restored { conversation_id: String },
    Refused { error: PublicError },
}

fn reason_name(reason: TrashReason) -> &'static str {
    match reason {
        TrashReason::ManualForget => "manual_forget",
        TrashReason::AutomaticForget => "automatic_forget",
        TrashReason::DeleteEverywhere => "delete_everywhere",
        TrashReason::AutodeleteRule => "autodelete_rule",
        TrashReason::AutodeleteEverywhere => "autodelete_everywhere",
        TrashReason::PlaneTransfer => "plane_transfer",
    }
}

fn entry_view(entry: &TrashEntry) -> TrashEntryView {
    TrashEntryView {
        conversation_id: entry.conversation_id.to_string(),
        reason: reason_name(entry.reason),
        trashed_at_unix_ms: entry.trashed_at_unix_ms.to_string(),
        expires_at_unix_ms: entry.expires_at_unix_ms.to_string(),
        restorable: entry.reason != TrashReason::PlaneTransfer,
    }
}

fn protection_view(protection: RetentionProtection) -> ProtectionView {
    let (kind, label) = match protection {
        RetentionProtection::ActiveRun => ("active_run", "Activity in progress"),
        RetentionProtection::PendingTurn => ("pending_turn", "Queued message"),
        RetentionProtection::EnabledSchedule => ("enabled_schedule", "Enabled schedule"),
        RetentionProtection::DirtyWorkspace => ("dirty_workspace", "Uncommitted changes"),
        RetentionProtection::UnpushedWork => ("unpushed_work", "Unpushed commits"),
        RetentionProtection::UnresolvedEffect => ("unresolved_effect", "Unfinished action"),
    };
    ProtectionView { kind, label }
}

fn trash_view(
    plane: PlaneId,
    plane_label: String,
    trash: &ConversationTrash,
    grace_days: Option<u32>,
) -> Result<TrashView, PublicError> {
    if trash.entries.len() > TRASH_LIMIT {
        return Err(PublicError::internal());
    }
    Ok(TrashView {
        plane_id: plane.to_string(),
        plane_label,
        cursor: trash.cursor.to_string(),
        grace_days,
        capped: trash.entries.len() == TRASH_LIMIT,
        entries: trash.entries.iter().map(entry_view).collect(),
    })
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn load_trash(
    bridge: State<'_, JetBridge>,
    plane_id: String,
) -> Result<TrashView, PublicError> {
    load_trash_for(&bridge, &plane_id).await
}

#[tauri::command]
pub(crate) async fn load_trash_status(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    conversation_id: String,
) -> Result<TrashStatusView, PublicError> {
    load_trash_status_for(&bridge, &plane_id, &conversation_id).await
}

#[tauri::command]
pub(crate) async fn resolve_conversation_names(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    conversation_ids: Vec<String>,
) -> Result<Vec<ConversationNameView>, PublicError> {
    resolve_names_for(&bridge, &plane_id, &conversation_ids).await
}

#[tauri::command]
pub(crate) async fn preview_trash(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    conversation_id: String,
) -> Result<TrashPreviewView, PublicError> {
    preview_trash_for(&bridge, &plane_id, &conversation_id).await
}

#[tauri::command]
pub(crate) async fn trash_conversation(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    review_id: String,
    mode: TrashMode,
    acknowledge_stop: bool,
) -> Result<TrashOutcome, PublicError> {
    trash_conversation_for(&bridge, &plane_id, &review_id, mode, acknowledge_stop).await
}

#[tauri::command]
pub(crate) async fn restore_conversation(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    conversation_id: String,
) -> Result<RestoreOutcome, PublicError> {
    restore_conversation_for(&bridge, &plane_id, &conversation_id).await
}

pub(super) async fn load_trash_for(
    bridge: &JetBridge,
    plane_id: &str,
) -> Result<TrashView, PublicError> {
    let (binding, client) = bridge.plane(Some(plane_id))?;
    let plane = binding.plane;
    async {
        let connection = client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let trash = connection
            .conversation_trash()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        bridge.planes.observe_success(plane, RETENTION_MINOR);
        let grace = grace_days(&connection).await;
        let view = trash_view(plane, plane_label(bridge, plane), &trash, grace)?;
        bridge.retention.replace_index(plane, &trash.entries)?;
        Ok(view)
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

pub(super) async fn load_trash_status_for(
    bridge: &JetBridge,
    plane_id: &str,
    conversation_id: &str,
) -> Result<TrashStatusView, PublicError> {
    let id = parse_conversation(conversation_id)?;
    let (binding, client) = bridge.plane(Some(plane_id))?;
    let plane = binding.plane;
    async {
        let preview = client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?
            .retention_preview(id)
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        check_preview(&preview, id)?;
        bridge.planes.observe_success(plane, RETENTION_MINOR);
        let trash = preview.trash.as_ref();
        bridge
            .retention
            .remember(plane, id, trash.map(|entry| entry.trashed_at_unix_ms))?;
        Ok(TrashStatusView {
            trash: trash.map(entry_view),
        })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

async fn resolve_names_for(
    bridge: &JetBridge,
    plane_id: &str,
    conversation_ids: &[String],
) -> Result<Vec<ConversationNameView>, PublicError> {
    let ids = parse_names(conversation_ids)?;
    let (binding, client) = bridge.plane(Some(plane_id))?;
    async {
        // One connection for the whole batch. Never remembered as the
        // selection: reading a name is not opening the task.
        let connection = client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let mut names = Vec::with_capacity(ids.len());
        for id in ids {
            let title = match connection.conversation(id).await {
                Ok(snapshot) if snapshot.conversation.conversation_id == id => {
                    Some(conversation_title(&snapshot.conversation))
                }
                _ => None,
            };
            names.push(ConversationNameView {
                conversation_id: id.to_string(),
                title,
            });
        }
        Ok(names)
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

pub(super) async fn preview_trash_for(
    bridge: &JetBridge,
    plane_id: &str,
    conversation_id: &str,
) -> Result<TrashPreviewView, PublicError> {
    let id = parse_conversation(conversation_id)?;
    let (binding, client) = bridge.plane(Some(plane_id))?;
    let plane = binding.plane;
    async {
        let connection = client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        // A Workspace Git the Plane cannot inspect refuses the preview; the
        // review still goes ahead, with nothing ruled out.
        let preview = match connection.retention_preview(id).await {
            Ok(preview) => {
                check_preview(&preview, id)?;
                Some(preview)
            }
            Err(error) if refusal_code(&error) == Some(WORKSPACE_UNREADABLE) => None,
            Err(error) => return Err(PublicError::from_client(&error)),
        };
        bridge.planes.observe_success(plane, RETENTION_MINOR);
        let snapshot = connection
            .conversation(id)
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        if snapshot.conversation.conversation_id != id {
            return Err(PublicError::internal());
        }
        let grace = grace_days(&connection).await;
        let has = |protection| {
            preview
                .as_ref()
                .is_some_and(|preview| preview.protections.contains(&protection))
        };
        let (active_run, pending_turn) = (
            has(RetentionProtection::ActiveRun),
            has(RetentionProtection::PendingTurn),
        );
        let workspace_unchecked = preview.is_none();
        let trash = preview.as_ref().and_then(|preview| preview.trash.as_ref());
        if let Some(preview) = &preview {
            bridge.retention.remember(
                plane,
                id,
                preview.trash.as_ref().map(|entry| entry.trashed_at_unix_ms),
            )?;
        }
        let review = TrashReview {
            conversation: id,
            recorded_active_run: active_run,
            recorded_pending_turn: pending_turn,
            workspace_unchecked,
            mode: None,
        };
        let stop_acknowledgement_required = review.stop_required();
        let review_id = if trash.is_some() {
            String::new()
        } else {
            bridge
                .retention
                .trash_reviews
                .issue(binding, id, review)?
                .to_string()
        };
        Ok(TrashPreviewView {
            review_id,
            conversation_id: id.to_string(),
            title: conversation_title(&snapshot.conversation),
            plane_label: plane_label(bridge, plane),
            protections: preview.as_ref().map(|preview| {
                preview
                    .protections
                    .iter()
                    .copied()
                    .map(protection_view)
                    .collect()
            }),
            workspace_unchecked,
            trash: trash.map(entry_view),
            audit_records: preview
                .as_ref()
                .map(|preview| preview.audit_records.to_string()),
            grace_days: grace,
            active_run,
            pending_turn,
            stop_acknowledgement_required,
        })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

pub(super) async fn trash_conversation_for(
    bridge: &JetBridge,
    plane_id: &str,
    review_id: &str,
    mode: TrashMode,
    acknowledge_stop: bool,
) -> Result<TrashOutcome, PublicError> {
    let plane = PlaneId::parse(plane_id)?;
    let id = Uuid::parse_str(review_id).map_err(|_| {
        PublicError::invalid_input(
            "retention.review_expired",
            "This review expired. Review the request again.",
        )
    })?;
    let (binding, attempt) = match bridge.retention.trash_reviews.attempt(id, plane, |review| {
        lock_mode(review, mode, acknowledge_stop)
    }) {
        Ok(attempt) => attempt,
        // Refused before anything was sent; the review is unchanged.
        Err(error) => {
            return Ok(TrashOutcome::Refused {
                error: error.with_plane(plane.to_string()),
            })
        }
    };
    let (review, fresh) = match attempt {
        Attempt::Known(outcome) => return Ok(outcome),
        Attempt::Fresh(review) => (review, true),
        Attempt::Retry(review) => (review, false),
    };
    // The review executes only on the Plane it was prepared against.
    let client = match bridge.bound(&binding) {
        Ok(client) => client,
        Err(error) => return Ok(record(bridge, id, TrashOutcome::Refused { error })),
    };
    execute_trash(bridge, id, binding, &client, review, fresh)
        .await
        .map_err(|error| bridge.settle(&binding, error))
}

async fn execute_trash(
    bridge: &JetBridge,
    id: Uuid,
    binding: PlaneBinding,
    client: &super::client::PlaneClient,
    review: TrashReview,
    fresh: bool,
) -> Result<TrashOutcome, PublicError> {
    let mode = review.mode.ok_or_else(PublicError::internal)?;
    let refused = |error: PublicError| {
        record(
            bridge,
            id,
            TrashOutcome::Refused {
                error: bridge.settle(&binding, error),
            },
        )
    };
    let connection = match client.connect().await {
        Ok(connection) => connection,
        // Nothing of a first attempt was sent: that is definite, and a new
        // review re-reads the task. A retry may have been applied earlier,
        // so it stays uncertain and is resent unchanged.
        Err(error) if fresh => return Ok(refused(PublicError::from_client(&error))),
        Err(error) => return Err(PublicError::from_client(&error)),
    };
    if fresh && mode == TrashMode::DeleteEverywhere {
        if let Some(stale) = restarted_work(&connection, &review).await {
            return Ok(refused(stale));
        }
    }
    let result = match mode {
        TrashMode::Forget => {
            connection
                .forget_conversation(id, review.conversation)
                .await
        }
        TrashMode::DeleteEverywhere => {
            connection
                .delete_conversation_everywhere(id, review.conversation)
                .await
        }
    };
    match result {
        Ok(entry) => {
            if entry.conversation_id != review.conversation {
                return Err(PublicError::internal());
            }
            bridge
                .planes
                .observe_success(binding.plane, RETENTION_MINOR);
            bridge.retention.remember(
                binding.plane,
                review.conversation,
                Some(entry.trashed_at_unix_ms),
            )?;
            Ok(record(
                bridge,
                id,
                TrashOutcome::Trashed {
                    entry: entry_view(&entry),
                },
            ))
        }
        Err(error) => {
            let public = PublicError::from_client(&error);
            if definite(&error, &public) {
                Ok(refused(public))
            } else {
                Err(public)
            }
        }
    }
}

/// Re-reads the task before a first Delete everywhere. Returns the refusal
/// to record when work started that the user did not see stop, or when the
/// re-read failed (nothing was sent, so a new review is safe).
async fn restarted_work(connection: &Client, review: &TrashReview) -> Option<PublicError> {
    let stale = || {
        PublicError::conflict(
            "retention.review_stale",
            "Activity started in this task after you reviewed it. Review it again.",
        )
    };
    match connection.retention_preview(review.conversation).await {
        Ok(preview) => {
            if preview.conversation_id != review.conversation {
                return Some(PublicError::internal());
            }
            let has = |protection| preview.protections.contains(&protection);
            let new_run = has(RetentionProtection::ActiveRun) && !review.recorded_active_run;
            let new_turn = has(RetentionProtection::PendingTurn) && !review.stop_required();
            (new_run || new_turn).then(stale)
        }
        // Such reviews already required the stop acknowledgement; a review
        // that had ruled activity out can no longer do so.
        Err(error) if refusal_code(&error) == Some(WORKSPACE_UNREADABLE) => {
            (!review.workspace_unchecked).then(stale)
        }
        Err(error) => Some(PublicError::from_client(&error)),
    }
}

pub(super) async fn restore_conversation_for(
    bridge: &JetBridge,
    plane_id: &str,
    conversation_id: &str,
) -> Result<RestoreOutcome, PublicError> {
    let id = parse_conversation(conversation_id)?;
    let (binding, client) = bridge.plane(Some(plane_id))?;
    let plane = binding.plane;
    let Some(trashed_at) = bridge.retention.trashed_at(plane, id)? else {
        return Ok(RestoreOutcome::Refused {
            error: PublicError::conflict(
                "retention.trash_unknown",
                "Jet Trash changed. Reload to see the current list.",
            )
            .with_plane(plane.to_string()),
        });
    };
    let key = (plane, id, trashed_at);
    let command_id = bridge.retention.restore_id(key)?;
    async {
        let result = client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?
            .restore_conversation(command_id, id)
            .await;
        match result {
            Ok(()) => {
                bridge.retention.settle_restore(&key)?;
                bridge.retention.remember(plane, id, None)?;
                bridge.planes.observe_success(plane, RETENTION_MINOR);
                Ok(RestoreOutcome::Restored {
                    conversation_id: id.to_string(),
                })
            }
            Err(error) => {
                let public = PublicError::from_client(&error);
                if !definite(&error, &public) {
                    return Err(public);
                }
                bridge.retention.settle_restore(&key)?;
                if public.code == "retention.not_trashed" {
                    bridge.retention.remember(plane, id, None)?;
                }
                Ok(RestoreOutcome::Refused {
                    error: bridge.settle(&binding, public),
                })
            }
        }
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn record(bridge: &JetBridge, id: Uuid, outcome: TrashOutcome) -> TrashOutcome {
    bridge.retention.trash_reviews.record(id, outcome.clone());
    outcome
}

/// Whether a failure proves the Plane did not apply the request: a daemon
/// refusal other than `outcome_unknown`, or a request the client never sent
/// because the Plane's protocol cannot carry it.
pub(crate) fn definite(error: &ClientError, public: &PublicError) -> bool {
    match error {
        ClientError::Remote(_) => public.category != "outcome_unknown",
        ClientError::FeatureUnavailable { .. } | ClientError::Incompatible { .. } => true,
        _ => false,
    }
}

/// The daemon's stable code when `error` is a definite refusal.
fn refusal_code(error: &ClientError) -> Option<&str> {
    match error {
        ClientError::Remote(wire) => Some(wire.code.as_str()),
        _ => None,
    }
}

fn check_preview(preview: &RetentionPreview, id: Uuid) -> Result<(), PublicError> {
    let entry_matches = preview
        .trash
        .as_ref()
        .is_none_or(|entry| entry.conversation_id == id);
    if preview.conversation_id == id && entry_matches {
        Ok(())
    } else {
        Err(PublicError::internal())
    }
}

/// `retention.trash_grace_days` for the disclosure, on the same connection.
/// A failed read only leaves the number out.
async fn grace_days(connection: &Client) -> Option<u32> {
    match plane_setting(connection, SettingKey::RetentionTrashGraceDays).await? {
        SettingValue::Count(days) => Some(days),
        SettingValue::Flag(_) | SettingValue::Text(_) => None,
    }
}

/// One Plane-scope Setting's resolved value, read for a disclosure on the
/// same connection. Any failure leaves it out.
async fn plane_setting(connection: &Client, key: SettingKey) -> Option<SettingValue> {
    connection
        .settings(SettingScope::Plane, SettingSelection::Key { key })
        .await
        .ok()?
        .settings
        .into_iter()
        .find(|setting| setting.key == key)
        .map(|setting| setting.value)
}

fn plane_label(bridge: &JetBridge, plane: PlaneId) -> String {
    bridge.planes.label(plane).unwrap_or_default()
}

fn parse_conversation(value: &str) -> Result<Uuid, PublicError> {
    Uuid::parse_str(value).map_err(|_| {
        PublicError::invalid_input("conversation.identifier_invalid", "That item is not valid.")
    })
}

fn parse_names(values: &[String]) -> Result<Vec<Uuid>, PublicError> {
    let invalid = || {
        PublicError::invalid_input(
            "retention.names_invalid",
            "Ask for 1 to 32 different tasks at a time.",
        )
    };
    if values.is_empty() || values.len() > NAMES_LIMIT {
        return Err(invalid());
    }
    let mut ids = Vec::with_capacity(values.len());
    for value in values {
        let id = Uuid::parse_str(value).map_err(|_| invalid())?;
        if ids.contains(&id) {
            return Err(invalid());
        }
        ids.push(id);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests;
