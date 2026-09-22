use jet_protocol::PlaneStatus;
use serde::Serialize;
use tauri::{ipc::Channel, State};

use super::{
    client::{ApprovalProjection, EventSummary, NativeUpdate},
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
        conversation_id: Option<String>,
        run_id: Option<String>,
        timeline: Vec<TimelineItemView>,
    },
    Reconnecting {
        error: PublicError,
    },
    Failed {
        error: PublicError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimelineItemView {
    kind: &'static str,
    text: String,
    item_id: Option<String>,
    approval: Option<ApprovalView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalView {
    request_id: String,
    review_id: Option<String>,
    run_id: Option<String>,
    tool: String,
    action: String,
    target: String,
    scope: &'static str,
    consequence: String,
    rationale: Option<String>,
    state: &'static str,
    can_authorize_retry: bool,
}

impl From<ApprovalProjection> for ApprovalView {
    fn from(value: ApprovalProjection) -> Self {
        Self {
            request_id: value.request_id,
            review_id: value.review_id,
            run_id: value.run_id,
            tool: value.tool,
            action: value.action,
            target: value.target,
            scope: value.scope,
            consequence: value.consequence,
            rationale: value.rationale,
            state: value.state,
            can_authorize_retry: value.can_authorize_retry,
        }
    }
}

impl From<NativeUpdate> for PlaneUpdate {
    fn from(update: NativeUpdate) -> Self {
        match update {
            NativeUpdate::Resumed { after } => Self::Resumed {
                after: after.to_string(),
            },
            NativeUpdate::Event(EventSummary {
                notification: _,
                sequence,
                recorded_at_unix_ms,
                kind,
                conversation_id,
                run_id,
                timeline,
            }) => Self::Event {
                sequence: sequence.to_string(),
                recorded_at_unix_ms: recorded_at_unix_ms.to_string(),
                kind,
                conversation_id: conversation_id.map(|id| id.to_string()),
                run_id: run_id.map(|id| id.to_string()),
                timeline: timeline
                    .into_iter()
                    .map(|item| TimelineItemView {
                        kind: item.kind,
                        text: item.text,
                        item_id: item.item_id,
                        approval: item.approval.map(Into::into),
                    })
                    .collect(),
            },
            NativeUpdate::Reconnecting { error } => Self::Reconnecting { error },
            NativeUpdate::Failed { error } => Self::Failed { error },
        }
    }
}

/// Opens the sole read-only webview boundary: a fenced status snapshot plus
/// ordered, redacted updates that resume after the native cursor.
pub(crate) async fn open_plane_feed(
    app: tauri::AppHandle,
    bridge: State<'_, JetBridge>,
    on_update: Channel<PlaneUpdate>,
    after: Option<String>,
) -> Result<ConnectionSnapshot, PublicError> {
    let requested_cursor = parse_resume_cursor(after)?;
    let client = bridge.client.clone();
    let notifications = bridge.notifications.clone();
    match client.status().await {
        Ok(status) => {
            let cursor = requested_cursor.unwrap_or_else(|| status.cursor.unwrap_or_default());
            notifications.fence(status.cursor.unwrap_or_default());
            let snapshot = ConnectionSnapshot::from_status(status);
            tauri::async_runtime::spawn(stream_updates(
                app,
                notifications,
                client,
                cursor,
                on_update,
            ));
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
                            let cursor = requested_cursor
                                .unwrap_or_else(|| status.cursor.unwrap_or_default());
                            notifications.fence(status.cursor.unwrap_or_default());
                            let connection = ConnectionSnapshot::from_status(status);
                            if on_update
                                .send(PlaneUpdate::Connected { connection })
                                .is_err()
                            {
                                return;
                            }
                            stream_updates(app, notifications, client, cursor, on_update).await;
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

fn parse_resume_cursor(after: Option<String>) -> Result<Option<u64>, PublicError> {
    after
        .map(|value| {
            if value.is_empty()
                || value.len() > 20
                || !value.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(PublicError::invalid_input(
                    "event.cursor_invalid",
                    "Jet could not resume from that activity cursor.",
                ));
            }
            value.parse::<u64>().map_err(|_| {
                PublicError::invalid_input(
                    "event.cursor_invalid",
                    "Jet could not resume from that activity cursor.",
                )
            })
        })
        .transpose()
}

async fn stream_updates(
    app: tauri::AppHandle,
    notifications: std::sync::Arc<super::notifications::NotificationState>,
    client: super::client::PlaneClient,
    cursor: u64,
    on_update: Channel<PlaneUpdate>,
) {
    client
        .stream_updates(cursor, |update| {
            if let NativeUpdate::Event(ref event) = update {
                notifications.observe(&app, event.sequence, event.notification);
            }
            on_update.send(update.into()).is_ok()
        })
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
    use super::{bounded_text, parse_resume_cursor};

    #[test]
    fn status_text_is_bounded_before_serialization() {
        assert_eq!(bounded_text("0.2.0", 48, "Unknown"), "0.2.0");
        assert_eq!(bounded_text("<script>", 48, "Unknown"), "Unknown");
        assert_eq!(bounded_text(&"x".repeat(49), 48, "Unknown"), "Unknown");
    }

    #[test]
    fn resume_cursor_is_bounded_and_numeric() {
        assert_eq!(parse_resume_cursor(Some("108".into())).unwrap(), Some(108));
        assert!(parse_resume_cursor(Some("1e8".into())).is_err());
        assert!(parse_resume_cursor(Some("9".repeat(21))).is_err());
    }
}
