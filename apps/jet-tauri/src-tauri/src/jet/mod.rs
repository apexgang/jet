mod channels;
mod client;
mod errors;
mod identity;

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
