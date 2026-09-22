use jet_protocol::PlaneStatus;
use serde::Serialize;
use tauri::{ipc::Channel, State};

use super::{
    client::{EventSummary, NativeUpdate},
    errors::PublicError,
    JetBridge,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionSnapshot {
    state: &'static str,
    core_version: Option<String>,
    daemon_starts: Option<String>,
    started_at_unix_ms: Option<String>,
    cursor: Option<String>,
}

impl ConnectionSnapshot {
    fn from_status(status: PlaneStatus) -> Self {
        Self {
            state: "online",
            core_version: Some(bounded_text(&status.core_version, 48, "Unknown")),
            daemon_starts: Some(status.daemon_starts.to_string()),
            started_at_unix_ms: Some(status.started_at_unix_ms.to_string()),
            cursor: Some(status.cursor.unwrap_or_default().to_string()),
        }
    }

    fn reconnecting() -> Self {
        Self {
            state: "reconnecting",
            core_version: None,
            daemon_starts: None,
            started_at_unix_ms: None,
            cursor: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum PlaneUpdate {
    Connected {
        connection: ConnectionSnapshot,
    },
    Resumed {
        after: String,
    },
    Event {
        sequence: String,
        recorded_at_unix_ms: String,
        kind: String,
    },
    Reconnecting {
        error: PublicError,
    },
    Failed {
        error: PublicError,
    },
}

impl From<NativeUpdate> for PlaneUpdate {
    fn from(update: NativeUpdate) -> Self {
        match update {
            NativeUpdate::Resumed { after } => Self::Resumed {
                after: after.to_string(),
            },
            NativeUpdate::Event(EventSummary {
                sequence,
                recorded_at_unix_ms,
                kind,
            }) => Self::Event {
                sequence: sequence.to_string(),
                recorded_at_unix_ms: recorded_at_unix_ms.to_string(),
                kind,
            },
            NativeUpdate::Reconnecting { error } => Self::Reconnecting { error },
            NativeUpdate::Failed { error } => Self::Failed { error },
        }
    }
}

/// Opens the sole read-only webview boundary: a fenced status snapshot plus
/// ordered, redacted updates that resume after the native cursor.
pub(crate) async fn open_plane_feed(
    bridge: State<'_, JetBridge>,
    on_update: Channel<PlaneUpdate>,
) -> Result<ConnectionSnapshot, PublicError> {
    let client = bridge.client.clone();
    match client.status().await {
        Ok(status) => {
            let cursor = status.cursor.unwrap_or_default();
            let snapshot = ConnectionSnapshot::from_status(status);
            tauri::async_runtime::spawn(stream_updates(client, cursor, on_update));
            Ok(snapshot)
        }
        Err(error) => {
            let public = PublicError::from_client(&error);
            if !public.retryable {
                return Err(public);
            }
            tauri::async_runtime::spawn(async move {
                if on_update
                    .send(PlaneUpdate::Reconnecting { error: public })
                    .is_err()
                {
                    return;
                }
                loop {
                    match client.status().await {
                        Ok(status) => {
                            let cursor = status.cursor.unwrap_or_default();
                            let connection = ConnectionSnapshot::from_status(status);
                            if on_update
                                .send(PlaneUpdate::Connected { connection })
                                .is_err()
                            {
                                return;
                            }
                            stream_updates(client, cursor, on_update).await;
                            return;
                        }
                        Err(error) => {
                            let error = PublicError::from_client(&error);
                            let terminal = !error.retryable;
                            let update = if terminal {
                                PlaneUpdate::Failed { error }
                            } else {
                                PlaneUpdate::Reconnecting { error }
                            };
                            if on_update.send(update).is_err() || terminal {
                                return;
                            }
                        }
                    }
                }
            });
            Ok(ConnectionSnapshot::reconnecting())
        }
    }
}

async fn stream_updates(
    client: super::client::PlaneClient,
    cursor: u64,
    on_update: Channel<PlaneUpdate>,
) {
    client
        .stream_updates(cursor, |update| on_update.send(update.into()).is_ok())
        .await;
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
    use super::bounded_text;

    #[test]
    fn status_text_is_bounded_before_serialization() {
        assert_eq!(bounded_text("0.2.0", 48, "Unknown"), "0.2.0");
        assert_eq!(bounded_text("<script>", 48, "Unknown"), "Unknown");
        assert_eq!(bounded_text(&"x".repeat(49), 48, "Unknown"), "Unknown");
    }
}
