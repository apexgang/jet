mod channels;
mod client;
mod conversations;
mod errors;
mod identity;
mod run_control;
mod setup;
mod work_panel;

use std::{io, path::Path, time::Duration};

use client::PlaneClient;
use tauri::{ipc::Channel, State};

use self::{
    channels::{ConnectionSnapshot, PlaneUpdate},
    errors::PublicError,
};

/// Native-only authority for the local Plane. None of these values cross the
/// webview boundary.
pub(crate) struct JetBridge {
    client: PlaneClient,
    setup: setup::SetupState,
    conversations: conversations::ConversationState,
    run_control: run_control::RunControlState,
    work_panel: work_panel::WorkPanelState,
}

impl JetBridge {
    pub(crate) fn for_local_plane(
        home_directory: &Path,
        app_data_directory: &Path,
    ) -> io::Result<Self> {
        let client_id = identity::load_or_create(app_data_directory)?;
        let socket = home_directory.join(".jet/runtime/jetd.sock");
        Ok(Self {
            client: PlaneClient::new(
                socket,
                client_id,
                [
                    Duration::from_millis(100),
                    Duration::from_millis(500),
                    Duration::from_secs(1),
                ],
                Duration::from_millis(500),
            ),
            setup: setup::SetupState::default(),
            conversations: conversations::ConversationState::new(app_data_directory),
            run_control: run_control::RunControlState::default(),
            work_panel: work_panel::WorkPanelState::default(),
        })
    }
}

#[tauri::command]
pub(crate) async fn open_plane_feed(
    bridge: State<'_, JetBridge>,
    on_update: Channel<PlaneUpdate>,
    after: Option<String>,
) -> Result<ConnectionSnapshot, PublicError> {
    channels::open_plane_feed(bridge, on_update, after).await
}

#[tauri::command]
pub(crate) async fn load_setup(
    bridge: State<'_, JetBridge>,
) -> Result<setup::SetupSnapshot, PublicError> {
    setup::load_setup(bridge).await
}

#[tauri::command]
pub(crate) async fn preview_project(
    bridge: State<'_, JetBridge>,
    path: String,
) -> Result<setup::ProjectPreviewView, PublicError> {
    setup::preview_project(bridge, path).await
}

#[tauri::command]
pub(crate) async fn register_project(
    bridge: State<'_, JetBridge>,
    preview_id: String,
) -> Result<setup::MutationResult, PublicError> {
    setup::register_project(bridge, preview_id).await
}

#[tauri::command]
pub(crate) async fn preview_project_removal(
    bridge: State<'_, JetBridge>,
    project_id: String,
) -> Result<setup::ProjectRemovalPreviewView, PublicError> {
    setup::preview_project_removal(bridge, project_id).await
}

#[tauri::command]
pub(crate) async fn remove_project(
    bridge: State<'_, JetBridge>,
    preview_id: String,
    typed_name: String,
    permanent: bool,
) -> Result<setup::MutationResult, PublicError> {
    setup::remove_project(bridge, preview_id, typed_name, permanent).await
}

#[tauri::command]
pub(crate) async fn bind_harness_account(
    bridge: State<'_, JetBridge>,
    provider: String,
) -> Result<setup::MutationResult, PublicError> {
    setup::bind_harness_account(bridge, provider).await
}

#[tauri::command]
pub(crate) async fn load_conversations(
    bridge: State<'_, JetBridge>,
    next_page: Option<String>,
) -> Result<conversations::ConversationPageView, PublicError> {
    conversations::load_conversations(bridge, next_page).await
}

#[tauri::command]
pub(crate) async fn search_conversations(
    bridge: State<'_, JetBridge>,
    text: String,
) -> Result<conversations::SearchResultView, PublicError> {
    conversations::search_conversations(bridge, text).await
}

#[tauri::command]
pub(crate) async fn load_conversation(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
) -> Result<conversations::ConversationDetailView, PublicError> {
    conversations::load_conversation(bridge, conversation_id).await
}

#[tauri::command]
pub(crate) async fn create_conversation(
    bridge: State<'_, JetBridge>,
    project_id: String,
) -> Result<conversations::ConversationRowView, PublicError> {
    conversations::create_conversation(bridge, project_id).await
}

#[tauri::command]
pub(crate) async fn start_run(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    craft: String,
    prompt: String,
) -> Result<conversations::StartResultView, PublicError> {
    conversations::start_run(bridge, conversation_id, craft, prompt).await
}

#[tauri::command]
pub(crate) async fn submit_turn(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    prompt: String,
) -> Result<conversations::TurnResultView, PublicError> {
    conversations::submit_turn(bridge, conversation_id, prompt).await
}

#[tauri::command]
pub(crate) async fn load_run_supervision(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    run_id: Option<String>,
) -> Result<run_control::RunSupervisionView, PublicError> {
    run_control::load_run_supervision(bridge, conversation_id, run_id).await
}

#[tauri::command]
pub(crate) async fn withdraw_turn(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    turn_id: String,
) -> Result<run_control::TurnView, PublicError> {
    run_control::withdraw_turn(bridge, conversation_id, turn_id).await
}

#[tauri::command]
pub(crate) async fn interrupt_turn(
    bridge: State<'_, JetBridge>,
    run_id: String,
) -> Result<run_control::CommandAcceptedView, PublicError> {
    run_control::interrupt_turn(bridge, run_id).await
}

#[tauri::command]
pub(crate) async fn stop_run(
    bridge: State<'_, JetBridge>,
    run_id: String,
) -> Result<run_control::CommandAcceptedView, PublicError> {
    run_control::stop_run(bridge, run_id).await
}

#[tauri::command]
pub(crate) async fn authorize_approval_retry(
    bridge: State<'_, JetBridge>,
    run_id: String,
    review_id: String,
) -> Result<run_control::ApprovalRetryView, PublicError> {
    run_control::authorize_approval_retry(bridge, run_id, review_id).await
}

#[tauri::command]
pub(crate) async fn load_work_panel(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    run_id: String,
    scope_kind: String,
    turn: Option<u32>,
    from_turn: Option<u32>,
    to_turn: Option<u32>,
) -> Result<work_panel::WorkPanelSnapshot, PublicError> {
    work_panel::load_work_panel(
        bridge,
        conversation_id,
        run_id,
        scope_kind,
        turn,
        from_turn,
        to_turn,
    )
    .await
}

#[tauri::command]
pub(crate) async fn load_more_changes(
    bridge: State<'_, JetBridge>,
    page_id: String,
) -> Result<work_panel::ChangePageView, PublicError> {
    work_panel::load_more_changes(bridge, page_id).await
}

#[tauri::command]
pub(crate) async fn load_patch_chunk(
    bridge: State<'_, JetBridge>,
    artifact_read_id: String,
) -> Result<work_panel::ArtifactChunkView, PublicError> {
    work_panel::load_patch_chunk(bridge, artifact_read_id).await
}

#[tauri::command]
pub(crate) async fn load_work_file(
    bridge: State<'_, JetBridge>,
    file_id: String,
) -> Result<work_panel::EditableFileView, PublicError> {
    work_panel::load_work_file(bridge, file_id).await
}

#[tauri::command]
pub(crate) async fn save_work_file(
    bridge: State<'_, JetBridge>,
    file_id: String,
    content: String,
) -> Result<work_panel::FileSavedView, PublicError> {
    work_panel::save_work_file(bridge, file_id, content).await
}

#[tauri::command]
pub(crate) async fn submit_file_review(
    bridge: State<'_, JetBridge>,
    file_id: String,
    line: u32,
    comment: String,
) -> Result<work_panel::ReviewSubmittedView, PublicError> {
    work_panel::submit_file_review(bridge, file_id, line, comment).await
}

#[tauri::command]
pub(crate) async fn open_workspace_terminal(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    rows: u16,
    columns: u16,
) -> Result<work_panel::TerminalView, PublicError> {
    work_panel::open_workspace_terminal(bridge, conversation_id, rows, columns).await
}

#[tauri::command]
pub(crate) async fn close_workspace_terminal(
    bridge: State<'_, JetBridge>,
    terminal_id: String,
) -> Result<work_panel::TerminalView, PublicError> {
    work_panel::close_workspace_terminal(bridge, terminal_id).await
}

#[tauri::command]
pub(crate) async fn attach_workspace_terminal(
    bridge: State<'_, JetBridge>,
    terminal_id: String,
    on_update: Channel<work_panel::TerminalUpdate>,
) -> Result<(), PublicError> {
    work_panel::attach_workspace_terminal(bridge, terminal_id, on_update).await
}

#[tauri::command]
pub(crate) async fn send_terminal_input(
    bridge: State<'_, JetBridge>,
    terminal_id: String,
    input: String,
) -> Result<(), PublicError> {
    work_panel::send_terminal_input(bridge, terminal_id, input).await
}

#[tauri::command]
pub(crate) async fn resize_workspace_terminal(
    bridge: State<'_, JetBridge>,
    terminal_id: String,
    rows: u16,
    columns: u16,
) -> Result<(), PublicError> {
    work_panel::resize_workspace_terminal(bridge, terminal_id, rows, columns).await
}

#[tauri::command]
pub(crate) fn detach_workspace_terminal(
    bridge: State<'_, JetBridge>,
    terminal_id: String,
) -> Result<(), PublicError> {
    work_panel::detach_workspace_terminal(bridge, terminal_id)
}
