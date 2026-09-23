use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use jet_client::TerminalEvent;
use jet_protocol::{
    ArtifactAvailability, ArtifactVerifier, ChangeArtifact, ChangeDiff, ChangeOrigin, DiffScope,
    FileRevision, FileTarget, PageCursor, ReviewComment, RunLifecycle, Sha256Digest, TerminalState,
    WorkingTree, WorkspaceTerminal,
};
use serde::Serialize;
use tauri::{ipc::Channel, State};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{errors::PublicError, JetBridge};

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
    active: Mutex<HashMap<Uuid, ActiveWork>>,
    edits: Mutex<HashMap<Uuid, PendingEdit>>,
    reviews: Mutex<HashMap<Uuid, PendingReview>>,
    opens: Mutex<HashMap<OpenKey, Uuid>>,
    closes: Mutex<HashMap<Uuid, Uuid>>,
    known_terminals: Mutex<HashMap<Uuid, Uuid>>,
    terminal_sessions: Arc<Mutex<HashMap<Uuid, TerminalSession>>>,
    terminal_offsets: Arc<Mutex<HashMap<Uuid, u64>>>,
}

#[derive(Clone)]
struct BoundFile {
    conversation_id: Uuid,
    run_id: Uuid,
    target: FileTarget,
    path: String,
    revision: Option<FileRevision>,
}

struct BoundPage {
    conversation_id: Uuid,
    run_id: Uuid,
    target: Option<FileTarget>,
    cursor: PageCursor,
}

struct ArtifactRead {
    conversation_id: Uuid,
    run_id: Uuid,
    sha256: String,
    size: u64,
    next_offset: u64,
    verifier: ArtifactVerifier,
}

#[derive(Clone, Copy)]
struct ActiveWork {
    run_id: Uuid,
    workspace_id: Option<Uuid>,
}

struct PendingEdit {
    command_id: Uuid,
    content: String,
}

struct PendingReview {
    command_id: Uuid,
    file_id: Uuid,
    line: u32,
    comment: String,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct OpenKey {
    conversation_id: Uuid,
    rows: u16,
    columns: u16,
}

struct TerminalSession {
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
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    run_id: String,
    scope_kind: String,
    turn: Option<u32>,
    from_turn: Option<u32>,
    to_turn: Option<u32>,
) -> Result<WorkPanelSnapshot, PublicError> {
    let conversation_id = parse_id(&conversation_id, "conversation.identifier_invalid")?;
    let run_id = parse_id(&run_id, "run.identifier_invalid")?;
    let client = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let conversation = client
        .conversation(conversation_id)
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
    let scope = requested_scope(&scope_kind, turn, from_turn, to_turn)?;
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
        .change_diff(run_id, scope)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let target = file_target(&conversation.conversation.working_tree, diff.workspace_id);
    let workspace_id = diff.workspace_id;
    bridge.work_panel.remember_active(
        conversation_id,
        ActiveWork {
            run_id,
            workspace_id,
        },
    )?;
    let files = bridge
        .work_panel
        .bind_files(conversation_id, run_id, target, &diff)?;
    let next_page = bridge
        .work_panel
        .bind_page(conversation_id, run_id, target, diff.next_page)?;
    let artifact_read_id = bridge
        .work_panel
        .bind_artifact(conversation_id, run_id, &diff)?;
    let (terminals, terminal_issue) = match workspace_id {
        Some(workspace_id) => match client.workspace_terminals(workspace_id).await {
            Ok((_, terminals)) => {
                bridge
                    .work_panel
                    .remember_terminals(workspace_id, &terminals)?;
                (terminals.into_iter().map(terminal_view).collect(), None)
            }
            Err(error) => (Vec::new(), Some(PublicError::from_client(&error))),
        },
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

pub(crate) async fn load_more_changes(
    bridge: State<'_, JetBridge>,
    page_id: String,
) -> Result<ChangePageView, PublicError> {
    let page_id = parse_id(&page_id, "changes.page_invalid")?;
    let page = bridge.work_panel.take_page(page_id)?;
    bridge
        .work_panel
        .require_active(page.conversation_id, page.run_id)?;
    let diff = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?
        .next_change_diff(page.cursor)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    if diff.run_id != page.run_id {
        return Err(PublicError::invalid_input(
            "changes.page_mismatch",
            "The changed-file page no longer belongs to this Run.",
        ));
    }
    let files =
        bridge
            .work_panel
            .bind_files(page.conversation_id, page.run_id, page.target, &diff)?;
    let next_page = bridge.work_panel.bind_page(
        page.conversation_id,
        page.run_id,
        page.target,
        diff.next_page,
    )?;
    Ok(ChangePageView { files, next_page })
}

pub(crate) async fn load_patch_chunk(
    bridge: State<'_, JetBridge>,
    artifact_read_id: String,
) -> Result<ArtifactChunkView, PublicError> {
    let read_id = parse_id(&artifact_read_id, "artifact.read_invalid")?;
    let mut read = bridge.work_panel.take_artifact(read_id)?;
    bridge
        .work_panel
        .require_active(read.conversation_id, read.run_id)?;
    let chunk = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?
        .change_artifact(read.sha256.clone(), read.next_offset)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
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
    bridge: State<'_, JetBridge>,
    file_id: String,
) -> Result<EditableFileView, PublicError> {
    let file_id = parse_id(&file_id, "file.identifier_invalid")?;
    let bound = bridge.work_panel.bound_file(file_id)?;
    // ASVS 5.3.2 and 8.3.1: the webview selects an opaque binding created
    // from Plane data; it never supplies a native path or widens authority.
    let file = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?
        .editable_file(bound.target, &bound.path)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
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
    bridge: State<'_, JetBridge>,
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
    let command_id = bridge.work_panel.edit_command(file_id, &content)?;
    let saved = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?
        .apply_user_edit(command_id, bound.target, &bound.path, revision, &content)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    bridge.work_panel.finish_edit(file_id, saved.clone())?;
    Ok(FileSavedView {
        file_id: file_id.to_string(),
        revision: revision_label(&saved),
        message: "Saved through the selected Workspace.",
    })
}

pub(crate) async fn submit_file_review(
    bridge: State<'_, JetBridge>,
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
    let command_id =
        bridge
            .work_panel
            .review_command(bound.conversation_id, file_id, line, &comment)?;
    let turn = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?
        .submit_review(
            command_id,
            bound.conversation_id,
            vec![ReviewComment {
                path: bound.path,
                line,
                comment,
            }],
        )
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    bridge.work_panel.finish_review(bound.conversation_id)?;
    Ok(ReviewSubmittedView {
        turn_id: turn.turn_id.to_string(),
        state: turn_state(turn.state),
        message: "The review comment was added as one queued Turn.",
    })
}

pub(crate) async fn open_workspace_terminal(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    rows: u16,
    columns: u16,
) -> Result<TerminalView, PublicError> {
    let conversation_id = parse_id(&conversation_id, "conversation.identifier_invalid")?;
    validate_dimensions(rows, columns)?;
    let active = bridge.work_panel.active_work(conversation_id)?;
    let workspace_id = active.workspace_id.ok_or_else(|| {
        PublicError::invalid_input(
            "terminal.workspace_required",
            "Terminals are available only for a managed Workspace.",
        )
    })?;
    let key = OpenKey {
        conversation_id,
        rows,
        columns,
    };
    let command_id = bridge.work_panel.open_command(key)?;
    let terminal = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?
        .open_terminal(command_id, workspace_id, rows, columns)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    if terminal.workspace_id != workspace_id {
        return Err(PublicError::invalid_input(
            "terminal.workspace_mismatch",
            "The Plane returned a terminal from another Workspace.",
        ));
    }
    bridge.work_panel.finish_open(key, &terminal)?;
    Ok(terminal_view(terminal))
}

pub(crate) async fn close_workspace_terminal(
    bridge: State<'_, JetBridge>,
    terminal_id: String,
) -> Result<TerminalView, PublicError> {
    let terminal_id = parse_id(&terminal_id, "terminal.identifier_invalid")?;
    // ASVS 5.3.2 and 8.3.1: only a terminal returned for the active managed
    // Workspace can be attached; the webview cannot launch a host process.
    bridge.work_panel.require_terminal(terminal_id)?;
    bridge.work_panel.detach_terminal(terminal_id)?;
    let command_id = bridge.work_panel.close_command(terminal_id)?;
    let terminal = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?
        .close_terminal(command_id, terminal_id)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    if terminal.terminal_id != terminal_id {
        return Err(PublicError::invalid_input(
            "terminal.response_mismatch",
            "The Plane returned a different terminal than Jet closed.",
        ));
    }
    bridge.work_panel.finish_close(terminal_id, &terminal)?;
    Ok(terminal_view(terminal))
}

pub(crate) async fn attach_workspace_terminal(
    bridge: State<'_, JetBridge>,
    terminal_id: String,
    on_update: Channel<TerminalUpdate>,
) -> Result<(), PublicError> {
    let terminal_id = parse_id(&terminal_id, "terminal.identifier_invalid")?;
    bridge.work_panel.require_terminal(terminal_id)?;
    let after = bridge.work_panel.terminal_offset(terminal_id)?;
    let client = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let (actions, mut receive_actions) = mpsc::channel(32);
    let generation = Uuid::new_v4();
    bridge
        .work_panel
        .start_terminal_session(terminal_id, generation, actions)?;
    let sessions = Arc::clone(&bridge.work_panel.terminal_sessions);
    let offsets = Arc::clone(&bridge.work_panel.terminal_offsets);
    tokio::spawn(async move {
        let result = async {
            let mut attachment = client
                .attach_terminal(terminal_id, after, TERMINAL_CREDIT)
                .await?;
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
                error: PublicError::from_client(&error),
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
    bridge: State<'_, JetBridge>,
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
    bridge
        .work_panel
        .terminal_action(terminal_id, TerminalAction::Input(input.into_bytes()))
        .await
}

pub(crate) async fn resize_workspace_terminal(
    bridge: State<'_, JetBridge>,
    terminal_id: String,
    rows: u16,
    columns: u16,
) -> Result<(), PublicError> {
    let terminal_id = parse_id(&terminal_id, "terminal.identifier_invalid")?;
    validate_dimensions(rows, columns)?;
    bridge
        .work_panel
        .terminal_action(terminal_id, TerminalAction::Resize { rows, columns })
        .await
}

pub(crate) fn detach_workspace_terminal(
    bridge: State<'_, JetBridge>,
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
        values.insert(conversation_id, active);
        drop(values);
        self.files
            .lock()
            .map_err(|_| PublicError::internal())?
            .retain(|_, file| {
                file.conversation_id == conversation_id && file.run_id == active.run_id
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

    fn active_work(&self, conversation_id: Uuid) -> Result<ActiveWork, PublicError> {
        self.active
            .lock()
            .map_err(|_| PublicError::internal())?
            .get(&conversation_id)
            .copied()
            .ok_or_else(|| {
                PublicError::invalid_input(
                    "work_panel.not_loaded",
                    "Reload the Work panel before using this action.",
                )
            })
    }

    fn require_active(&self, conversation_id: Uuid, run_id: Uuid) -> Result<(), PublicError> {
        if self.active_work(conversation_id)?.run_id == run_id {
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
        conversation_id: Uuid,
        run_id: Uuid,
        target: Option<FileTarget>,
        diff: &ChangeDiff,
    ) -> Result<Vec<ChangedFileView>, PublicError> {
        let mut bindings = self.files.lock().map_err(|_| PublicError::internal())?;
        let mut views = Vec::with_capacity(diff.files.len());
        for file in &diff.files {
            let id = bindings
                .iter()
                .find_map(|(id, existing)| {
                    (existing.run_id == run_id && existing.path == file.path).then_some(*id)
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
        conversation_id: Uuid,
        run_id: Uuid,
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
                conversation_id,
                run_id,
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
        conversation_id: Uuid,
        run_id: Uuid,
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
                    conversation_id,
                    run_id,
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
        self.require_active(file.conversation_id, file.run_id)?;
        Ok(file)
    }

    fn remember_revision(&self, id: Uuid, revision: FileRevision) -> Result<(), PublicError> {
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

    fn edit_command(&self, file_id: Uuid, content: &str) -> Result<Uuid, PublicError> {
        let mut edits = self.edits.lock().map_err(|_| PublicError::internal())?;
        if let Some(pending) = edits.get(&file_id) {
            if pending.content == content {
                return Ok(pending.command_id);
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
                command_id,
                content: content.into(),
            },
        );
        Ok(command_id)
    }

    fn finish_edit(&self, file_id: Uuid, revision: FileRevision) -> Result<(), PublicError> {
        self.edits
            .lock()
            .map_err(|_| PublicError::internal())?
            .remove(&file_id);
        self.remember_revision(file_id, revision)
    }

    fn review_command(
        &self,
        conversation_id: Uuid,
        file_id: Uuid,
        line: u32,
        comment: &str,
    ) -> Result<Uuid, PublicError> {
        let mut reviews = self.reviews.lock().map_err(|_| PublicError::internal())?;
        if let Some(pending) = reviews.get(&conversation_id) {
            if pending.file_id == file_id && pending.line == line && pending.comment == comment {
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
            conversation_id,
            PendingReview {
                command_id,
                file_id,
                line,
                comment: comment.into(),
            },
        );
        Ok(command_id)
    }

    fn finish_review(&self, conversation_id: Uuid) -> Result<(), PublicError> {
        self.reviews
            .lock()
            .map_err(|_| PublicError::internal())?
            .remove(&conversation_id);
        Ok(())
    }

    fn open_command(&self, key: OpenKey) -> Result<Uuid, PublicError> {
        command_id(&self.opens, key)
    }

    fn finish_open(&self, key: OpenKey, terminal: &WorkspaceTerminal) -> Result<(), PublicError> {
        self.opens
            .lock()
            .map_err(|_| PublicError::internal())?
            .remove(&key);
        self.known_terminals
            .lock()
            .map_err(|_| PublicError::internal())?
            .insert(terminal.terminal_id, terminal.workspace_id);
        Ok(())
    }

    fn close_command(&self, terminal_id: Uuid) -> Result<Uuid, PublicError> {
        command_id(&self.closes, terminal_id)
    }

    fn finish_close(
        &self,
        terminal_id: Uuid,
        terminal: &WorkspaceTerminal,
    ) -> Result<(), PublicError> {
        self.closes
            .lock()
            .map_err(|_| PublicError::internal())?
            .remove(&terminal_id);
        self.known_terminals
            .lock()
            .map_err(|_| PublicError::internal())?
            .insert(terminal.terminal_id, terminal.workspace_id);
        Ok(())
    }

    fn remember_terminals(
        &self,
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
            known.insert(terminal.terminal_id, terminal.workspace_id);
        }
        Ok(())
    }

    fn require_terminal(&self, terminal_id: Uuid) -> Result<(), PublicError> {
        let workspace_id = self
            .known_terminals
            .lock()
            .map_err(|_| PublicError::internal())?
            .get(&terminal_id)
            .copied();
        let active_workspace = self
            .active
            .lock()
            .map_err(|_| PublicError::internal())?
            .values()
            .next()
            .and_then(|active| active.workspace_id);
        if workspace_id.is_some() && workspace_id == active_workspace {
            Ok(())
        } else {
            Err(PublicError::invalid_input(
                "terminal.selection_expired",
                "Reload the Work panel before using this terminal.",
            ))
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
                generation,
                actions,
            },
        );
        Ok(())
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
    use super::{all_zero, requested_scope, validate_dimensions, ActiveWork, WorkPanelState};
    use jet_protocol::DiffScope;
    use uuid::Uuid;

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
                    run_id: first_run,
                    workspace_id: None,
                },
            )
            .unwrap();
        assert!(state.require_active(conversation, first_run).is_ok());
        state
            .remember_active(
                conversation,
                ActiveWork {
                    run_id: second_run,
                    workspace_id: None,
                },
            )
            .unwrap();
        assert!(state.require_active(conversation, first_run).is_err());
        assert!(state.require_active(conversation, second_run).is_ok());
    }
}
