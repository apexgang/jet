use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use jet_client::TerminalEvent;
use jet_protocol::{
    ArtifactAvailability, ArtifactVerifier, ChangeArtifact, ChangeDiff, ChangeOrigin, DiffScope,
    FileRevision, FileTarget, PageCursor, ReviewComment, RunLifecycle, Sha256Digest, TerminalState,
    WorkingTree, WorkspaceTerminal, WORKSPACE_TERMINALS_MINOR,
};
use serde::Serialize;
use tauri::ipc::Channel;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{
    command_ids::{definite, settle_command},
    errors::PublicError,
    planes::{PlaneBinding, PlaneId},
    JetBridge,
};

const MAX_BOUND_FILES: usize = 4_096;
const MAX_PAGE_TOKENS: usize = 128;
const MAX_PENDING_COMMANDS: usize = 256;
const MAX_EDIT_BYTES: usize = 128 * 1024;
const MAX_COMMENT_BYTES: usize = 8_192;
const MAX_TERMINAL_INPUT_BYTES: usize = 65_536;
const TERMINAL_CREDIT: u64 = 65_536;

#[derive(Default)]
pub(crate) struct WorkPanelState {
    files: Mutex<HashMap<Uuid, BoundFile>>,
    pages: Mutex<HashMap<Uuid, BoundPage>>,
    artifacts: Mutex<HashMap<Uuid, ArtifactRead>>,
    active: Mutex<HashMap<(PlaneId, Uuid), ActiveWork>>,
    edits: Mutex<HashMap<Uuid, PendingEdit>>,
    reviews: Mutex<HashMap<(PlaneId, Uuid), PendingReview>>,
    opens: Mutex<HashMap<OpenKey, Uuid>>,
    closes: Mutex<HashMap<Uuid, Uuid>>,
    /// Terminal → (Plane it was listed on, its Workspace).
    known_terminals: Mutex<HashMap<Uuid, (PlaneBinding, Uuid)>>,
    terminal_sessions: Arc<Mutex<HashMap<Uuid, TerminalSession>>>,
    terminal_offsets: Arc<Mutex<HashMap<Uuid, u64>>>,
}

/// The webview-supplied fields of one Work panel load.
pub(crate) struct WorkPanelRequest {
    pub(crate) conversation_id: String,
    pub(crate) run_id: String,
    pub(crate) scope_kind: String,
    pub(crate) turn: Option<u32>,
    pub(crate) from_turn: Option<u32>,
    pub(crate) to_turn: Option<u32>,
}

// Every binding below keeps the Plane it was issued from. Follow-up commands
// that take only these opaque IDs execute through that binding and nowhere
// else (`plane.review_moved` when the Plane changed).
#[derive(Clone)]
struct BoundFile {
    binding: PlaneBinding,
    conversation_id: Uuid,
    run_id: Uuid,
    target: FileTarget,
    path: String,
    revision: Option<FileRevision>,
}

struct BoundPage {
    binding: PlaneBinding,
    conversation_id: Uuid,
    run_id: Uuid,
    target: Option<FileTarget>,
    cursor: PageCursor,
}

struct ArtifactRead {
    binding: PlaneBinding,
    conversation_id: Uuid,
    run_id: Uuid,
    sha256: String,
    size: u64,
    next_offset: u64,
    verifier: ArtifactVerifier,
}

#[derive(Clone, Copy)]
struct ActiveWork {
    binding: PlaneBinding,
    run_id: Uuid,
    workspace_id: Option<Uuid>,
}

/// A save whose outcome is uncertain. The retry resends this exact body:
/// the same content against the same expected revision (D4).
struct PendingEdit {
    binding: PlaneBinding,
    command_id: Uuid,
    content: String,
    revision: FileRevision,
}

struct PendingReview {
    binding: PlaneBinding,
    command_id: Uuid,
    file_id: Uuid,
    line: u32,
    comment: String,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct OpenKey {
    plane: PlaneId,
    conversation_id: Uuid,
    rows: u16,
    columns: u16,
}

struct TerminalSession {
    binding: PlaneBinding,
    generation: Uuid,
    actions: mpsc::Sender<TerminalAction>,
}

enum TerminalAction {
    Input(Vec<u8>),
    Resize { rows: u16, columns: u16 },
    Detach,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkPanelSnapshot {
    run_id: String,
    scope: &'static str,
    cursor: String,
    total_files: u32,
    files: Vec<ChangedFileView>,
    next_page: Option<String>,
    patch: String,
    patch_truncated: bool,
    artifact: ArtifactView,
    artifact_read_id: Option<String>,
    content_complete: bool,
    latest_turn: u32,
    workspace_id: Option<String>,
    terminals: Vec<TerminalView>,
    terminal_issue: Option<PublicError>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChangePageView {
    files: Vec<ChangedFileView>,
    next_page: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangedFileView {
    id: String,
    path: String,
    before_size: Option<String>,
    after_size: Option<String>,
    status: &'static str,
    origin: &'static str,
    content_available: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactView {
    availability: &'static str,
    sha256: String,
    size: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactChunkView {
    bytes: Vec<u8>,
    offset: String,
    next_offset: String,
    complete: bool,
    verified: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditableFileView {
    file_id: String,
    path: String,
    content: Option<String>,
    content_bytes: usize,
    revision: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileSavedView {
    file_id: String,
    revision: String,
    message: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewSubmittedView {
    turn_id: String,
    state: &'static str,
    message: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalView {
    id: String,
    workspace_id: String,
    state: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum TerminalUpdate {
    Attached {
        terminal_id: String,
        after: String,
    },
    Output {
        terminal_id: String,
        offset: String,
        bytes: Vec<u8>,
        next_offset: String,
    },
    Gap {
        terminal_id: String,
        first_missing_offset: String,
        missing_bytes: String,
        next_offset: String,
    },
    Resized {
        terminal_id: String,
    },
    Finished {
        terminal_id: String,
        total_bytes: String,
    },
    Failed {
        terminal_id: String,
        error: PublicError,
    },
}

pub(crate) async fn load_work_panel(
    bridge: &JetBridge,
    request: WorkPanelRequest,
    plane_id: Option<String>,
) -> Result<WorkPanelSnapshot, PublicError> {
    let conversation_id = parse_id(&request.conversation_id, "conversation.identifier_invalid")?;
    let run_id = parse_id(&request.run_id, "run.identifier_invalid")?;
    let scope = requested_scope(
        &request.scope_kind,
        request.turn,
        request.from_turn,
        request.to_turn,
    )?;
    let (binding, plane_client) = bridge.plane(plane_id.as_deref())?;
    async {
        let client = plane_client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let conversation = client
            .query(client.conversation(conversation_id))
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let run = conversation
            .runs
            .iter()
            .find(|run| run.run_id == run_id)
            .ok_or_else(|| {
                PublicError::invalid_input(
                    "run.conversation_mismatch",
                    "That Run does not belong to the selected Conversation.",
                )
            })?;
        if matches!(scope, DiffScope::Final)
            && !matches!(
                run.lifecycle,
                RunLifecycle::Completed
                    | RunLifecycle::Failed
                    | RunLifecycle::Canceled
                    | RunLifecycle::Lost
            )
        {
            return Err(PublicError::invalid_input(
                "changes.final_unavailable",
                "Final changes are available after this Run ends.",
            ));
        }
        let diff = client
            .query(client.change_diff(run_id, scope))
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let target = file_target(&conversation.conversation.working_tree, diff.workspace_id);
        let workspace_id = diff.workspace_id;
        let active = ActiveWork {
            binding,
            run_id,
            workspace_id,
        };
        bridge.work_panel.remember_active(conversation_id, active)?;
        let files = bridge
            .work_panel
            .bind_files(active, conversation_id, target, &diff)?;
        let next_page =
            bridge
                .work_panel
                .bind_page(active, conversation_id, target, diff.next_page)?;
        let artifact_read_id = bridge
            .work_panel
            .bind_artifact(active, conversation_id, &diff)?;
        let (terminals, terminal_issue) = match workspace_id {
            Some(workspace_id) => {
                match client.query(client.workspace_terminals(workspace_id)).await {
                    Ok((_, terminals)) => {
                        bridge
                            .planes
                            .observe_success(binding.plane, WORKSPACE_TERMINALS_MINOR);
                        bridge
                            .work_panel
                            .remember_terminals(binding, workspace_id, &terminals)?;
                        (terminals.into_iter().map(terminal_view).collect(), None)
                    }
                    Err(error) => (
                        Vec::new(),
                        Some(bridge.settle(&binding, PublicError::from_client(&error))),
                    ),
                }
            }
            None => (Vec::new(), None),
        };
        Ok(WorkPanelSnapshot {
            run_id: run_id.to_string(),
            scope: scope_name(&diff.scope),
            cursor: diff.cursor.to_string(),
            total_files: diff.total_files,
            files,
            next_page,
            patch: diff.patch.clone(),
            patch_truncated: diff.patch_truncated,
            artifact: artifact_view(&diff.artifact),
            artifact_read_id,
            content_complete: diff.before.content_complete && diff.after.content_complete,
            latest_turn: diff.latest_turn,
            workspace_id: workspace_id.map(|id| id.to_string()),
            terminals,
            terminal_issue,
        })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

pub(crate) async fn load_more_changes(
    bridge: &JetBridge,
    page_id: String,
) -> Result<ChangePageView, PublicError> {
    let page_id = parse_id(&page_id, "changes.page_invalid")?;
    let page = bridge.work_panel.take_page(page_id)?;
    bridge
        .work_panel
        .require_active(page.binding.plane, page.conversation_id, page.run_id)?;
    let plane_client = bridge.bound(&page.binding)?;
    async {
        let client = plane_client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let diff = client
            .query(client.next_change_diff(page.cursor))
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        if diff.run_id != page.run_id {
            return Err(PublicError::invalid_input(
                "changes.page_mismatch",
                "The changed-file page no longer belongs to this Run.",
            ));
        }
        let active = bridge
            .work_panel
            .active_work(page.binding.plane, page.conversation_id)?;
        let files =
            bridge
                .work_panel
                .bind_files(active, page.conversation_id, page.target, &diff)?;
        let next_page = bridge.work_panel.bind_page(
            active,
            page.conversation_id,
            page.target,
            diff.next_page,
        )?;
        Ok(ChangePageView { files, next_page })
    }
    .await
    .map_err(|error| bridge.settle(&page.binding, error))
}

pub(crate) async fn load_patch_chunk(
    bridge: &JetBridge,
    artifact_read_id: String,
) -> Result<ArtifactChunkView, PublicError> {
    let read_id = parse_id(&artifact_read_id, "artifact.read_invalid")?;
    let mut read = bridge.work_panel.take_artifact(read_id)?;
    bridge
        .work_panel
        .require_active(read.binding.plane, read.conversation_id, read.run_id)?;
    let binding = read.binding;
    let plane_client = bridge.bound(&binding)?;
    let client = plane_client
        .connect()
        .await
        .map_err(|error| bridge.settle(&binding, PublicError::from_client(&error)))?;
    let chunk = client
        .query(client.change_artifact(read.sha256.clone(), read.next_offset))
        .await
        .map_err(|error| bridge.settle(&binding, PublicError::from_client(&error)))?;
    if chunk.offset != read.next_offset
        || chunk.artifact.sha256 != read.sha256
        || chunk.artifact.size != read.size
        || chunk.bytes.is_empty()
    {
        return Err(PublicError::invalid_input(
            "artifact.chunk_invalid",
            "The Plane returned an invalid patch chunk.",
        ));
    }
    read.verifier.accept(&chunk.bytes).map_err(|_| {
        PublicError::invalid_input(
            "artifact.chunk_invalid",
            "The Plane returned an invalid patch chunk.",
        )
    })?;
    let offset = read.next_offset;
    read.next_offset = read
        .next_offset
        .checked_add(chunk.bytes.len() as u64)
        .ok_or_else(PublicError::internal)?;
    let bytes = chunk.bytes;
    let next_offset = read.next_offset;
    let complete = read.next_offset == read.size;
    let verified = if complete {
        read.verifier.finish().is_ok()
    } else {
        bridge.work_panel.put_artifact(read_id, read)?;
        false
    };
    if complete && !verified {
        return Err(PublicError::invalid_input(
            "artifact.hash_mismatch",
            "The downloaded patch did not match its declared identity.",
        ));
    }
    Ok(ArtifactChunkView {
        bytes,
        offset: offset.to_string(),
        next_offset: next_offset.to_string(),
        complete,
        verified,
    })
}

pub(crate) async fn load_work_file(
    bridge: &JetBridge,
    file_id: String,
) -> Result<EditableFileView, PublicError> {
    let file_id = parse_id(&file_id, "file.identifier_invalid")?;
    let bound = bridge.work_panel.bound_file(file_id)?;
    // ASVS 5.3.2 and 8.3.1: the webview selects an opaque binding created
    // from Plane data; it never supplies a native path or widens authority.
    let plane_client = bridge.bound(&bound.binding)?;
    let client = plane_client
        .connect()
        .await
        .map_err(|error| bridge.settle(&bound.binding, PublicError::from_client(&error)))?;
    let file = client
        .query(client.editable_file(bound.target, &bound.path))
        .await
        .map_err(|error| bridge.settle(&bound.binding, PublicError::from_client(&error)))?;
    if file.target != bound.target || file.path != bound.path {
        return Err(PublicError::invalid_input(
            "file.response_mismatch",
            "The Plane returned a different file than Jet requested.",
        ));
    }
    bridge
        .work_panel
        .remember_revision(file_id, file.revision.clone())?;
    Ok(EditableFileView {
        file_id: file_id.to_string(),
        path: file.path,
        content_bytes: file.content.as_ref().map_or(0, String::len),
        content: file.content,
        revision: revision_label(&file.revision),
    })
}

pub(crate) async fn save_work_file(
    bridge: &JetBridge,
    file_id: String,
    content: String,
) -> Result<FileSavedView, PublicError> {
    let file_id = parse_id(&file_id, "file.identifier_invalid")?;
    if content.len() > MAX_EDIT_BYTES {
        return Err(PublicError::invalid_input(
            "user_edit.too_large",
            "A file edit can contain at most 128 KiB of UTF-8 text.",
        ));
    }
    let bound = bridge.work_panel.bound_file(file_id)?;
    let revision = bound.revision.ok_or_else(|| {
        PublicError::invalid_input(
            "user_edit.unprepared",
            "Reload this file before saving an edit.",
        )
    })?;
    // ASVS 2.2.1, 5.1.4, and 8.3.1: size, file identity, and optimistic
    // revision are all validated at this native boundary before mutation.
    let plane_client = bridge.bound(&bound.binding)?;
    // Connect first: an edit that never reached the Plane (offline, a
    // failed login, the connect deadline) must not lock the file to its
    // content (D4). A pending edit left by an earlier uncertain save stays.
    let client = plane_client
        .connect()
        .await
        .map_err(|error| bridge.settle(&bound.binding, PublicError::from_client(&error)))?;
    let (command_id, revision) =
        bridge
            .work_panel
            .edit_command(file_id, bound.binding, &content, revision)?;
    let outcome = client
        .command(client.apply_user_edit(command_id, bound.target, &bound.path, revision, &content))
        .await;
    let saved = match outcome {
        Ok(saved) => saved,
        Err(error) => {
            // `user_edit.stale_revision` and every other refusal end this
            // edit: after a refresh the user may save anything again. Only
            // an uncertain outcome keeps it for an exact retry (D4).
            if definite(&error) {
                bridge.work_panel.drop_edit(file_id, command_id)?;
            }
            return Err(bridge.settle(&bound.binding, PublicError::from_client(&error)));
        }
    };
    bridge
        .work_panel
        .finish_edit(file_id, command_id, saved.clone())?;
    Ok(FileSavedView {
        file_id: file_id.to_string(),
        revision: revision_label(&saved),
        message: "Saved through the selected Workspace.",
    })
}

pub(crate) async fn submit_file_review(
    bridge: &JetBridge,
    file_id: String,
    line: u32,
    comment: String,
) -> Result<ReviewSubmittedView, PublicError> {
    let file_id = parse_id(&file_id, "file.identifier_invalid")?;
    if line == 0 || comment.trim().is_empty() || comment.len() > MAX_COMMENT_BYTES {
        return Err(PublicError::invalid_input(
            "review.comment_invalid",
            "Enter a line and a comment between 1 and 8,192 UTF-8 bytes.",
        ));
    }
    let bound = bridge.work_panel.bound_file(file_id)?;
    let binding = bound.binding;
    let plane_client = bridge.bound(&binding)?;
    // As for edits: a review that never reached the Plane keeps nothing.
    let client = plane_client
        .connect()
        .await
        .map_err(|error| bridge.settle(&binding, PublicError::from_client(&error)))?;
    let command_id = bridge.work_panel.review_command(
        binding,
        bound.conversation_id,
        file_id,
        line,
        &comment,
    )?;
    let outcome = client
        .command(client.submit_review(
            command_id,
            bound.conversation_id,
            vec![ReviewComment {
                path: bound.path,
                line,
                comment,
            }],
        ))
        .await;
    if outcome
        .as_ref()
        .map_or_else(|error| definite(error), |_| true)
    {
        bridge
            .work_panel
            .finish_review(binding.plane, bound.conversation_id)?;
    }
    let turn =
        outcome.map_err(|error| bridge.settle(&binding, PublicError::from_client(&error)))?;
    Ok(ReviewSubmittedView {
        turn_id: turn.turn_id.to_string(),
        state: turn_state(turn.state),
        message: "The review comment was added as one queued Turn.",
    })
}

pub(crate) async fn open_workspace_terminal(
    bridge: &JetBridge,
    conversation_id: String,
    rows: u16,
    columns: u16,
    plane_id: Option<String>,
) -> Result<TerminalView, PublicError> {
    let conversation_id = parse_id(&conversation_id, "conversation.identifier_invalid")?;
    validate_dimensions(rows, columns)?;
    let (requested, _) = bridge.plane(plane_id.as_deref())?;
    let active = bridge
        .work_panel
        .active_work(requested.plane, conversation_id)
        .map_err(|error| bridge.settle(&requested, error))?;
    // The Workspace comes from the Work panel load, so the terminal opens on
    // the Plane that load was bound to.
    let binding = active.binding;
    let workspace_id = active.workspace_id.ok_or_else(|| {
        PublicError::invalid_input(
            "terminal.workspace_required",
            "Terminals are available only for a managed Workspace.",
        )
    })?;
    let key = OpenKey {
        plane: binding.plane,
        conversation_id,
        rows,
        columns,
    };
    let plane_client = bridge.bound(&binding)?;
    let command_id = bridge.work_panel.open_command(key)?;
    let client = plane_client
        .connect()
        .await
        .map_err(|error| bridge.settle(&binding, PublicError::from_client(&error)))?;
    let outcome = client
        .command(client.open_terminal(command_id, workspace_id, rows, columns))
        .await;
    settle_command(&bridge.work_panel.opens, &key, &outcome)?;
    let terminal =
        outcome.map_err(|error| bridge.settle(&binding, PublicError::from_client(&error)))?;
    if terminal.workspace_id != workspace_id {
        return Err(PublicError::invalid_input(
            "terminal.workspace_mismatch",
            "The Plane returned a terminal from another Workspace.",
        ));
    }
    bridge.work_panel.finish_open(key, binding, &terminal)?;
    Ok(terminal_view(terminal))
}

pub(crate) async fn close_workspace_terminal(
    bridge: &JetBridge,
    terminal_id: String,
) -> Result<TerminalView, PublicError> {
    let terminal_id = parse_id(&terminal_id, "terminal.identifier_invalid")?;
    // ASVS 5.3.2 and 8.3.1: only a terminal returned for the active managed
    // Workspace can be attached; the webview cannot launch a host process.
    let binding = bridge.work_panel.require_terminal(terminal_id)?;
    let plane_client = bridge.bound(&binding)?;
    bridge.work_panel.detach_terminal(terminal_id)?;
    let command_id = bridge.work_panel.close_command(terminal_id)?;
    let client = plane_client
        .connect()
        .await
        .map_err(|error| bridge.settle(&binding, PublicError::from_client(&error)))?;
    let outcome = client
        .command(client.close_terminal(command_id, terminal_id))
        .await;
    settle_command(&bridge.work_panel.closes, &terminal_id, &outcome)?;
    let terminal =
        outcome.map_err(|error| bridge.settle(&binding, PublicError::from_client(&error)))?;
    if terminal.terminal_id != terminal_id {
        return Err(PublicError::invalid_input(
            "terminal.response_mismatch",
            "The Plane returned a different terminal than Jet closed.",
        ));
    }
    bridge
        .work_panel
        .finish_close(terminal_id, binding, &terminal)?;
    Ok(terminal_view(terminal))
}

pub(crate) async fn attach_workspace_terminal(
    bridge: &JetBridge,
    terminal_id: String,
    on_update: Channel<TerminalUpdate>,
) -> Result<(), PublicError> {
    let terminal_id = parse_id(&terminal_id, "terminal.identifier_invalid")?;
    let binding = bridge.work_panel.require_terminal(terminal_id)?;
    let after = bridge.work_panel.terminal_offset(terminal_id)?;
    let plane_client = bridge.bound(&binding)?;
    let client = plane_client
        .connect()
        .await
        .map_err(|error| bridge.settle(&binding, PublicError::from_client(&error)))?;
    let (actions, mut receive_actions) = mpsc::channel(32);
    let generation = Uuid::new_v4();
    bridge
        .work_panel
        .start_terminal_session(terminal_id, binding, generation, actions)?;
    let plane = binding.plane.to_string();
    let sessions = Arc::clone(&bridge.work_panel.terminal_sessions);
    let offsets = Arc::clone(&bridge.work_panel.terminal_offsets);
    tokio::spawn(async move {
        let result = async {
            // Only the attach reply is bounded; the attachment is a stream.
            let mut attachment = client
                .query(client.attach_terminal(terminal_id, after, TERMINAL_CREDIT))
                .await
                .map_err(|error| *error)?;
            let _ = on_update.send(TerminalUpdate::Attached {
                terminal_id: terminal_id.to_string(),
                after: after.to_string(),
            });
            loop {
                tokio::select! {
                    event = attachment.receive() => {
                        match event? {
                            TerminalEvent::Output { offset, bytes } => {
                                let next_offset = offset.saturating_add(bytes.len() as u64);
                                if let Ok(mut values) = offsets.lock() {
                                    values.insert(terminal_id, next_offset);
                                }
                                if on_update.send(TerminalUpdate::Output {
                                    terminal_id: terminal_id.to_string(),
                                    offset: offset.to_string(),
                                    bytes: bytes.clone(),
                                    next_offset: next_offset.to_string(),
                                }).is_err() {
                                    break;
                                }
                                attachment.credit(bytes.len() as u64).await?;
                            }
                            TerminalEvent::Gap { first_missing_offset, missing_bytes } => {
                                let next_offset = first_missing_offset.saturating_add(missing_bytes);
                                if let Ok(mut values) = offsets.lock() {
                                    values.insert(terminal_id, next_offset);
                                }
                                if on_update.send(TerminalUpdate::Gap {
                                    terminal_id: terminal_id.to_string(),
                                    first_missing_offset: first_missing_offset.to_string(),
                                    missing_bytes: missing_bytes.to_string(),
                                    next_offset: next_offset.to_string(),
                                }).is_err() {
                                    break;
                                }
                            }
                            TerminalEvent::Resized => {
                                let _ = on_update.send(TerminalUpdate::Resized {
                                    terminal_id: terminal_id.to_string(),
                                });
                            }
                            TerminalEvent::Finished { total_bytes } => {
                                if let Ok(mut values) = offsets.lock() {
                                    values.insert(terminal_id, total_bytes);
                                }
                                let _ = on_update.send(TerminalUpdate::Finished {
                                    terminal_id: terminal_id.to_string(),
                                    total_bytes: total_bytes.to_string(),
                                });
                                break;
                            }
                        }
                    }
                    action = receive_actions.recv() => {
                        match action {
                            Some(TerminalAction::Input(bytes)) => attachment.input(bytes).await?,
                            Some(TerminalAction::Resize { rows, columns }) => {
                                attachment.resize(rows, columns).await?;
                            }
                            Some(TerminalAction::Detach) | None => break,
                        }
                    }
                }
            }
            Ok::<(), jet_client::ClientError>(())
        }
        .await;
        if let Err(error) = result {
            let _ = on_update.send(TerminalUpdate::Failed {
                terminal_id: terminal_id.to_string(),
                error: PublicError::from_client(&error).with_plane(plane),
            });
        }
        if let Ok(mut sessions) = sessions.lock() {
            if sessions
                .get(&terminal_id)
                .is_some_and(|session| session.generation == generation)
            {
                sessions.remove(&terminal_id);
            }
        }
    });
    Ok(())
}

pub(crate) async fn send_terminal_input(
    bridge: &JetBridge,
    terminal_id: String,
    input: String,
) -> Result<(), PublicError> {
    let terminal_id = parse_id(&terminal_id, "terminal.identifier_invalid")?;
    if input.is_empty() || input.len() > MAX_TERMINAL_INPUT_BYTES {
        return Err(PublicError::invalid_input(
            "terminal.input_invalid",
            "Terminal input must contain 1 to 65,536 UTF-8 bytes.",
        ));
    }
    let binding = bridge.work_panel.session_binding(terminal_id)?;
    bridge.bound(&binding)?;
    bridge
        .work_panel
        .terminal_action(terminal_id, TerminalAction::Input(input.into_bytes()))
        .await
}

pub(crate) async fn resize_workspace_terminal(
    bridge: &JetBridge,
    terminal_id: String,
    rows: u16,
    columns: u16,
) -> Result<(), PublicError> {
    let terminal_id = parse_id(&terminal_id, "terminal.identifier_invalid")?;
    validate_dimensions(rows, columns)?;
    let binding = bridge.work_panel.session_binding(terminal_id)?;
    bridge.bound(&binding)?;
    bridge
        .work_panel
        .terminal_action(terminal_id, TerminalAction::Resize { rows, columns })
        .await
}

pub(crate) fn detach_workspace_terminal(
    bridge: &JetBridge,
    terminal_id: String,
) -> Result<(), PublicError> {
    let terminal_id = parse_id(&terminal_id, "terminal.identifier_invalid")?;
    bridge.work_panel.detach_terminal(terminal_id)
}

impl WorkPanelState {
    fn remember_active(
        &self,
        conversation_id: Uuid,
        active: ActiveWork,
    ) -> Result<(), PublicError> {
        let mut values = self.active.lock().map_err(|_| PublicError::internal())?;
        values.clear();
        values.insert((active.binding.plane, conversation_id), active);
        drop(values);
        self.files
            .lock()
            .map_err(|_| PublicError::internal())?
            .retain(|_, file| {
                file.binding == active.binding
                    && file.conversation_id == conversation_id
                    && file.run_id == active.run_id
            });
        self.pages
            .lock()
            .map_err(|_| PublicError::internal())?
            .clear();
        self.artifacts
            .lock()
            .map_err(|_| PublicError::internal())?
            .clear();
        Ok(())
    }

    fn active_work(
        &self,
        plane: PlaneId,
        conversation_id: Uuid,
    ) -> Result<ActiveWork, PublicError> {
        self.active
            .lock()
            .map_err(|_| PublicError::internal())?
            .get(&(plane, conversation_id))
            .copied()
            .ok_or_else(|| {
                PublicError::invalid_input(
                    "work_panel.not_loaded",
                    "Reload the Work panel before using this action.",
                )
            })
    }

    fn require_active(
        &self,
        plane: PlaneId,
        conversation_id: Uuid,
        run_id: Uuid,
    ) -> Result<(), PublicError> {
        if self.active_work(plane, conversation_id)?.run_id == run_id {
            Ok(())
        } else {
            Err(PublicError::invalid_input(
                "work_panel.selection_expired",
                "Reload the Work panel before using this selection.",
            ))
        }
    }

    fn bind_files(
        &self,
        active: ActiveWork,
        conversation_id: Uuid,
        target: Option<FileTarget>,
        diff: &ChangeDiff,
    ) -> Result<Vec<ChangedFileView>, PublicError> {
        let run_id = active.run_id;
        let mut bindings = self.files.lock().map_err(|_| PublicError::internal())?;
        let mut views = Vec::with_capacity(diff.files.len());
        for file in &diff.files {
            let id = bindings
                .iter()
                .find_map(|(id, existing)| {
                    (existing.binding == active.binding
                        && existing.run_id == run_id
                        && existing.path == file.path)
                        .then_some(*id)
                })
                .unwrap_or_else(Uuid::new_v4);
            if let Some(target) = target {
                if bindings.len() >= MAX_BOUND_FILES && !bindings.contains_key(&id) {
                    return Err(PublicError::invalid_input(
                        "changes.too_many_files",
                        "This change set contains more files than the desktop client can inspect at once.",
                    ));
                }
                bindings.insert(
                    id,
                    BoundFile {
                        binding: active.binding,
                        conversation_id,
                        run_id,
                        target,
                        path: file.path.clone(),
                        revision: None,
                    },
                );
            }
            views.push(ChangedFileView {
                id: id.to_string(),
                path: file.path.clone(),
                before_size: file.before_size.map(|value| value.to_string()),
                after_size: file.after_size.map(|value| value.to_string()),
                status: file_status(file.before_object.as_deref(), file.after_object.as_deref()),
                origin: origin_name(&file.origin),
                content_available: target.is_some()
                    && file.before_object.is_some()
                    && file.after_object.is_some(),
            });
        }
        Ok(views)
    }

    fn bind_page(
        &self,
        active: ActiveWork,
        conversation_id: Uuid,
        target: Option<FileTarget>,
        cursor: Option<PageCursor>,
    ) -> Result<Option<String>, PublicError> {
        let Some(cursor) = cursor else {
            return Ok(None);
        };
        let mut pages = self.pages.lock().map_err(|_| PublicError::internal())?;
        if pages.len() >= MAX_PAGE_TOKENS {
            pages.clear();
        }
        let id = Uuid::new_v4();
        pages.insert(
            id,
            BoundPage {
                binding: active.binding,
                conversation_id,
                run_id: active.run_id,
                target,
                cursor,
            },
        );
        Ok(Some(id.to_string()))
    }

    fn take_page(&self, id: Uuid) -> Result<BoundPage, PublicError> {
        self.pages
            .lock()
            .map_err(|_| PublicError::internal())?
            .remove(&id)
            .ok_or_else(|| {
                PublicError::invalid_input(
                    "changes.page_expired",
                    "Reload Changes before requesting another page.",
                )
            })
    }

    fn bind_artifact(
        &self,
        active: ActiveWork,
        conversation_id: Uuid,
        diff: &ChangeDiff,
    ) -> Result<Option<String>, PublicError> {
        if !diff.patch_truncated || diff.artifact.availability != ArtifactAvailability::Stored {
            return Ok(None);
        }
        let digest = Sha256Digest::parse(&diff.artifact.sha256).map_err(|_| {
            PublicError::invalid_input(
                "artifact.identity_invalid",
                "The Plane returned an invalid patch identity.",
            )
        })?;
        let mut verifier = ArtifactVerifier::new(diff.artifact.size, digest);
        verifier.accept(diff.patch.as_bytes()).map_err(|_| {
            PublicError::invalid_input(
                "artifact.preview_invalid",
                "The Plane returned an invalid patch preview.",
            )
        })?;
        let id = Uuid::new_v4();
        self.artifacts
            .lock()
            .map_err(|_| PublicError::internal())?
            .insert(
                id,
                ArtifactRead {
                    binding: active.binding,
                    conversation_id,
                    run_id: active.run_id,
                    sha256: diff.artifact.sha256.clone(),
                    size: diff.artifact.size,
                    next_offset: diff.patch.len() as u64,
                    verifier,
                },
            );
        Ok(Some(id.to_string()))
    }

    fn take_artifact(&self, id: Uuid) -> Result<ArtifactRead, PublicError> {
        self.artifacts
            .lock()
            .map_err(|_| PublicError::internal())?
            .remove(&id)
            .ok_or_else(|| {
                PublicError::invalid_input(
                    "artifact.read_expired",
                    "Reload Changes before loading more patch content.",
                )
            })
    }

    fn put_artifact(&self, id: Uuid, read: ArtifactRead) -> Result<(), PublicError> {
        self.artifacts
            .lock()
            .map_err(|_| PublicError::internal())?
            .insert(id, read);
        Ok(())
    }

    fn bound_file(&self, id: Uuid) -> Result<BoundFile, PublicError> {
        let file = self
            .files
            .lock()
            .map_err(|_| PublicError::internal())?
            .get(&id)
            .cloned()
            .ok_or_else(|| {
                PublicError::invalid_input(
                    "file.selection_expired",
                    "Reload Changes before opening this file.",
                )
            })?;
        self.require_active(file.binding.plane, file.conversation_id, file.run_id)?;
        Ok(file)
    }

    /// Records the revision a load observed. A pending uncertain edit was
    /// prepared against an older revision: it has either been applied or
    /// can no longer apply (the Plane checks the expected revision), so it
    /// no longer blocks a new save (D4).
    fn remember_revision(&self, id: Uuid, revision: FileRevision) -> Result<(), PublicError> {
        {
            let mut edits = self.edits.lock().map_err(|_| PublicError::internal())?;
            if edits
                .get(&id)
                .is_some_and(|pending| pending.revision != revision)
            {
                edits.remove(&id);
            }
        }
        let mut files = self.files.lock().map_err(|_| PublicError::internal())?;
        let file = files.get_mut(&id).ok_or_else(|| {
            PublicError::invalid_input(
                "file.selection_expired",
                "Reload Changes before opening this file.",
            )
        })?;
        file.revision = Some(revision);
        Ok(())
    }

    /// The command ID and expected revision for saving `content`: those of
    /// the pending uncertain edit when this is its exact retry, or new ones.
    fn edit_command(
        &self,
        file_id: Uuid,
        binding: PlaneBinding,
        content: &str,
        revision: FileRevision,
    ) -> Result<(Uuid, FileRevision), PublicError> {
        let mut edits = self.edits.lock().map_err(|_| PublicError::internal())?;
        if let Some(pending) = edits.get(&file_id) {
            if pending.binding == binding && pending.content == content {
                return Ok((pending.command_id, pending.revision.clone()));
            }
            return Err(PublicError::invalid_input(
                "user_edit.retry_mismatch",
                "Retry the unchanged edit before preparing another save.",
            ));
        }
        if edits.len() >= MAX_PENDING_COMMANDS {
            return Err(too_many_pending());
        }
        let command_id = Uuid::new_v4();
        edits.insert(
            file_id,
            PendingEdit {
                binding,
                command_id,
                content: content.into(),
                revision: revision.clone(),
            },
        );
        Ok((command_id, revision))
    }

    fn finish_edit(
        &self,
        file_id: Uuid,
        command_id: Uuid,
        revision: FileRevision,
    ) -> Result<(), PublicError> {
        self.drop_edit(file_id, command_id)?;
        self.remember_revision(file_id, revision)
    }

    /// The edit sent as `command_id` has a known outcome: the next save is
    /// a new request. A newer pending edit that replaced it on this file
    /// (after a reload showed a newer revision) keeps its own ID.
    fn drop_edit(&self, file_id: Uuid, command_id: Uuid) -> Result<(), PublicError> {
        let mut edits = self.edits.lock().map_err(|_| PublicError::internal())?;
        if edits
            .get(&file_id)
            .is_some_and(|pending| pending.command_id == command_id)
        {
            edits.remove(&file_id);
        }
        Ok(())
    }

    fn review_command(
        &self,
        binding: PlaneBinding,
        conversation_id: Uuid,
        file_id: Uuid,
        line: u32,
        comment: &str,
    ) -> Result<Uuid, PublicError> {
        let mut reviews = self.reviews.lock().map_err(|_| PublicError::internal())?;
        if let Some(pending) = reviews.get(&(binding.plane, conversation_id)) {
            if pending.binding == binding
                && pending.file_id == file_id
                && pending.line == line
                && pending.comment == comment
            {
                return Ok(pending.command_id);
            }
            return Err(PublicError::invalid_input(
                "review.retry_mismatch",
                "Retry the unchanged review before preparing another one.",
            ));
        }
        if reviews.len() >= MAX_PENDING_COMMANDS {
            return Err(too_many_pending());
        }
        let command_id = Uuid::new_v4();
        reviews.insert(
            (binding.plane, conversation_id),
            PendingReview {
                binding,
                command_id,
                file_id,
                line,
                comment: comment.into(),
            },
        );
        Ok(command_id)
    }

    fn finish_review(&self, plane: PlaneId, conversation_id: Uuid) -> Result<(), PublicError> {
        self.reviews
            .lock()
            .map_err(|_| PublicError::internal())?
            .remove(&(plane, conversation_id));
        Ok(())
    }

    fn open_command(&self, key: OpenKey) -> Result<Uuid, PublicError> {
        command_id(&self.opens, key)
    }

    fn finish_open(
        &self,
        key: OpenKey,
        binding: PlaneBinding,
        terminal: &WorkspaceTerminal,
    ) -> Result<(), PublicError> {
        self.opens
            .lock()
            .map_err(|_| PublicError::internal())?
            .remove(&key);
        self.known_terminals
            .lock()
            .map_err(|_| PublicError::internal())?
            .insert(terminal.terminal_id, (binding, terminal.workspace_id));
        Ok(())
    }

    fn close_command(&self, terminal_id: Uuid) -> Result<Uuid, PublicError> {
        command_id(&self.closes, terminal_id)
    }

    fn finish_close(
        &self,
        terminal_id: Uuid,
        binding: PlaneBinding,
        terminal: &WorkspaceTerminal,
    ) -> Result<(), PublicError> {
        self.closes
            .lock()
            .map_err(|_| PublicError::internal())?
            .remove(&terminal_id);
        self.known_terminals
            .lock()
            .map_err(|_| PublicError::internal())?
            .insert(terminal.terminal_id, (binding, terminal.workspace_id));
        Ok(())
    }

    fn remember_terminals(
        &self,
        binding: PlaneBinding,
        workspace_id: Uuid,
        terminals: &[WorkspaceTerminal],
    ) -> Result<(), PublicError> {
        let mut known = self
            .known_terminals
            .lock()
            .map_err(|_| PublicError::internal())?;
        known.clear();
        for terminal in terminals {
            if terminal.workspace_id != workspace_id {
                return Err(PublicError::invalid_input(
                    "terminal.workspace_mismatch",
                    "The Plane returned a terminal from another Workspace.",
                ));
            }
            known.insert(terminal.terminal_id, (binding, terminal.workspace_id));
        }
        Ok(())
    }

    /// The Plane binding of a terminal listed for the active managed
    /// Workspace, or `terminal.selection_expired`.
    fn require_terminal(&self, terminal_id: Uuid) -> Result<PlaneBinding, PublicError> {
        let known = self
            .known_terminals
            .lock()
            .map_err(|_| PublicError::internal())?
            .get(&terminal_id)
            .copied();
        let active = self
            .active
            .lock()
            .map_err(|_| PublicError::internal())?
            .values()
            .next()
            .copied();
        match (known, active) {
            (Some((binding, workspace_id)), Some(active))
                if binding == active.binding && Some(workspace_id) == active.workspace_id =>
            {
                Ok(binding)
            }
            _ => Err(PublicError::invalid_input(
                "terminal.selection_expired",
                "Reload the Work panel before using this terminal.",
            )),
        }
    }

    fn terminal_offset(&self, terminal_id: Uuid) -> Result<u64, PublicError> {
        Ok(*self
            .terminal_offsets
            .lock()
            .map_err(|_| PublicError::internal())?
            .get(&terminal_id)
            .unwrap_or(&0))
    }

    fn start_terminal_session(
        &self,
        terminal_id: Uuid,
        binding: PlaneBinding,
        generation: Uuid,
        actions: mpsc::Sender<TerminalAction>,
    ) -> Result<(), PublicError> {
        let mut sessions = self
            .terminal_sessions
            .lock()
            .map_err(|_| PublicError::internal())?;
        if sessions.contains_key(&terminal_id) {
            return Err(PublicError::invalid_input(
                "terminal.already_attached",
                "This terminal is already attached in the Work panel.",
            ));
        }
        sessions.insert(
            terminal_id,
            TerminalSession {
                binding,
                generation,
                actions,
            },
        );
        Ok(())
    }

    fn session_binding(&self, terminal_id: Uuid) -> Result<PlaneBinding, PublicError> {
        self.terminal_sessions
            .lock()
            .map_err(|_| PublicError::internal())?
            .get(&terminal_id)
            .map(|session| session.binding)
            .ok_or_else(|| {
                PublicError::invalid_input(
                    "terminal.not_attached",
                    "Attach this terminal before sending input.",
                )
            })
    }

    async fn terminal_action(
        &self,
        terminal_id: Uuid,
        action: TerminalAction,
    ) -> Result<(), PublicError> {
        let sender = self
            .terminal_sessions
            .lock()
            .map_err(|_| PublicError::internal())?
            .get(&terminal_id)
            .map(|session| session.actions.clone())
            .ok_or_else(|| {
                PublicError::invalid_input(
                    "terminal.not_attached",
                    "Attach this terminal before sending input.",
                )
            })?;
        sender.send(action).await.map_err(|_| {
            PublicError::invalid_input(
                "terminal.not_attached",
                "Attach this terminal before sending input.",
            )
        })
    }

    fn detach_terminal(&self, terminal_id: Uuid) -> Result<(), PublicError> {
        if let Some(session) = self
            .terminal_sessions
            .lock()
            .map_err(|_| PublicError::internal())?
            .remove(&terminal_id)
        {
            let _ = session.actions.try_send(TerminalAction::Detach);
        }
        Ok(())
    }
}

fn file_target(
    working_tree: &Option<WorkingTree>,
    workspace_id: Option<Uuid>,
) -> Option<FileTarget> {
    if let Some(workspace_id) = workspace_id {
        return Some(FileTarget::Workspace { workspace_id });
    }
    match working_tree {
        Some(WorkingTree::LocalCheckout { project_id }) => Some(FileTarget::Project {
            project_id: *project_id,
        }),
        Some(WorkingTree::Workspace { .. } | WorkingTree::NoProject) | None => None,
    }
}

fn file_status(before: Option<&str>, after: Option<&str>) -> &'static str {
    match (before.map(all_zero), after.map(all_zero)) {
        (Some(true), Some(false)) => "added",
        (Some(false), Some(true)) => "deleted",
        _ => "modified",
    }
}

fn all_zero(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte == b'0')
}

fn origin_name(origin: &ChangeOrigin) -> &'static str {
    match origin {
        ChangeOrigin::UserEdit { .. } => "user edit",
        ChangeOrigin::WorkspaceTerminal { .. } => "terminal",
        ChangeOrigin::Harness { .. } => "agent",
        ChangeOrigin::Mixed => "mixed",
        ChangeOrigin::ExternalOrUnknown => "external or unknown",
    }
}

fn scope_name(scope: &DiffScope) -> &'static str {
    match scope {
        DiffScope::Current => "Current",
        DiffScope::Final => "Final",
        DiffScope::Historical { .. } => "Historical",
        DiffScope::Turn { .. } => "Turn",
    }
}

fn availability_name(value: ArtifactAvailability) -> &'static str {
    match value {
        ArtifactAvailability::Stored => "stored",
        ArtifactAvailability::DiskPressure => "disk_pressure",
        ArtifactAvailability::RunBudgetExceeded => "run_budget_exceeded",
        ArtifactAvailability::ArtifactSizeExceeded => "artifact_size_exceeded",
    }
}

fn artifact_view(artifact: &ChangeArtifact) -> ArtifactView {
    ArtifactView {
        availability: availability_name(artifact.availability),
        sha256: artifact.sha256.clone(),
        size: artifact.size.to_string(),
    }
}

fn terminal_view(terminal: WorkspaceTerminal) -> TerminalView {
    TerminalView {
        id: terminal.terminal_id.to_string(),
        workspace_id: terminal.workspace_id.to_string(),
        state: terminal_state(terminal.state),
    }
}

fn terminal_state(state: TerminalState) -> &'static str {
    match state {
        TerminalState::Opening => "opening",
        TerminalState::Open => "open",
        TerminalState::Closing => "closing",
        TerminalState::Closed => "closed",
        TerminalState::Unavailable => "unavailable",
    }
}

fn turn_state(state: jet_protocol::TurnState) -> &'static str {
    match state {
        jet_protocol::TurnState::Queued => "queued",
        jet_protocol::TurnState::Active => "active",
        jet_protocol::TurnState::Completed => "completed",
        jet_protocol::TurnState::Superseded => "superseded",
        jet_protocol::TurnState::Canceled => "canceled",
        jet_protocol::TurnState::Withdrawn => "withdrawn",
        jet_protocol::TurnState::Failed => "failed",
        jet_protocol::TurnState::OutcomeUnknown => "outcome_unknown",
    }
}

fn revision_label(revision: &FileRevision) -> String {
    format!("{} · {}", revision.mode, revision.object)
}

fn validate_dimensions(rows: u16, columns: u16) -> Result<(), PublicError> {
    if !(1..=1000).contains(&rows) || !(1..=1000).contains(&columns) {
        return Err(PublicError::invalid_input(
            "terminal.dimensions_invalid",
            "Terminal rows and columns must be between 1 and 1,000.",
        ));
    }
    Ok(())
}

fn requested_scope(
    kind: &str,
    turn: Option<u32>,
    from_turn: Option<u32>,
    to_turn: Option<u32>,
) -> Result<DiffScope, PublicError> {
    match (kind, turn, from_turn, to_turn) {
        ("current", None, None, None) => Ok(DiffScope::Current),
        ("final", None, None, None) => Ok(DiffScope::Final),
        ("turn", Some(turn), None, None) if turn > 0 => Ok(DiffScope::Turn { turn }),
        ("historical", None, Some(from_turn), Some(to_turn))
            if from_turn <= to_turn && to_turn > 0 =>
        {
            Ok(DiffScope::Historical { from_turn, to_turn })
        }
        _ => Err(PublicError::invalid_input(
            "changes.scope_invalid",
            "Choose a valid retained checkpoint range.",
        )),
    }
}

fn parse_id(value: &str, code: &'static str) -> Result<Uuid, PublicError> {
    Uuid::parse_str(value).map_err(|_| PublicError::invalid_input(code, "That item is not valid."))
}

fn command_id<K>(commands: &Mutex<HashMap<K, Uuid>>, key: K) -> Result<Uuid, PublicError>
where
    K: std::hash::Hash + Eq,
{
    let mut commands = commands.lock().map_err(|_| PublicError::internal())?;
    if let Some(command_id) = commands.get(&key) {
        return Ok(*command_id);
    }
    if commands.len() >= MAX_PENDING_COMMANDS {
        return Err(too_many_pending());
    }
    Ok(*commands.entry(key).or_insert_with(Uuid::new_v4))
}

fn too_many_pending() -> PublicError {
    PublicError::invalid_input(
        "client.too_many_pending_commands",
        "Finish or retry an earlier task before starting another one.",
    )
}

#[cfg(test)]
mod tests {
    use super::{
        all_zero, load_work_file, requested_scope, revision_label, save_work_file,
        submit_file_review, validate_dimensions, ActiveWork, BoundFile, WorkPanelState,
    };
    use crate::jet::{
        fake_plane::{answer, command_id, complete, exchange, next, refuse, remote_plane, wire},
        planes::{PlaneBinding, PlaneId},
    };
    use jet_protocol::{
        ClientMessage, CommandRequest, CommandResponse, DiffScope, EditableFile, ErrorCategory,
        FileRevision, FileTarget, QueryRequest, QueryResponse,
    };
    use uuid::Uuid;

    const LOCAL: PlaneBinding = PlaneBinding {
        plane: PlaneId::Local,
        identity: None,
    };
    const WORKSPACE: Uuid = Uuid::from_u128(0x3e);
    const TARGET: FileTarget = FileTarget::Workspace {
        workspace_id: WORKSPACE,
    };

    fn revision(object: &str) -> FileRevision {
        FileRevision {
            object: object.repeat(40),
            mode: "100644".into(),
        }
    }

    /// Makes `binding`'s Work panel active and binds one editable file.
    fn bind_file(state: &WorkPanelState, binding: PlaneBinding, loaded: FileRevision) -> Uuid {
        let conversation = Uuid::from_u128(4);
        let run = Uuid::from_u128(5);
        state
            .remember_active(
                conversation,
                ActiveWork {
                    binding,
                    run_id: run,
                    workspace_id: Some(WORKSPACE),
                },
            )
            .unwrap();
        let id = Uuid::from_u128(6);
        state.files.lock().unwrap().insert(
            id,
            BoundFile {
                binding,
                conversation_id: conversation,
                run_id: run,
                target: TARGET,
                path: "src/lib.rs".into(),
                revision: Some(loaded),
            },
        );
        id
    }

    /// D4: an uncertain save is retried with its exact body, its content and
    /// its expected revision, and blocks other content until a reload shows
    /// the file moved on (the old edit then either applied or can't).
    #[test]
    fn an_uncertain_edit_retries_its_exact_body_until_a_reload_moves_the_file() {
        let state = WorkPanelState::default();
        let file = bind_file(&state, LOCAL, revision("a"));
        let (first, sent) = state
            .edit_command(file, LOCAL, "mine", revision("a"))
            .unwrap();
        assert_eq!(sent, revision("a"));
        assert_eq!(
            state
                .edit_command(file, LOCAL, "other", revision("a"))
                .unwrap_err()
                .code,
            "user_edit.retry_mismatch"
        );
        // Reloading the same revision keeps the uncertain edit.
        state.remember_revision(file, revision("a")).unwrap();
        assert_eq!(
            state
                .edit_command(file, LOCAL, "mine", revision("a"))
                .unwrap(),
            (first, revision("a"))
        );
        // A newer revision ends it: any content may be saved against it.
        state.remember_revision(file, revision("b")).unwrap();
        let (second, sent) = state
            .edit_command(file, LOCAL, "other", revision("b"))
            .unwrap();
        assert_ne!(second, first);
        assert_eq!(sent, revision("b"));

        // The first save's late refusal settles only its own ID: the newer
        // uncertain edit stays pending for its exact retry.
        state.drop_edit(file, first).unwrap();
        assert_eq!(
            state
                .edit_command(file, LOCAL, "other", revision("b"))
                .unwrap(),
            (second, revision("b"))
        );
        state.drop_edit(file, second).unwrap();
        assert_ne!(
            state
                .edit_command(file, LOCAL, "other", revision("b"))
                .unwrap()
                .0,
            second
        );
    }

    /// D4: any definite refusal ends the pending edit, even without a
    /// reload: here disk pressure refuses the save, and the next save with
    /// other content is a new request instead of `user_edit.retry_mismatch`.
    #[tokio::test]
    async fn a_refused_edit_no_longer_locks_its_content() {
        let fake = remote_plane();
        let binding = PlaneBinding {
            plane: fake.plane,
            identity: None,
        };
        let file = bind_file(&fake.bridge().work_panel, binding, revision("a"));
        let serve = |refusal: Option<&'static str>| {
            let fake = &fake;
            async move {
                let (mut reader, mut writer) = fake.accept().await;
                let (stream, message) = next(&mut reader).await;
                match refusal {
                    Some(code) => {
                        refuse(
                            &mut writer,
                            stream,
                            &message,
                            wire(ErrorCategory::Unavailable, code),
                        )
                        .await;
                    }
                    None => {
                        complete(
                            &mut writer,
                            stream,
                            &message,
                            CommandResponse::UserEditApplied {
                                target: TARGET,
                                path: "src/lib.rs".into(),
                                revision: revision("c"),
                            },
                        )
                        .await;
                    }
                }
                command_id(&message)
            }
        };
        let (refused, first) = exchange(
            save_work_file(fake.bridge(), file.to_string(), "mine".into()),
            serve(Some("storage.disk_pressure")),
        )
        .await;
        assert_eq!(refused.unwrap_err().code, "storage.disk_pressure");
        let (saved, second) = exchange(
            save_work_file(fake.bridge(), file.to_string(), "mine, shorter".into()),
            serve(None),
        )
        .await;
        assert!(saved.is_ok(), "{saved:?}");
        assert!(second.is_some());
        assert_ne!(second, first);
    }

    /// D4: `user_edit.stale_revision` ends the pending edit. After the
    /// refresh the user saves new content under a new Command ID against the
    /// new revision, instead of being stuck or forced to resend the stale
    /// edit over the concurrent change.
    #[tokio::test]
    async fn a_stale_revision_refusal_lets_the_refreshed_file_be_saved() {
        let fake = remote_plane();
        let binding = PlaneBinding {
            plane: fake.plane,
            identity: None,
        };
        let file = bind_file(&fake.bridge().work_panel, binding, revision("a"));

        let (refused, refused_id) = exchange(
            save_work_file(fake.bridge(), file.to_string(), "mine".into()),
            async {
                let (mut reader, mut writer) = fake.accept().await;
                let (stream, message) = next(&mut reader).await;
                assert!(matches!(
                    &message,
                    ClientMessage::Command {
                        command: CommandRequest::ApplyUserEdit { expected_revision, content, .. },
                        ..
                    } if *expected_revision == revision("a") && content == "mine"
                ));
                let id = command_id(&message);
                refuse(
                    &mut writer,
                    stream,
                    &message,
                    wire(ErrorCategory::Conflict, "user_edit.stale_revision"),
                )
                .await;
                id
            },
        )
        .await;
        assert_eq!(refused.unwrap_err().code, "user_edit.stale_revision");

        let (loaded, _) = exchange(load_work_file(fake.bridge(), file.to_string()), async {
            let (mut reader, mut writer) = fake.accept().await;
            let (stream, message) = next(&mut reader).await;
            assert!(matches!(
                message,
                ClientMessage::Query {
                    query: QueryRequest::EditableFile { .. },
                    ..
                }
            ));
            let file = EditableFile {
                cursor: 12,
                target: TARGET,
                path: "src/lib.rs".into(),
                revision: revision("b"),
                content: Some("theirs".into()),
            };
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::EditableFile(file),
            )
            .await;
        })
        .await;
        assert_eq!(loaded.unwrap().content.as_deref(), Some("theirs"));

        let (saved, saved_id) = exchange(
            save_work_file(fake.bridge(), file.to_string(), "theirs and mine".into()),
            async {
                let (mut reader, mut writer) = fake.accept().await;
                let (stream, message) = next(&mut reader).await;
                assert!(matches!(
                    &message,
                    ClientMessage::Command {
                        command: CommandRequest::ApplyUserEdit { expected_revision, .. },
                        ..
                    } if *expected_revision == revision("b")
                ));
                let id = command_id(&message);
                complete(
                    &mut writer,
                    stream,
                    &message,
                    CommandResponse::UserEditApplied {
                        target: TARGET,
                        path: "src/lib.rs".into(),
                        revision: revision("c"),
                    },
                )
                .await;
                id
            },
        )
        .await;
        assert_eq!(saved.unwrap().revision, revision_label(&revision("c")));
        assert_ne!(saved_id, refused_id, "the refused edit's ID is not reused");
        assert!(fake.bridge().work_panel.edits.lock().unwrap().is_empty());
    }

    /// D4: a save that never reaches the Plane (here the local Plane is
    /// offline) records no pending edit, so the next save of other content
    /// is not refused with `user_edit.retry_mismatch`. The same holds for a
    /// review comment.
    #[tokio::test]
    async fn a_save_that_never_reached_the_plane_does_not_lock_the_file() {
        let setup = crate::jet::enrollment::tests::setup();
        let bridge = &setup.bridge;
        let file = bind_file(&bridge.work_panel, LOCAL, revision("a"));

        for content in ["mine", "other"] {
            let error = save_work_file(bridge, file.to_string(), content.into())
                .await
                .unwrap_err();
            assert_eq!(error.code, "transport.offline", "{content}");
        }
        assert!(bridge.work_panel.edits.lock().unwrap().is_empty());

        for comment in ["first", "second"] {
            let error = submit_file_review(bridge, file.to_string(), 1, comment.into())
                .await
                .unwrap_err();
            assert_eq!(error.code, "transport.offline", "{comment}");
        }
        assert!(bridge.work_panel.reviews.lock().unwrap().is_empty());
    }

    #[test]
    fn work_panel_boundary_rejects_unsafe_terminal_values() {
        assert!(validate_dimensions(24, 80).is_ok());
        assert!(validate_dimensions(0, 80).is_err());
        assert!(validate_dimensions(24, 1001).is_err());
        assert_eq!(
            requested_scope("turn", Some(2), None, None).unwrap(),
            DiffScope::Turn { turn: 2 }
        );
        assert!(requested_scope("historical", None, Some(3), Some(2)).is_err());
        assert!(all_zero("000000"));
        assert!(!all_zero("000001"));
    }

    #[test]
    fn loading_another_run_expires_previous_work_authority() {
        let state = WorkPanelState::default();
        let conversation = Uuid::new_v4();
        let first_run = Uuid::new_v4();
        let second_run = Uuid::new_v4();
        state
            .remember_active(
                conversation,
                ActiveWork {
                    binding: LOCAL,
                    run_id: first_run,
                    workspace_id: None,
                },
            )
            .unwrap();
        assert!(state
            .require_active(PlaneId::Local, conversation, first_run)
            .is_ok());
        state
            .remember_active(
                conversation,
                ActiveWork {
                    binding: LOCAL,
                    run_id: second_run,
                    workspace_id: None,
                },
            )
            .unwrap();
        assert!(state
            .require_active(PlaneId::Local, conversation, first_run)
            .is_err());
        assert!(state
            .require_active(PlaneId::Local, conversation, second_run)
            .is_ok());
    }

    #[test]
    fn work_authority_is_bound_to_the_plane_it_was_loaded_from() {
        let state = WorkPanelState::default();
        let conversation = Uuid::from_u128(4);
        let run = Uuid::from_u128(5);
        let remote = PlaneBinding {
            plane: PlaneId::Remote(Uuid::from_u128(2)),
            identity: Some(Uuid::from_u128(20)),
        };
        state
            .remember_active(
                conversation,
                ActiveWork {
                    binding: remote,
                    run_id: run,
                    workspace_id: None,
                },
            )
            .unwrap();
        // The same Conversation and Run UUIDs on this computer are not the
        // loaded Work panel.
        assert!(state
            .require_active(PlaneId::Local, conversation, run)
            .is_err());
        assert!(state
            .require_active(remote.plane, conversation, run)
            .is_ok());

        // A pending edit keeps its Plane: the same body on another Plane is a
        // different request, never a retry of this one.
        let file = Uuid::from_u128(6);
        let first = state
            .edit_command(file, remote, "text", revision("a"))
            .unwrap();
        assert_eq!(
            state
                .edit_command(file, remote, "text", revision("a"))
                .unwrap(),
            first
        );
        assert_eq!(
            state
                .edit_command(file, LOCAL, "text", revision("a"))
                .unwrap_err()
                .code,
            "user_edit.retry_mismatch"
        );
        let review = state
            .review_command(remote, conversation, file, 1, "note")
            .unwrap();
        assert_ne!(
            state
                .review_command(LOCAL, conversation, file, 1, "note")
                .unwrap(),
            review
        );
    }
}
