//! Device-local desktop preferences. They are presentation choices of this
//! computer, never Plane policy, and never grant daemon authority.

use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{errors::PublicError, local_store, JetBridge};

const PREFERENCES_FILE: &str = "desktop-preferences.json";
const MAX_PREFERENCES_BYTES: u64 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DesktopPreferences {
    /// Reopen the last selected task when Jet starts.
    reopen_last_task: bool,
    /// Check for a newer Jet shortly after launch (`updates.rs`). A privacy
    /// item: the check contacts github.com.
    check_for_updates: bool,
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self {
            reopen_last_task: true,
            check_for_updates: true,
        }
    }
}

/// The Wave 3 file, written before "Check for updates automatically".
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyPreferences {
    reopen_last_task: bool,
}

impl From<LegacyPreferences> for DesktopPreferences {
    fn from(legacy: LegacyPreferences) -> Self {
        Self {
            reopen_last_task: legacy.reopen_last_task,
            ..Self::default()
        }
    }
}

fn preferences_invalid() -> PublicError {
    PublicError::invalid_input(
        "preferences.invalid",
        "Those desktop preferences are not valid.",
    )
}

/// A change to one or both preferences; one left out keeps its stored
/// value. Each pane sends only the choice it shows, so a window holding an
/// older copy cannot undo a choice made elsewhere (General's "Reopen the
/// last task", Versions' "Check for updates automatically").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreferencesChange {
    #[serde(default, deserialize_with = "present")]
    reopen_last_task: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    check_for_updates: Option<bool>,
}

impl PreferencesChange {
    fn apply(self, stored: DesktopPreferences) -> DesktopPreferences {
        DesktopPreferences {
            reopen_last_task: self.reopen_last_task.unwrap_or(stored.reopen_last_task),
            check_for_updates: self.check_for_updates.unwrap_or(stored.check_for_updates),
        }
    }
}

/// A field that is present must be a Boolean; `null` is refused.
fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
    bool::deserialize(deserializer).map(Some)
}

/// An object with at least one known preference, each a Boolean, and
/// nothing else.
fn parse(value: serde_json::Value) -> Result<PreferencesChange, PublicError> {
    if !value.is_object() {
        return Err(preferences_invalid());
    }
    let change: PreferencesChange =
        serde_json::from_value(value).map_err(|_| preferences_invalid())?;
    if change.reopen_last_task.is_none() && change.check_for_updates.is_none() {
        return Err(preferences_invalid());
    }
    Ok(change)
}

/// Stored preferences, or the defaults when the file is missing, oversized
/// or unreadable.
fn load(path: &Path) -> DesktopPreferences {
    let mut bytes = Vec::new();
    let read = super::local_store::open_regular(path)
        .and_then(|file| file.take(MAX_PREFERENCES_BYTES + 1).read_to_end(&mut bytes));
    if read.is_err() || bytes.len() as u64 > MAX_PREFERENCES_BYTES {
        return DesktopPreferences::default();
    }
    serde_json::from_slice(&bytes)
        .or_else(|_| serde_json::from_slice::<LegacyPreferences>(&bytes).map(Into::into))
        .unwrap_or_default()
}

/// Owner-only, fsynced, atomic replace (`local_store`). Shared by every
/// device-local preference file. Blocking: call it on `spawn_blocking`.
pub(super) fn write_private_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "preferences".into());
    local_store::write_private_atomically(directory, &name, bytes)
}

fn write(path: &Path, preferences: DesktopPreferences) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(&preferences).map_err(std::io::Error::other)?;
    write_private_atomically(path, &bytes)
}

pub(crate) struct PreferencesState {
    path: PathBuf,
    current: Mutex<DesktopPreferences>,
    /// Serializes writes so memory and disk agree on the last change. The
    /// file is written without holding `current`.
    write: tokio::sync::Mutex<()>,
}

impl PreferencesState {
    pub(crate) fn new(app_data_directory: &Path) -> Self {
        let path = app_data_directory.join(PREFERENCES_FILE);
        let current = load(&path);
        Self {
            path,
            current: Mutex::new(current),
            write: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) fn current(&self) -> Result<DesktopPreferences, PublicError> {
        self.current
            .lock()
            .map(|current| *current)
            .map_err(|_| PublicError::internal())
    }

    pub(crate) fn reopen_last_task(&self) -> Result<bool, PublicError> {
        Ok(self.current()?.reopen_last_task)
    }

    pub(crate) fn check_for_updates(&self) -> Result<bool, PublicError> {
        Ok(self.current()?.check_for_updates)
    }

    /// Applies `change` to the stored preferences. The write lock is taken
    /// before reading them, so two changes never start from the same copy.
    async fn change(&self, change: PreferencesChange) -> Result<DesktopPreferences, PublicError> {
        let _write = self.write.lock().await;
        let preferences = change.apply(self.current()?);
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || write(&path, preferences))
            .await
            .map_err(|_| PublicError::internal())?
            .map_err(|_| PublicError::internal())?;
        *self.current.lock().map_err(|_| PublicError::internal())? = preferences;
        Ok(preferences)
    }
}

/// Only takes a lock.
#[tauri::command]
pub(crate) fn load_desktop_preferences(
    bridge: State<'_, JetBridge>,
) -> Result<DesktopPreferences, PublicError> {
    bridge.preferences.current()
}

/// Changes the preferences `preferences` names and returns all of them.
#[tauri::command]
pub(crate) async fn set_desktop_preferences(
    bridge: State<'_, JetBridge>,
    preferences: serde_json::Value,
) -> Result<DesktopPreferences, PublicError> {
    let change = parse(preferences)?;
    bridge.preferences.change(change).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn preference_changes_are_exact_and_camel_case() {
        let stored = DesktopPreferences::default();
        for (change, expected) in [
            (
                json!({"reopenLastTask": false, "checkForUpdates": false}),
                (false, false),
            ),
            // The Wave 3 shape: one preference, the other kept.
            (json!({"reopenLastTask": false}), (false, true)),
            (json!({"checkForUpdates": false}), (true, false)),
        ] {
            let applied = parse(change.clone()).unwrap().apply(stored);
            assert_eq!(
                (applied.reopen_last_task, applied.check_for_updates),
                expected,
                "{change}"
            );
        }
        for invalid in [
            json!({"reopen_last_task": false}),
            json!({"reopenLastTask": false, "theme": "dark"}),
            json!({"reopenLastTask": "no"}),
            json!({"checkForUpdates": 1}),
            json!({"reopenLastTask": null}),
            json!({"reopenLastTask": null, "checkForUpdates": null}),
            json!({}),
            json!([true, true]),
        ] {
            assert_eq!(parse(invalid).unwrap_err().code, "preferences.invalid");
        }
        assert_eq!(
            serde_json::to_value(DesktopPreferences::default()).unwrap(),
            json!({"reopenLastTask": true, "checkForUpdates": true})
        );
    }

    #[tokio::test]
    async fn preferences_persist_atomically_and_owner_only() {
        let directory = tempfile::tempdir().unwrap();
        let state = PreferencesState::new(directory.path());
        assert!(state.reopen_last_task().unwrap());

        assert!(state.check_for_updates().unwrap());

        // Each change keeps the other preference as stored, whatever copy
        // the window that sends it last read.
        let saved = state
            .change(parse(json!({"reopenLastTask": false})).unwrap())
            .await
            .unwrap();
        assert_eq!(
            saved,
            DesktopPreferences {
                reopen_last_task: false,
                check_for_updates: true,
            }
        );
        state
            .change(parse(json!({"checkForUpdates": false})).unwrap())
            .await
            .unwrap();
        assert!(!state.reopen_last_task().unwrap());
        assert!(!state.check_for_updates().unwrap());
        let reloaded = PreferencesState::new(directory.path());
        assert!(!reloaded.reopen_last_task().unwrap());
        assert!(!reloaded.check_for_updates().unwrap());

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

    /// A Wave 3 file keeps its choice and gets the new default.
    #[test]
    fn wave_3_preferences_still_load() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(PREFERENCES_FILE);
        fs::write(&path, r#"{"reopenLastTask":false}"#).unwrap();
        assert_eq!(
            load(&path),
            DesktopPreferences {
                reopen_last_task: false,
                check_for_updates: true,
            }
        );
    }

    #[test]
    fn unreadable_preferences_fall_back_to_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(PREFERENCES_FILE);
        for contents in [
            "{".to_owned(),
            r#"{"reopenLastTask":false,"extra":1}"#.to_owned(),
            format!(
                r#"{{"reopenLastTask":false{}}}"#,
                " ".repeat(MAX_PREFERENCES_BYTES as usize)
            ),
        ] {
            fs::write(&path, contents).unwrap();
            assert_eq!(load(&path), DesktopPreferences::default());
        }
    }
}
