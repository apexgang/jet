//! Native opt-in notifications: no prompt content, no replay, no webview sender.
use super::{errors::PublicError, planes::PlaneId, JetBridge};
use jet_protocol::Event;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::PathBuf, sync::Mutex};
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

/// Event sequences are Plane-local (ADR-0069), so the high-water mark that
/// prevents replay is kept per Plane.
#[derive(Default)]
struct NotificationGate {
    preferences: NotificationPreferences,
    cursors: HashMap<PlaneId, u64>,
    error: Option<&'static str>,
}
impl NotificationGate {
    fn fence(&mut self, plane: PlaneId, cursor: u64) {
        let fence = self.cursors.entry(plane).or_default();
        *fence = (*fence).max(cursor);
    }

    fn consume(
        &mut self,
        plane: PlaneId,
        sequence: u64,
        signal: Option<NotificationSignal>,
    ) -> Option<NotificationSignal> {
        let Some(cursor) = self.cursors.get_mut(&plane) else {
            // A Plane that was never fenced never notifies: its first event
            // becomes its fence instead of replaying its history.
            self.cursors.insert(plane, sequence);
            return None;
        };
        if sequence <= *cursor {
            return None;
        }
        *cursor = sequence;
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
    pub(crate) fn fence(&self, plane: PlaneId, cursor: u64) {
        if let Ok(mut gate) = self.gate.lock() {
            gate.fence(plane, cursor);
        }
    }
    /// `plane_label` is `Some` only when several Planes are registered, so
    /// the copy names the Plane exactly when that disambiguates it.
    pub(crate) fn observe(
        &self,
        app: &AppHandle,
        plane: PlaneId,
        plane_label: Option<&str>,
        sequence: u64,
        signal: Option<NotificationSignal>,
    ) {
        let Ok(mut gate) = self.gate.lock() else {
            return;
        };
        if let Some(signal) = gate.consume(plane, sequence, signal) {
            // Fixed copy cannot disclose prompts, paths, tools or credentials on
            // the lock screen. Native events, not webview claims, select it.
            // The label is the bounded native Plane label, never event text.
            if app
                .notification()
                .builder()
                .title(title(plane_label))
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

fn title(plane_label: Option<&str>) -> String {
    match plane_label {
        Some(label) => format!("Jet on {label}"),
        None => "Jet".into(),
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
    // Enabling starts every reachable Plane at a fresh authoritative cursor.
    // A Plane that cannot be read now fences at its first feed connect, so it
    // never replays history. Turning off remains possible offline, and
    // preferences never grant any daemon authority.
    let fences = if preferences.enabled {
        read_fences(&bridge).await?
    } else {
        Vec::new()
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
    for (plane, cursor) in fences {
        gate.fence(plane, cursor);
    }
    Ok(NotificationSettings {
        preferences,
        error: None,
    })
}

/// Reads the current cursor of the local Plane and every online remote Plane
/// in parallel. Enabling fails only when a Plane was online and none of the
/// reads succeeded.
async fn read_fences(bridge: &JetBridge) -> Result<Vec<(PlaneId, u64)>, PublicError> {
    let mut reads = tokio::task::JoinSet::new();
    for (plane, client, online) in bridge.planes.fence_targets() {
        reads.spawn(async move { (plane, online, client.status().await) });
    }
    let mut fences = Vec::new();
    let mut failure = None;
    while let Some(read) = reads.join_next().await {
        let Ok((plane, online, result)) = read else {
            continue;
        };
        match result {
            Ok(status) => {
                bridge.planes.observe_status(plane, &status);
                fences.push((plane, status.cursor.unwrap_or_default()));
            }
            Err(error) if online => {
                failure.get_or_insert_with(|| {
                    bridge
                        .planes
                        .settle(plane, PublicError::from_client(&error))
                });
            }
            Err(_) => {}
        }
    }
    match failure {
        Some(error) if fences.is_empty() => Err(error),
        _ => Ok(fences),
    }
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
        let local = PlaneId::Local;
        let mut gate = NotificationGate::default();
        gate.fence(local, 0);
        assert_eq!(
            gate.consume(local, 10, Some(NotificationSignal::Approval)),
            None
        );
        gate.preferences = NotificationPreferences {
            enabled: true,
            approvals: true,
            completion: true,
            failure: false,
        };
        assert_eq!(
            gate.consume(local, 10, Some(NotificationSignal::Approval)),
            None
        );
        assert_eq!(
            gate.consume(local, 11, Some(NotificationSignal::Approval)),
            Some(NotificationSignal::Approval)
        );
        assert_eq!(
            gate.consume(local, 11, Some(NotificationSignal::Approval)),
            None
        );
        assert_eq!(
            gate.consume(local, 12, Some(NotificationSignal::Failed)),
            None
        );
        assert_eq!(
            gate.consume(local, 13, Some(NotificationSignal::Completed)),
            Some(NotificationSignal::Completed)
        );
        gate.preferences.enabled = false;
        assert_eq!(
            gate.consume(local, 14, Some(NotificationSignal::Completed)),
            None
        );
    }

    fn enabled_gate() -> NotificationGate {
        NotificationGate {
            preferences: NotificationPreferences {
                enabled: true,
                approvals: true,
                completion: true,
                failure: true,
            },
            ..Default::default()
        }
    }

    #[test]
    fn planes_with_overlapping_sequences_both_notify() {
        let remote = PlaneId::Remote(uuid::Uuid::from_u128(2));
        let mut gate = enabled_gate();
        gate.fence(PlaneId::Local, 40);
        gate.fence(remote, 5);
        // Plane-local sequences overlap: a high local cursor must not
        // suppress the remote Plane, and the reverse.
        assert_eq!(
            gate.consume(remote, 6, Some(NotificationSignal::Approval)),
            Some(NotificationSignal::Approval)
        );
        assert_eq!(
            gate.consume(PlaneId::Local, 41, Some(NotificationSignal::Completed)),
            Some(NotificationSignal::Completed)
        );
        assert_eq!(
            gate.consume(remote, 6, Some(NotificationSignal::Approval)),
            None
        );
        assert_eq!(
            gate.consume(remote, 7, Some(NotificationSignal::Failed)),
            Some(NotificationSignal::Failed)
        );
    }

    #[test]
    fn a_plane_offline_at_enable_is_fenced_on_connect_and_never_replays() {
        let remote = PlaneId::Remote(uuid::Uuid::from_u128(2));
        let mut gate = enabled_gate();
        gate.fence(PlaneId::Local, 3);
        // The remote Plane connects later at cursor 50: older events replayed
        // from its feed stay silent.
        gate.fence(remote, 50);
        assert_eq!(
            gate.consume(remote, 12, Some(NotificationSignal::Approval)),
            None
        );
        assert_eq!(
            gate.consume(remote, 50, Some(NotificationSignal::Approval)),
            None
        );
        assert_eq!(
            gate.consume(remote, 51, Some(NotificationSignal::Approval)),
            Some(NotificationSignal::Approval)
        );
        // A Plane that somehow streams before any fence never notifies its
        // first event; that event becomes its fence.
        let unfenced = PlaneId::Remote(uuid::Uuid::from_u128(3));
        assert_eq!(
            gate.consume(unfenced, 9, Some(NotificationSignal::Approval)),
            None
        );
        assert_eq!(
            gate.consume(unfenced, 10, Some(NotificationSignal::Approval)),
            Some(NotificationSignal::Approval)
        );
    }

    #[test]
    fn notification_title_names_the_plane_only_when_asked() {
        assert_eq!(title(None), "Jet");
        assert_eq!(title(Some("This computer")), "Jet on This computer");
    }
}
