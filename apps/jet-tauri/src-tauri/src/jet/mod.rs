mod channels;
mod client;
mod conversations;
mod errors;
mod identity;
mod run_control;
mod setup;

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
