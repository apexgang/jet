//! The separate Settings window: typed deep links, the last pane, and the
//! window lifecycle. The Settings webview has its own capability and never
//! receives agent content (wave 3.2 §4.1).

use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use serde::{Deserialize, Serialize};
use tauri::{ipc::Channel, AppHandle, Manager, State, WebviewUrl, WebviewWindow};
use uuid::Uuid;

use super::{errors::PublicError, settings, JetBridge};

pub(crate) const SETTINGS_LABEL: &str = "settings";
const SETTINGS_FILE: &str = "settings-window.json";
const MAX_SETTINGS_FILE_BYTES: u64 = 256;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SettingsPane {
    #[default]
    General,
    Agents,
    Work,
    Connections,
    Safety,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SettingsSection {
    Appearance,
    Notifications,
    Restoration,
    Harnesses,
    Extensions,
    Accounts,
    Usage,
    Utility,
    Projects,
    Delivery,
    Reviews,
    Schedules,
    Retention,
    LocalService,
    Planes,
    Execution,
    Permissions,
    Storage,
    Audit,
}

impl SettingsSection {
    pub(crate) fn pane(self) -> SettingsPane {
        match self {
            Self::Appearance | Self::Notifications | Self::Restoration => SettingsPane::General,
            Self::Harnesses | Self::Extensions | Self::Accounts | Self::Usage | Self::Utility => {
                SettingsPane::Agents
            }
            Self::Projects | Self::Delivery | Self::Reviews | Self::Schedules | Self::Retention => {
                SettingsPane::Work
            }
            Self::LocalService | Self::Planes => SettingsPane::Connections,
            Self::Execution | Self::Permissions | Self::Storage | Self::Audit => {
                SettingsPane::Safety
            }
        }
    }
}

/// A typed deep link. It is never a URL or a string the webview built.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettingsTarget {
    pane: SettingsPane,
    #[serde(default)]
    section: Option<SettingsSection>,
    #[serde(default)]
    plane_id: Option<String>,
}

/// What `settings-window.json` holds. The Plane is never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RememberedPane {
    pane: SettingsPane,
    #[serde(default)]
    section: Option<SettingsSection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsNavigation {
    generation: String,
    target: SettingsTarget,
}

fn target_invalid() -> PublicError {
    PublicError::invalid_input(
        "settings.target_invalid",
        "That Settings location does not exist.",
    )
}

/// Parses and validates a webview target. A section must belong to its
/// pane. A Plane `plane_known` rejects is dropped, so the Settings window
/// falls back to its default Plane instead of failing.
fn parse_target(
    value: serde_json::Value,
    plane_known: impl Fn(&str) -> bool,
) -> Result<SettingsTarget, PublicError> {
    let mut target: SettingsTarget = serde_json::from_value(value).map_err(|_| target_invalid())?;
    if target
        .section
        .is_some_and(|section| section.pane() != target.pane)
    {
        return Err(target_invalid());
    }
    if target
        .plane_id
        .as_deref()
        .is_some_and(|plane_id| !plane_known(plane_id))
    {
        target.plane_id = None;
    }
    Ok(target)
}

/// The persisted pane, or General when the file is missing, oversized,
/// corrupt or names a section outside its pane.
fn load_remembered(path: &Path) -> SettingsTarget {
    let mut bytes = Vec::new();
    let read = fs::File::open(path).and_then(|file| {
        file.take(MAX_SETTINGS_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
    });
    if read.is_err() || bytes.len() as u64 > MAX_SETTINGS_FILE_BYTES {
        return SettingsTarget::default();
    }
    match serde_json::from_slice::<RememberedPane>(&bytes) {
        Ok(remembered)
            if remembered
                .section
                .is_none_or(|section| section.pane() == remembered.pane) =>
        {
            SettingsTarget {
                pane: remembered.pane,
                section: remembered.section,
                plane_id: None,
            }
        }
        _ => SettingsTarget::default(),
    }
}

/// Owner-only, fsynced, atomic replace (the `identity.rs` pattern).
fn write_remembered(path: &Path, remembered: RememberedPane) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(&remembered).map_err(std::io::Error::other)?;
    if bytes.len() as u64 > MAX_SETTINGS_FILE_BYTES {
        return Err(std::io::Error::other("settings window state is oversized"));
    }
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let temporary = directory.join(format!(".{SETTINGS_FILE}.{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let result = options.open(&temporary).and_then(|mut file| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    });
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) struct SettingsWindowState {
    path: PathBuf,
    last: Mutex<SettingsTarget>,
    /// Set by `open_settings`, consumed by the window's navigation watcher.
    pending: Mutex<Option<SettingsTarget>>,
    /// The one navigation watcher: the Settings window's channel.
    navigation: Mutex<Option<Channel<SettingsNavigation>>>,
    /// One number per navigation message, the initial reply included. The
    /// reply and Channel messages travel separately, so the window keeps
    /// only the newest number it has seen.
    generation: AtomicU64,
    /// Serializes file writes so the last remembered pane is the one on disk.
    write: tokio::sync::Mutex<()>,
}

impl SettingsWindowState {
    pub(crate) fn new(app_data_directory: &Path) -> Self {
        let path = app_data_directory.join(SETTINGS_FILE);
        let last = load_remembered(&path);
        Self {
            path,
            last: Mutex::new(last),
            pending: Mutex::new(None),
            navigation: Mutex::new(None),
            generation: AtomicU64::new(0),
            write: tokio::sync::Mutex::new(()),
        }
    }

    fn set_pending(&self, target: SettingsTarget) -> Result<(), PublicError> {
        *self.pending.lock().map_err(|_| PublicError::internal())? = Some(target);
        Ok(())
    }

    fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Numbers the watcher's first message and returns the target the window
    /// should show first: a pending deep link exactly once, else the last pane.
    fn begin_watch(&self) -> Result<(u64, SettingsTarget), PublicError> {
        let generation = self.next_generation();
        let pending = self
            .pending
            .lock()
            .map_err(|_| PublicError::internal())?
            .take();
        let target = match pending {
            Some(target) => target,
            None => self
                .last
                .lock()
                .map_err(|_| PublicError::internal())?
                .clone(),
        };
        Ok((generation, target))
    }

    fn watch(
        &self,
        channel: Channel<SettingsNavigation>,
    ) -> Result<SettingsNavigation, PublicError> {
        let (generation, target) = self.begin_watch()?;
        *self
            .navigation
            .lock()
            .map_err(|_| PublicError::internal())? = Some(channel);
        Ok(SettingsNavigation {
            generation: generation.to_string(),
            target,
        })
    }

    /// Sends a pending deep link to a registered watcher. Without a watcher
    /// (the window is still loading) the target stays pending.
    fn deliver_pending(&self) -> Result<(), PublicError> {
        let mut navigation = self
            .navigation
            .lock()
            .map_err(|_| PublicError::internal())?;
        let Some(channel) = navigation.as_ref() else {
            return Ok(());
        };
        let mut pending = self.pending.lock().map_err(|_| PublicError::internal())?;
        let Some(target) = pending.take() else {
            return Ok(());
        };
        // Numbered after the watcher's initial reply, so a link that
        // arrives first is never replaced by that older reply.
        let message = SettingsNavigation {
            generation: self.next_generation().to_string(),
            target: target.clone(),
        };
        if channel.send(message).is_err() {
            // The webview went away: keep the link for its next watcher.
            *pending = Some(target);
            *navigation = None;
        }
        Ok(())
    }

    fn remember(&self, target: &SettingsTarget) -> Result<RememberedPane, PublicError> {
        let remembered = RememberedPane {
            pane: target.pane,
            section: target.section,
        };
        *self.last.lock().map_err(|_| PublicError::internal())? = SettingsTarget {
            pane: remembered.pane,
            section: remembered.section,
            plane_id: None,
        };
        Ok(remembered)
    }

    /// The Settings window was destroyed: its channel is gone.
    pub(crate) fn window_destroyed(&self) {
        if let Ok(mut navigation) = self.navigation.lock() {
            *navigation = None;
        }
    }
}

/// Opens or focuses the Settings window, optionally at a deep link.
/// Async because building a window from a synchronous command can deadlock.
#[tauri::command]
pub(crate) async fn open_settings(
    app: AppHandle,
    bridge: State<'_, JetBridge>,
    target: Option<serde_json::Value>,
) -> Result<(), PublicError> {
    let target = target
        .map(|value| {
            parse_target(value, |plane_id| {
                settings::plane_client(&bridge, plane_id).is_ok()
            })
        })
        .transpose()?;
    let state = &bridge.settings_window;
    if let Some(target) = target {
        state.set_pending(target)?;
    }
    if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
        // Focus failures are cosmetic; the link is still delivered.
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return state.deliver_pending();
    }
    tauri::WebviewWindowBuilder::new(&app, SETTINGS_LABEL, WebviewUrl::App("settings".into()))
        .title("Jet Settings")
        .inner_size(820.0, 620.0)
        .min_inner_size(640.0, 480.0)
        .build()
        .map_err(|_| PublicError::internal())?;
    Ok(())
}

/// Registers the Settings window's navigation channel, replacing any earlier
/// one, and returns the target to show first.
#[tauri::command]
pub(crate) fn watch_settings_navigation(
    window: WebviewWindow,
    bridge: State<'_, JetBridge>,
    on_navigate: Channel<SettingsNavigation>,
) -> Result<SettingsNavigation, PublicError> {
    // The capability already scopes this to the Settings window.
    if window.label() != SETTINGS_LABEL {
        return Err(target_invalid());
    }
    bridge.settings_window.watch(on_navigate)
}

/// Remembers the pane the user chose, natively and without its Plane.
#[tauri::command]
pub(crate) async fn remember_settings_pane(
    bridge: State<'_, JetBridge>,
    target: serde_json::Value,
) -> Result<(), PublicError> {
    let target = parse_target(target, |_| false)?;
    let state = &bridge.settings_window;
    let _write = state.write.lock().await;
    let remembered = state.remember(&target)?;
    let path = state.path.clone();
    tokio::task::spawn_blocking(move || write_remembered(&path, remembered))
        .await
        .map_err(|_| PublicError::internal())?
        .map_err(|_| PublicError::internal())
}

/// Closes the calling window only when it is the Settings window. Narrower
/// than granting `core:window:allow-close`.
#[tauri::command]
pub(crate) fn close_settings(window: WebviewWindow) -> Result<(), PublicError> {
    if window.label() == SETTINGS_LABEL {
        window.close().map_err(|_| PublicError::internal())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ALL_SECTIONS: [SettingsSection; 19] = [
        SettingsSection::Appearance,
        SettingsSection::Notifications,
        SettingsSection::Restoration,
        SettingsSection::Harnesses,
        SettingsSection::Extensions,
        SettingsSection::Accounts,
        SettingsSection::Usage,
        SettingsSection::Utility,
        SettingsSection::Projects,
        SettingsSection::Delivery,
        SettingsSection::Reviews,
        SettingsSection::Schedules,
        SettingsSection::Retention,
        SettingsSection::LocalService,
        SettingsSection::Planes,
        SettingsSection::Execution,
        SettingsSection::Permissions,
        SettingsSection::Storage,
        SettingsSection::Audit,
    ];

    #[test]
    fn every_section_belongs_to_exactly_its_pane() {
        let expected = [
            ("appearance", "general"),
            ("notifications", "general"),
            ("restoration", "general"),
            ("harnesses", "agents"),
            ("extensions", "agents"),
            ("accounts", "agents"),
            ("usage", "agents"),
            ("utility", "agents"),
            ("projects", "work"),
            ("delivery", "work"),
            ("reviews", "work"),
            ("schedules", "work"),
            ("retention", "work"),
            ("local_service", "connections"),
            ("planes", "connections"),
            ("execution", "safety"),
            ("permissions", "safety"),
            ("storage", "safety"),
            ("audit", "safety"),
        ];
        for (section, (name, pane)) in ALL_SECTIONS.iter().zip(expected) {
            assert_eq!(serde_json::to_value(section).unwrap(), json!(name));
            assert_eq!(serde_json::to_value(section.pane()).unwrap(), json!(pane));
        }
    }

    #[test]
    fn targets_are_typed_and_checked() {
        let known = |plane_id: &str| plane_id == "local";
        let target = parse_target(
            json!({"pane": "general", "section": "notifications", "plane_id": "local"}),
            known,
        )
        .unwrap();
        assert_eq!(target.section, Some(SettingsSection::Notifications));
        assert_eq!(target.plane_id.as_deref(), Some("local"));

        for invalid in [
            json!({"pane": "general", "url": "https://example.com"}),
            json!({"pane": "general", "section": "accounts"}),
            json!({"pane": "privacy"}),
            json!({"section": "notifications"}),
            json!("general"),
        ] {
            assert_eq!(
                parse_target(invalid, known).unwrap_err().code,
                "settings.target_invalid"
            );
        }

        // An unknown Plane is dropped, never an error.
        let dropped = parse_target(
            json!({"pane": "connections", "plane_id": "00000000-0000-0000-0000-000000000007"}),
            known,
        )
        .unwrap();
        assert_eq!(dropped.plane_id, None);
        assert_eq!(dropped.pane, SettingsPane::Connections);
    }

    #[test]
    fn remembered_pane_round_trips_without_its_plane() {
        let directory = tempfile::tempdir().unwrap();
        let state = SettingsWindowState::new(directory.path());
        let target = SettingsTarget {
            pane: SettingsPane::Connections,
            section: Some(SettingsSection::LocalService),
            plane_id: Some("local".into()),
        };
        let remembered = state.remember(&target).unwrap();
        write_remembered(&state.path, remembered).unwrap();

        let written = fs::read_to_string(directory.path().join(SETTINGS_FILE)).unwrap();
        assert!(!written.contains("plane"), "{written}");
        assert!(!written.contains("local\""), "{written}");

        let reopened = SettingsWindowState::new(directory.path());
        let (_, restored) = reopened.begin_watch().unwrap();
        assert_eq!(
            restored,
            SettingsTarget {
                pane: SettingsPane::Connections,
                section: Some(SettingsSection::LocalService),
                plane_id: None,
            }
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(directory.path().join(SETTINGS_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn oversized_or_corrupt_state_gives_general() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(SETTINGS_FILE);
        let oversized = format!(
            "{{\"pane\":\"work\",\"section\":null{}}}",
            " ".repeat(MAX_SETTINGS_FILE_BYTES as usize)
        );
        for contents in [
            "not json".to_owned(),
            oversized,
            r#"{"pane":"work","section":"accounts"}"#.to_owned(),
            r#"{"pane":"work","plane_id":"local"}"#.to_owned(),
        ] {
            fs::write(&path, contents).unwrap();
            assert_eq!(load_remembered(&path), SettingsTarget::default());
        }
        fs::write(&path, r#"{"pane":"work"}"#).unwrap();
        assert_eq!(load_remembered(&path).pane, SettingsPane::Work);
    }

    #[test]
    fn a_pending_link_is_shown_exactly_once_then_the_last_pane() {
        let directory = tempfile::tempdir().unwrap();
        let state = SettingsWindowState::new(directory.path());
        let link = SettingsTarget {
            pane: SettingsPane::Connections,
            section: Some(SettingsSection::LocalService),
            plane_id: None,
        };
        state.set_pending(link.clone()).unwrap();

        let (first_generation, first) = state.begin_watch().unwrap();
        assert_eq!(first, link);
        let (second_generation, second) = state.begin_watch().unwrap();
        assert_eq!(second, SettingsTarget::default());
        assert!(second_generation > first_generation);
    }

    #[test]
    fn an_open_window_receives_the_link_on_its_channel() {
        use std::sync::{Arc, Mutex};
        use tauri::ipc::InvokeResponseBody;

        let directory = tempfile::tempdir().unwrap();
        let state = SettingsWindowState::new(directory.path());
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = received.clone();
        let channel = Channel::new(move |body| {
            if let InvokeResponseBody::Json(json) = body {
                sink.lock().unwrap().push(json);
            }
            Ok(())
        });
        let initial = state.watch(channel).unwrap();
        assert_eq!(initial.target, SettingsTarget::default());

        // No link pending: nothing is sent.
        state.deliver_pending().unwrap();
        assert!(received.lock().unwrap().is_empty());

        state
            .set_pending(SettingsTarget {
                pane: SettingsPane::General,
                section: Some(SettingsSection::Notifications),
                plane_id: None,
            })
            .unwrap();
        state.deliver_pending().unwrap();
        let sent = received.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        let message: serde_json::Value = serde_json::from_str(&sent[0]).unwrap();
        assert_eq!(
            message["target"],
            json!({"pane": "general", "section": "notifications", "plane_id": null})
        );
        // Newer than the initial reply, whichever reaches the window first.
        let link: u64 = message["generation"].as_str().unwrap().parse().unwrap();
        let first: u64 = initial.generation.parse().unwrap();
        assert!(link > first);
        // Consumed: a later watcher starts at the last pane.
        assert_eq!(state.begin_watch().unwrap().1, SettingsTarget::default());

        state.window_destroyed();
        state
            .set_pending(SettingsTarget {
                pane: SettingsPane::Connections,
                section: None,
                plane_id: None,
            })
            .unwrap();
        state.deliver_pending().unwrap();
        assert_eq!(received.lock().unwrap().len(), 1);
        assert_eq!(
            state.begin_watch().unwrap().1.pane,
            SettingsPane::Connections
        );
    }
}
