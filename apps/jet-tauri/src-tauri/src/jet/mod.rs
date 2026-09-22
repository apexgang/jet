mod channels;
mod client;
mod errors;
mod identity;
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
        })
    }
}

#[tauri::command]
pub(crate) async fn open_plane_feed(
    bridge: State<'_, JetBridge>,
    on_update: Channel<PlaneUpdate>,
) -> Result<ConnectionSnapshot, PublicError> {
    channels::open_plane_feed(bridge, on_update).await
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
