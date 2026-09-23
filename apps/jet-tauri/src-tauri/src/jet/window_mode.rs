//! F11, Ctrl+W and Ctrl+Q (wave 3.4 §4.3). Custom commands instead of
//! `core:window:*` grants, so each window's capability stays narrow. Closing
//! the window or quitting never stops Runs: `jetd` owns them.

use serde::Serialize;
use tauri::{AppHandle, Runtime, WebviewWindow};

use super::{errors::PublicError, window_state::MAIN_LABEL};

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WindowModeView {
    fullscreen: bool,
}

/// Only the main window may change its own mode or close through these.
fn ensure_main(label: &str) -> Result<(), PublicError> {
    if label == MAIN_LABEL {
        Ok(())
    } else {
        Err(PublicError::invalid_input(
            "window.not_main",
            "This window can't do that.",
        ))
    }
}

fn mode_unavailable() -> PublicError {
    PublicError::local_unavailable(
        "window.mode_unavailable",
        "Full screen isn't available in this window.",
        false,
    )
}

/// Synchronous: one cheap window call on the main thread. Full screen is
/// session-only and never restored at launch.
#[tauri::command]
pub(crate) fn toggle_main_window_fullscreen<R: Runtime>(
    window: WebviewWindow<R>,
) -> Result<WindowModeView, PublicError> {
    ensure_main(window.label())?;
    let fullscreen = !window.is_fullscreen().map_err(|_| mode_unavailable())?;
    window
        .set_fullscreen(fullscreen)
        .map_err(|_| mode_unavailable())?;
    Ok(WindowModeView { fullscreen })
}

/// Ctrl+W. The window's close hook saves its geometry. With the Settings
/// window still open the app keeps running until that closes too.
#[tauri::command]
pub(crate) fn close_main_window<R: Runtime>(window: WebviewWindow<R>) -> Result<(), PublicError> {
    ensure_main(window.label())?;
    window.close().map_err(|_| {
        PublicError::local_unavailable(
            "window.close_failed",
            "Jet couldn't complete that window action.",
            true,
        )
    })
}

/// Ctrl+Q from either window. The exit hook saves the main window's
/// geometry. An unsent draft is lost, as with the window's close button.
#[tauri::command]
pub(crate) fn quit_jet<R: Runtime>(app: AppHandle<R>) {
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_main_accepts_only_main() {
        assert!(ensure_main("main").is_ok());
        for label in ["settings", "Main", "", "main-2"] {
            let error = ensure_main(label).unwrap_err();
            assert_eq!(error.code, "window.not_main");
            assert_eq!(error.category, "invalid_input");
        }
    }

    #[test]
    fn full_screen_failure_has_its_stable_code() {
        let error = mode_unavailable();
        assert_eq!(error.code, "window.mode_unavailable");
        assert!(!error.retryable);
        assert_eq!(
            serde_json::to_value(WindowModeView { fullscreen: true }).unwrap(),
            serde_json::json!({ "fullscreen": true })
        );
    }
}
