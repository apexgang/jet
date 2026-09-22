//! Native opt-in notifications: no prompt content, no replay, no webview sender.
use super::{errors::PublicError, JetBridge};
use jet_protocol::Event;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, sync::Mutex};
use tauri::{AppHandle, State};
use tauri_plugin_notification::NotificationExt;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NotificationPreferences {
    enabled: bool,
    approvals: bool,
    completion: bool,
    failure: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotificationSignal {
    Approval,
    Completed,
    Failed,
}

impl NotificationSignal {
    pub(crate) fn from_event(event: &Event) -> Option<Self> {
        match event.kind.as_str() {
            "approval.requested" => Some(Self::Approval),
            "run.lifecycle_changed" => match event.payload.get("to").and_then(|v| v.as_str()) {
                Some("completed") => Some(Self::Completed),
                Some("failed" | "lost") => Some(Self::Failed),
                _ => None,
            },
            _ => None,
        }
    }
    fn body(self) -> &'static str {
        match self {
            Self::Approval => "A task needs your approval. Open Jet to review it.",
            Self::Completed => "A Run completed. Open Jet to review its results.",
            Self::Failed => "A Run needs attention. Open Jet to inspect its state.",
        }
    }
}

#[derive(Default)]
struct NotificationGate {
    preferences: NotificationPreferences,
    cursor: u64,
    error: Option<&'static str>,
}
impl NotificationGate {
    fn consume(
        &mut self,
        sequence: u64,
        signal: Option<NotificationSignal>,
    ) -> Option<NotificationSignal> {
        if sequence <= self.cursor {
            return None;
        }
        self.cursor = sequence;
        if !self.preferences.enabled {
            return None;
        }
        signal.filter(|s| match s {
            NotificationSignal::Approval => self.preferences.approvals,
            NotificationSignal::Completed => self.preferences.completion,
            NotificationSignal::Failed => self.preferences.failure,
        })
    }
}

pub(crate) struct NotificationState {
    path: PathBuf,
    gate: Mutex<NotificationGate>,
}
impl NotificationState {
    pub(crate) fn new(directory: &std::path::Path) -> Self {
        let path = directory.join("notification-preferences.json");
        let preferences = fs::read(&path)
            .ok()
            .filter(|b| b.len() <= 1024)
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            gate: Mutex::new(NotificationGate {
                preferences,
                ..Default::default()
            }),
        }
    }
    pub(crate) fn fence(&self, cursor: u64) {
        if let Ok(mut gate) = self.gate.lock() {
            gate.cursor = gate.cursor.max(cursor);
        }
    }
    pub(crate) fn observe(
        &self,
        app: &AppHandle,
        sequence: u64,
        signal: Option<NotificationSignal>,
    ) {
        let Ok(mut gate) = self.gate.lock() else {
            return;
        };
        if let Some(signal) = gate.consume(sequence, signal) {
            // Fixed copy cannot disclose prompts, paths, tools or credentials on
            // the lock screen. Native events, not webview claims, select it.
            if app
                .notification()
                .builder()
                .title("Jet")
                .body(signal.body())
                .show()
                .is_err()
            {
                gate.error = Some("Desktop notifications could not be delivered. Check your system notification settings.");
            }
        }
    }
    fn view(&self) -> Result<NotificationSettings, PublicError> {
        let gate = self.gate.lock().map_err(|_| PublicError::internal())?;
        Ok(NotificationSettings {
            preferences: gate.preferences,
            error: gate.error,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationSettings {
    preferences: NotificationPreferences,
    error: Option<&'static str>,
}

#[tauri::command]
pub(crate) fn load_notification_settings(
    bridge: State<'_, JetBridge>,
) -> Result<NotificationSettings, PublicError> {
    bridge.notifications.view()
}

#[tauri::command]
pub(crate) async fn set_notification_settings(
    app: AppHandle,
    bridge: State<'_, JetBridge>,
    preferences: NotificationPreferences,
) -> Result<NotificationSettings, PublicError> {
    if preferences.enabled
        && app
            .notification()
            .request_permission()
            .map_err(|_| PublicError::internal())?
            != tauri_plugin_notification::PermissionState::Granted
    {
        return Err(PublicError::invalid_input(
            "notifications.permission_denied",
            "Allow Jet notifications in your desktop settings before enabling them.",
        ));
    }
    // Enabling starts at a fresh authoritative cursor. Turning off remains
    // possible offline; preferences never grant any daemon authority.
    let cursor = if preferences.enabled {
        Some(
            bridge
                .client
                .status()
                .await
                .map_err(|e| PublicError::from_client(&e))?
                .cursor
                .unwrap_or_default(),
        )
    } else {
        None
    };
    let mut gate = bridge
        .notifications
        .gate
        .lock()
        .map_err(|_| PublicError::internal())?;
    let temporary = bridge.notifications.path.with_extension("tmp");
    fs::write(
        &temporary,
        serde_json::to_vec(&preferences).map_err(|_| PublicError::internal())?,
    )
    .map_err(|_| PublicError::internal())?;
    fs::rename(temporary, &bridge.notifications.path).map_err(|_| PublicError::internal())?;
    gate.preferences = preferences;
    gate.error = None;
    if let Some(cursor) = cursor {
        gate.cursor = gate.cursor.max(cursor);
    }
    Ok(NotificationSettings {
        preferences,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_authoritative_attention_events_produce_generic_cues() {
        let mut event = jet_protocol::Event {
            sequence: 1,
            event_id: uuid::Uuid::new_v4(),
            actor: jet_protocol::Actor::InteractiveClient {
                client_id: uuid::Uuid::new_v4(),
            },
            origin: None,
            recorded_at_unix_ms: 1,
            conversation_id: None,
            run_id: None,
            kind: "run.lifecycle_changed".into(),
            payload_version: 1,
            payload: serde_json::json!({"to":"completed", "text":"private task content"}),
        };
        assert_eq!(
            NotificationSignal::from_event(&event),
            Some(NotificationSignal::Completed)
        );
        event.payload = serde_json::json!({"to":"lost"});
        assert_eq!(
            NotificationSignal::from_event(&event),
            Some(NotificationSignal::Failed)
        );
        event.kind = "run.output".into();
        assert_eq!(NotificationSignal::from_event(&event), None);
        event.kind = "approval.requested".into();
        assert_eq!(
            NotificationSignal::from_event(&event),
            Some(NotificationSignal::Approval)
        );
        event.kind = "approval.reviewed".into();
        assert_eq!(NotificationSignal::from_event(&event), None);
    }

    #[test]
    fn notifications_are_opt_in_filtered_and_never_replayed() {
        let mut gate = NotificationGate::default();
        assert_eq!(gate.consume(10, Some(NotificationSignal::Approval)), None);
        gate.preferences = NotificationPreferences {
            enabled: true,
            approvals: true,
            completion: true,
            failure: false,
        };
        assert_eq!(gate.consume(10, Some(NotificationSignal::Approval)), None);
        assert_eq!(
            gate.consume(11, Some(NotificationSignal::Approval)),
            Some(NotificationSignal::Approval)
        );
        assert_eq!(gate.consume(11, Some(NotificationSignal::Approval)), None);
        assert_eq!(gate.consume(12, Some(NotificationSignal::Failed)), None);
        assert_eq!(
            gate.consume(13, Some(NotificationSignal::Completed)),
            Some(NotificationSignal::Completed)
        );
        gate.preferences.enabled = false;
        assert_eq!(gate.consume(14, Some(NotificationSignal::Completed)), None);
    }
}
