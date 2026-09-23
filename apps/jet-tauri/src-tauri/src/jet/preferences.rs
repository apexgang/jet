//! Device-local desktop preferences. They are presentation choices of this
//! computer, never Plane policy, and never grant daemon authority.

use std::{
    fs,
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
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self {
            reopen_last_task: true,
        }
    }
}

fn preferences_invalid() -> PublicError {
    PublicError::invalid_input(
        "preferences.invalid",
        "Those desktop preferences are not valid.",
    )
}

fn parse(value: serde_json::Value) -> Result<DesktopPreferences, PublicError> {
    serde_json::from_value(value).map_err(|_| preferences_invalid())
}

/// Stored preferences, or the defaults when the file is missing, oversized
/// or unreadable.
fn load(path: &Path) -> DesktopPreferences {
    let mut bytes = Vec::new();
    let read = fs::File::open(path)
        .and_then(|file| file.take(MAX_PREFERENCES_BYTES + 1).read_to_end(&mut bytes));
    if read.is_err() || bytes.len() as u64 > MAX_PREFERENCES_BYTES {
        return DesktopPreferences::default();
    }
    serde_json::from_slice(&bytes).unwrap_or_default()
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

    async fn set(
        &self,
        preferences: DesktopPreferences,
    ) -> Result<DesktopPreferences, PublicError> {
        let _write = self.write.lock().await;
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

#[tauri::command]
pub(crate) async fn set_desktop_preferences(
    bridge: State<'_, JetBridge>,
    preferences: serde_json::Value,
) -> Result<DesktopPreferences, PublicError> {
    let preferences = parse(preferences)?;
    bridge.preferences.set(preferences).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preferences_are_exact_and_camel_case() {
        assert_eq!(
            parse(json!({"reopenLastTask": false})).unwrap(),
            DesktopPreferences {
                reopen_last_task: false
            }
        );
        for invalid in [
            json!({"reopen_last_task": false}),
            json!({"reopenLastTask": false, "theme": "dark"}),
            json!({"reopenLastTask": "no"}),
            json!({}),
        ] {
            assert_eq!(parse(invalid).unwrap_err().code, "preferences.invalid");
        }
        assert_eq!(
            serde_json::to_value(DesktopPreferences::default()).unwrap(),
            json!({"reopenLastTask": true})
        );
    }

    #[tokio::test]
    async fn preferences_persist_atomically_and_owner_only() {
        let directory = tempfile::tempdir().unwrap();
        let state = PreferencesState::new(directory.path());
        assert!(state.reopen_last_task().unwrap());

        state
            .set(DesktopPreferences {
                reopen_last_task: false,
            })
            .await
            .unwrap();
        assert!(!state.reopen_last_task().unwrap());
        assert!(!PreferencesState::new(directory.path())
            .reopen_last_task()
            .unwrap());

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
