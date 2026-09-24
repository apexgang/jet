//! Native opt-in notifications: no prompt content, no replay, no webview sender.
use super::{
    errors::PublicError, planes::PlaneId, preferences::write_private_atomically, JetBridge,
};
use jet_protocol::Event;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Mutex,
};
use tauri::{AppHandle, State};
use tauri_plugin_notification::NotificationExt;

const PREFERENCES_FILE: &str = "notification-preferences-v2.json";
/// Wave 2 preferences, read once when no v2 file exists yet.
const LEGACY_PREFERENCES_FILE: &str = "notification-preferences.json";
const MAX_PREFERENCES_BYTES: u64 = 8 * 1024;
const MAX_LEGACY_PREFERENCES_BYTES: u64 = 1024;
/// Upper bound on muted Planes; far above the registry's own capacity.
const MAX_MUTED_PLANES: usize = 64;

/// Device-local notification routing. `muted_planes` holds opaque Plane
/// handles (`local` or a registry UUID) whose signals stay silent here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NotificationPreferences {
    enabled: bool,
    approvals: bool,
    completion: bool,
    failure: bool,
    #[serde(with = "plane_handles")]
    muted_planes: Vec<PlaneId>,
}

/// The v1 file shape (`notification-preferences.json`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyPreferences {
    enabled: bool,
    approvals: bool,
    completion: bool,
    failure: bool,
}

impl From<LegacyPreferences> for NotificationPreferences {
    fn from(legacy: LegacyPreferences) -> Self {
        Self {
            enabled: legacy.enabled,
            approvals: legacy.approvals,
            completion: legacy.completion,
            failure: legacy.failure,
            muted_planes: Vec::new(),
        }
    }
}

impl NotificationPreferences {
    /// Keeps each registered Plane once, in order. A handle the registry no
    /// longer knows (a forgotten Plane) is dropped rather than refused: it
    /// can never be routed again.
    fn pruned(mut self, registered: impl Fn(PlaneId) -> bool) -> Self {
        let mut kept = Vec::with_capacity(self.muted_planes.len());
        for plane in self.muted_planes {
            if registered(plane) && !kept.contains(&plane) {
                kept.push(plane);
            }
        }
        self.muted_planes = kept;
        self
    }

    fn is_muted(&self, plane: PlaneId) -> bool {
        self.muted_planes.contains(&plane)
    }
}

/// `PlaneId` crosses the boundary only as its canonical text form.
mod plane_handles {
    use super::{PlaneId, MAX_MUTED_PLANES};
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(
        planes: &[PlaneId],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(planes.iter().map(PlaneId::to_string))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<PlaneId>, D::Error> {
        let values = Vec::<String>::deserialize(deserializer)?;
        if values.len() > MAX_MUTED_PLANES {
            return Err(D::Error::custom("too many muted Planes"));
        }
        values
            .iter()
            .map(|value| {
                PlaneId::parse(value).map_err(|_| D::Error::custom("invalid Plane handle"))
            })
            .collect()
    }
}

fn preferences_invalid() -> PublicError {
    PublicError::invalid_input(
        "preferences.invalid",
        "Those notification preferences are not valid.",
    )
}

/// Parses webview input into exact v2 preferences.
fn parse(value: serde_json::Value) -> Result<NotificationPreferences, PublicError> {
    serde_json::from_value(value).map_err(|_| preferences_invalid())
}

fn read_bounded(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    Ok(bytes)
}

/// The v2 file, or the v1 file only when no v2 file exists yet. Anything
/// unreadable falls back to the defaults (notifications off).
fn load(directory: &Path, registered: impl Fn(PlaneId) -> bool) -> NotificationPreferences {
    let stored = match read_bounded(&directory.join(PREFERENCES_FILE), MAX_PREFERENCES_BYTES) {
        Ok(bytes) => serde_json::from_slice::<NotificationPreferences>(&bytes).ok(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => read_bounded(
            &directory.join(LEGACY_PREFERENCES_FILE),
            MAX_LEGACY_PREFERENCES_BYTES,
        )
        .ok()
        .and_then(|bytes| serde_json::from_slice::<LegacyPreferences>(&bytes).ok())
        .map(NotificationPreferences::from),
        Err(_) => None,
    };
    stored.unwrap_or_default().pruned(registered)
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
        // The cursor advances even while muted, so unmuting a Plane never
        // replays what it did in the meantime.
        *cursor = sequence;
        if !self.preferences.enabled || self.preferences.is_muted(plane) {
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
    /// Serializes saves so memory and disk agree on the last change. The
    /// file is written on `spawn_blocking` without holding `gate`, so feeds
    /// keep delivering while a save is in progress.
    write: tokio::sync::Mutex<()>,
}
impl NotificationState {
    /// `registered` says whether a Plane handle is still in the registry;
    /// muted handles it does not know are pruned.
    pub(crate) fn new(directory: &Path, registered: impl Fn(PlaneId) -> bool) -> Self {
        Self {
            path: directory.join(PREFERENCES_FILE),
            gate: Mutex::new(NotificationGate {
                preferences: load(directory, registered),
                ..Default::default()
            }),
            write: tokio::sync::Mutex::new(()),
        }
    }

    /// The preferences in effect: loaded at launch or last saved.
    #[cfg(test)]
    pub(crate) fn preferences(&self) -> NotificationPreferences {
        self.gate
            .lock()
            .map(|gate| gate.preferences.clone())
            .unwrap_or_default()
    }

    pub(crate) fn fence(&self, plane: PlaneId, cursor: u64) {
        if let Ok(mut gate) = self.gate.lock() {
            gate.fence(plane, cursor);
        }
    }
    /// Drops a forgotten Plane's fence and routing. If it is added again it
    /// is a new Plane handle, fenced at its first connect. The file keeps the
    /// stale handle until the next save; loading prunes it.
    pub(crate) fn forget(&self, plane: PlaneId) {
        if let Ok(mut gate) = self.gate.lock() {
            gate.cursors.remove(&plane);
            gate.preferences
                .muted_planes
                .retain(|muted| *muted != plane);
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

    async fn save(
        &self,
        preferences: NotificationPreferences,
        fences: Vec<(PlaneId, u64)>,
    ) -> Result<(), PublicError> {
        self.save_with(preferences, fences, |path, bytes| {
            write_private_atomically(&path, &bytes)
        })
        .await
    }

    /// Writes the file first, then publishes the preferences and fences to
    /// the gate in one short critical section.
    async fn save_with<W>(
        &self,
        preferences: NotificationPreferences,
        fences: Vec<(PlaneId, u64)>,
        write: W,
    ) -> Result<(), PublicError>
    where
        W: FnOnce(PathBuf, Vec<u8>) -> io::Result<()> + Send + 'static,
    {
        let _write = self.write.lock().await;
        let bytes = serde_json::to_vec(&preferences).map_err(|_| PublicError::internal())?;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || write(path, bytes))
            .await
            .map_err(|_| PublicError::internal())?
            .map_err(|_| PublicError::internal())?;
        let mut gate = self.gate.lock().map_err(|_| PublicError::internal())?;
        gate.preferences = preferences;
        gate.error = None;
        for (plane, cursor) in fences {
            gate.fence(plane, cursor);
        }
        Ok(())
    }

    /// `planes` is the registry's current list; routing names only those.
    fn view(&self, planes: Vec<(PlaneId, String)>) -> Result<NotificationSettings, PublicError> {
        let gate = self.gate.lock().map_err(|_| PublicError::internal())?;
        let preferences = gate
            .preferences
            .clone()
            .pruned(|plane| planes.iter().any(|(known, _)| *known == plane));
        Ok(NotificationSettings {
            preferences,
            error: gate.error,
            planes: planes
                .into_iter()
                .map(|(plane, label)| NotificationPlaneView {
                    plane_id: plane.to_string(),
                    label,
                })
                .collect(),
        })
    }
}

fn title(plane_label: Option<&str>) -> String {
    match plane_label {
        Some(label) => format!("Jet on {label}"),
        None => "Jet".into(),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationPlaneView {
    plane_id: String,
    label: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationSettings {
    preferences: NotificationPreferences,
    error: Option<&'static str>,
    /// Registered Planes a notification can come from. Native, no connect.
    planes: Vec<NotificationPlaneView>,
}

/// Only takes locks; never connects.
#[tauri::command]
pub(crate) fn load_notification_settings(
    bridge: State<'_, JetBridge>,
) -> Result<NotificationSettings, PublicError> {
    bridge.notifications.view(bridge.planes.labels())
}

#[tauri::command]
pub(crate) async fn set_notification_settings(
    app: AppHandle,
    bridge: State<'_, JetBridge>,
    preferences: serde_json::Value,
) -> Result<NotificationSettings, PublicError> {
    let preferences = parse(preferences)?.pruned(|plane| bridge.planes.contains(plane));
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
    bridge.notifications.save(preferences, fences).await?;
    bridge.notifications.view(bridge.planes.labels())
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
            muted_planes: Vec::new(),
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
                muted_planes: Vec::new(),
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

    #[test]
    fn a_muted_plane_is_silent_and_never_replays_after_unmuting() {
        let remote = PlaneId::Remote(uuid::Uuid::from_u128(2));
        let mut gate = enabled_gate();
        gate.fence(PlaneId::Local, 0);
        gate.fence(remote, 0);
        gate.preferences.muted_planes = vec![remote];
        assert_eq!(
            gate.consume(remote, 1, Some(NotificationSignal::Approval)),
            None
        );
        assert_eq!(
            gate.consume(PlaneId::Local, 1, Some(NotificationSignal::Approval)),
            Some(NotificationSignal::Approval)
        );
        gate.preferences.muted_planes.clear();
        // The muted event advanced the fence: it is not delivered late.
        assert_eq!(
            gate.consume(remote, 1, Some(NotificationSignal::Approval)),
            None
        );
        assert_eq!(
            gate.consume(remote, 2, Some(NotificationSignal::Approval)),
            Some(NotificationSignal::Approval)
        );
    }

    #[test]
    fn preferences_input_is_exact_and_bounded() {
        let remote = uuid::Uuid::from_u128(0xabcdef).to_string();
        let parsed = parse(serde_json::json!({
            "enabled": true, "approvals": true, "completion": false, "failure": true,
            "mutedPlanes": ["local", remote]
        }))
        .unwrap();
        assert_eq!(
            parsed.muted_planes,
            vec![
                PlaneId::Local,
                PlaneId::Remote(uuid::Uuid::from_u128(0xabcdef))
            ]
        );
        assert_eq!(
            serde_json::to_value(&parsed).unwrap(),
            serde_json::json!({
                "enabled": true, "approvals": true, "completion": false, "failure": true,
                "mutedPlanes": ["local", remote]
            })
        );
        let too_many: Vec<String> = (0..=MAX_MUTED_PLANES as u128)
            .map(|n| uuid::Uuid::from_u128(n + 1).to_string())
            .collect();
        for invalid in [
            // v1 shape: mutedPlanes is required on input.
            serde_json::json!({"enabled": true, "approvals": true, "completion": true, "failure": true}),
            serde_json::json!({"enabled": true, "approvals": true, "completion": true, "failure": true, "mutedPlanes": [], "sound": true}),
            serde_json::json!({"enabled": true, "approvals": true, "completion": true, "failure": true, "mutedPlanes": ["LOCAL"]}),
            serde_json::json!({"enabled": true, "approvals": true, "completion": true, "failure": true, "mutedPlanes": ["/run/jetd.sock"]}),
            serde_json::json!({"enabled": true, "approvals": true, "completion": true, "failure": true, "mutedPlanes": [remote.to_uppercase()]}),
            serde_json::json!({"enabled": true, "approvals": true, "completion": true, "failure": true, "mutedPlanes": too_many}),
        ] {
            assert_eq!(parse(invalid).unwrap_err().code, "preferences.invalid");
        }
    }

    #[test]
    fn muted_planes_are_pruned_and_deduplicated() {
        let known = PlaneId::Remote(uuid::Uuid::from_u128(1));
        let forgotten = PlaneId::Remote(uuid::Uuid::from_u128(2));
        let preferences = NotificationPreferences {
            muted_planes: vec![known, forgotten, PlaneId::Local, known],
            ..Default::default()
        }
        .pruned(|plane| plane != forgotten);
        assert_eq!(preferences.muted_planes, vec![known, PlaneId::Local]);
    }

    fn write_file(directory: &Path, name: &str, contents: &str) {
        fs::write(directory.join(name), contents).unwrap();
    }

    fn stored(state: &NotificationState) -> NotificationPreferences {
        state.gate.lock().unwrap().preferences.clone()
    }

    #[test]
    fn v1_preferences_migrate_with_no_muted_planes() {
        let directory = tempfile::tempdir().unwrap();
        write_file(
            directory.path(),
            LEGACY_PREFERENCES_FILE,
            r#"{"enabled":true,"approvals":false,"completion":true,"failure":true}"#,
        );
        let state = NotificationState::new(directory.path(), |_| true);
        assert_eq!(
            stored(&state),
            NotificationPreferences {
                enabled: true,
                approvals: false,
                completion: true,
                failure: true,
                muted_planes: Vec::new(),
            }
        );
        // Once a v2 file exists, v1 is no longer consulted.
        write_file(
            directory.path(),
            PREFERENCES_FILE,
            r#"{"enabled":false,"approvals":true,"completion":true,"failure":true,"mutedPlanes":["local"]}"#,
        );
        let state = NotificationState::new(directory.path(), |_| true);
        assert!(!stored(&state).enabled);
        assert_eq!(stored(&state).muted_planes, vec![PlaneId::Local]);
        // An unreadable v2 file means defaults, not a v1 resurrection.
        write_file(directory.path(), PREFERENCES_FILE, "{");
        let state = NotificationState::new(directory.path(), |_| true);
        assert_eq!(stored(&state), NotificationPreferences::default());
    }

    #[test]
    fn unknown_plane_ids_are_pruned_on_load() {
        let directory = tempfile::tempdir().unwrap();
        let known = uuid::Uuid::from_u128(1);
        let forgotten = uuid::Uuid::from_u128(2);
        write_file(
            directory.path(),
            PREFERENCES_FILE,
            &format!(
                r#"{{"enabled":true,"approvals":true,"completion":true,"failure":true,"mutedPlanes":["{known}","{forgotten}","local"]}}"#
            ),
        );
        let state = NotificationState::new(directory.path(), |plane| {
            plane != PlaneId::Remote(forgotten)
        });
        assert_eq!(
            stored(&state).muted_planes,
            vec![PlaneId::Remote(known), PlaneId::Local]
        );
        // The view names only registered Planes.
        let view = state
            .view(vec![(PlaneId::Local, "This computer".into())])
            .unwrap();
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(
            json["preferences"]["mutedPlanes"],
            serde_json::json!(["local"])
        );
        assert_eq!(
            json["planes"],
            serde_json::json!([{"planeId": "local", "label": "This computer"}])
        );
    }

    #[tokio::test]
    async fn saving_writes_outside_the_gate_lock_owner_only() {
        let directory = tempfile::tempdir().unwrap();
        let state = std::sync::Arc::new(NotificationState::new(directory.path(), |_| true));
        let observer = std::sync::Arc::clone(&state);
        let unlocked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = std::sync::Arc::clone(&unlocked);
        let preferences = NotificationPreferences {
            enabled: true,
            approvals: true,
            completion: false,
            failure: true,
            muted_planes: vec![PlaneId::Local],
        };
        state
            .save_with(
                preferences.clone(),
                vec![(PlaneId::Local, 9)],
                move |path, bytes| {
                    // A feed can still consume events while the file is written.
                    seen.store(
                        observer.gate.try_lock().is_ok(),
                        std::sync::atomic::Ordering::SeqCst,
                    );
                    write_private_atomically(&path, &bytes)
                },
            )
            .await
            .unwrap();
        assert!(unlocked.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(stored(&state), preferences);
        assert_eq!(state.gate.lock().unwrap().cursors[&PlaneId::Local], 9);
        assert_eq!(
            stored(&NotificationState::new(directory.path(), |_| true)),
            preferences
        );
        let path = directory.path().join(PREFERENCES_FILE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let leftovers = fs::read_dir(directory.path())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0);
    }

    #[tokio::test]
    async fn a_failed_write_changes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let state = NotificationState::new(directory.path(), |_| true);
        let error = state
            .save_with(
                NotificationPreferences {
                    enabled: true,
                    ..Default::default()
                },
                vec![(PlaneId::Local, 4)],
                |_, _| Err(io::Error::other("disk full")),
            )
            .await
            .unwrap_err();
        assert_eq!(error.category, "internal");
        assert_eq!(stored(&state), NotificationPreferences::default());
        assert!(state.gate.lock().unwrap().cursors.is_empty());
    }

    #[test]
    fn forgetting_a_plane_drops_its_routing() {
        let directory = tempfile::tempdir().unwrap();
        let state = NotificationState::new(directory.path(), |_| true);
        let remote = PlaneId::Remote(uuid::Uuid::from_u128(3));
        {
            let mut gate = state.gate.lock().unwrap();
            gate.preferences.muted_planes = vec![remote, PlaneId::Local];
            gate.fence(remote, 5);
        }
        state.forget(remote);
        assert_eq!(stored(&state).muted_planes, vec![PlaneId::Local]);
        assert!(!state.gate.lock().unwrap().cursors.contains_key(&remote));
    }
}
