//! Main-window geometry restoration (wave 3.4 §4.2). A pure placement model
//! decides where the window reopens; the Tauri glue restores it before the
//! window is first shown, follows it in memory and writes it on close and
//! exit. Sizes are logical pixels, so a window moved between 1x and 2x
//! displays keeps its size. Full screen is never restored, and no webview
//! command reads or writes geometry.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use tauri::{LogicalPosition, LogicalSize, Monitor, Runtime, WebviewWindow, Window, WindowEvent};

use super::local_store;

pub(crate) const MAIN_LABEL: &str = "main";
const GEOMETRY_FILE: &str = "window-geometry.json";
const MAX_GEOMETRY_BYTES: u64 = 1024;
const MAX_MONITOR_NAME_BYTES: usize = 128;
const GEOMETRY_VERSION: u8 = 1;
/// `tauri.conf.json` `minWidth`/`minHeight` (design-language l.50).
pub(crate) const MINIMUM_SIZE: (f64, f64) = (900.0, 600.0);
/// The point of the title bar a person grabs to move the window. A window
/// is placed only when this point is on a display.
const GRAB_POINT: (f64, f64) = (48.0, 16.0);

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavedGeometry {
    pub version: u8,
    /// Logical pixels of the normal (not maximised) frame.
    pub width: f64,
    pub height: f64,
    /// Logical; `None` where the window system hides positions (Wayland).
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub monitor: Option<String>,
    pub maximized: bool,
}

/// A display's logical work area.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct MonitorArea {
    pub name: Option<String>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl MonitorArea {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Placement {
    /// The configured 1280x800, placed by the window manager.
    Default,
    Size {
        width: f64,
        height: f64,
        maximized: bool,
    },
    SizeAndPosition {
        width: f64,
        height: f64,
        x: f64,
        y: f64,
        maximized: bool,
    },
}

fn usable(saved: &SavedGeometry) -> bool {
    let finite = |value: Option<f64>| value.is_none_or(f64::is_finite);
    saved.version == GEOMETRY_VERSION
        && saved.width.is_finite()
        && saved.height.is_finite()
        && saved.width >= 1.0
        && saved.height >= 1.0
        && finite(saved.x)
        && finite(saved.y)
        && saved.x.is_some() == saved.y.is_some()
        && saved
            .monitor
            .as_ref()
            .is_none_or(|name| name.len() <= MAX_MONITOR_NAME_BYTES)
}

/// Where the main window reopens.
///
/// A missing or unusable record gives `Default`. The size is clamped to the
/// minimum and to the work area of the saved display (or, when that display
/// is gone, the display under the grab point, then the primary one). The
/// position is kept only when the window system supports it and the grab
/// point is on a display.
pub(crate) fn placement(
    saved: Option<SavedGeometry>,
    monitors: &[MonitorArea],
    primary: Option<&MonitorArea>,
    minimum: (f64, f64),
    position_supported: bool,
) -> Placement {
    let Some(saved) = saved.filter(usable) else {
        return Placement::Default;
    };
    let grab = saved
        .x
        .zip(saved.y)
        .map(|(x, y)| (x + GRAB_POINT.0, y + GRAB_POINT.1));
    let named = saved.monitor.as_deref().and_then(|name| {
        monitors
            .iter()
            .find(|monitor| monitor.name.as_deref() == Some(name))
    });
    let under_grab = grab.and_then(|(x, y)| monitors.iter().find(|monitor| monitor.contains(x, y)));
    let target = named.or(under_grab).or(primary).or(monitors.first());

    let clamp = |value: f64, limit: Option<f64>, minimum: f64| {
        let bounded = limit.map_or(value, |limit| value.min(limit));
        bounded.max(minimum).round()
    };
    let width = clamp(saved.width, target.map(|area| area.width), minimum.0);
    let height = clamp(saved.height, target.map(|area| area.height), minimum.1);

    match (saved.x.zip(saved.y), under_grab) {
        (Some((x, y)), Some(_)) if position_supported => Placement::SizeAndPosition {
            width,
            height,
            x: x.round(),
            y: y.round(),
            maximized: saved.maximized,
        },
        _ => Placement::Size {
            width,
            height,
            maximized: saved.maximized,
        },
    }
}

/// Wayland does not let a client read or set its window position, so the
/// position is neither saved nor restored there (PE-5).
pub(crate) fn position_supported(wayland_display: Option<&str>, gdk_backend: Option<&str>) -> bool {
    let wayland = wayland_display.is_some_and(|display| !display.is_empty());
    !wayland || gdk_backend.is_some_and(|backend| backend.trim() == "x11")
}

fn position_supported_here() -> bool {
    position_supported(
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        std::env::var("GDK_BACKEND").ok().as_deref(),
    )
}

/// A window frame in logical pixels.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct LogicalFrame {
    pub width: f64,
    pub height: f64,
    pub position: Option<(f64, f64)>,
}

/// The logical normal frame from physical values at `scale`.
pub(crate) fn logical_frame(
    size: (u32, u32),
    position: Option<(i32, i32)>,
    scale: f64,
) -> Option<LogicalFrame> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    Some(LogicalFrame {
        width: f64::from(size.0) / scale,
        height: f64::from(size.1) / scale,
        position: position.map(|(x, y)| (f64::from(x) / scale, f64::from(y) / scale)),
    })
}

fn bounded_monitor_name(name: Option<&String>) -> Option<String> {
    name.filter(|name| name.len() <= MAX_MONITOR_NAME_BYTES && !name.chars().any(char::is_control))
        .cloned()
}

fn monitor_area(monitor: &Monitor) -> Option<MonitorArea> {
    let scale = monitor.scale_factor();
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let area = monitor.work_area();
    Some(MonitorArea {
        name: bounded_monitor_name(monitor.name()),
        x: f64::from(area.position.x) / scale,
        y: f64::from(area.position.y) / scale,
        width: f64::from(area.size.width) / scale,
        height: f64::from(area.size.height) / scale,
    })
}

fn parse(bytes: &[u8]) -> Option<SavedGeometry> {
    serde_json::from_slice(bytes).ok()
}

/// The saved geometry and the main window's current one.
pub(crate) struct WindowGeometryState {
    directory: PathBuf,
    saved: Option<SavedGeometry>,
    current: Mutex<Option<SavedGeometry>>,
}

impl WindowGeometryState {
    /// Reads `window-geometry.json` once. Anything missing, oversized or
    /// unreadable means the default placement.
    pub(crate) fn new(app_data_directory: &Path) -> Self {
        let saved =
            local_store::read_bounded(&app_data_directory.join(GEOMETRY_FILE), MAX_GEOMETRY_BYTES)
                .ok()
                .flatten()
                .and_then(|bytes| parse(&bytes))
                .filter(usable);
        Self {
            directory: app_data_directory.to_path_buf(),
            current: Mutex::new(saved.clone()),
            saved,
        }
    }

    fn update(&self, change: impl FnOnce(&mut Option<SavedGeometry>)) {
        if let Ok(mut current) = self.current.lock() {
            change(&mut current);
        }
    }

    /// Writes the current geometry. Restoration is a convenience, so a
    /// failed write is ignored.
    pub(crate) fn persist(&self) {
        let Some(current) = self.current.lock().ok().and_then(|current| current.clone()) else {
            return;
        };
        if let Ok(bytes) = serde_json::to_vec(&current) {
            let _ = local_store::write_private_atomically(&self.directory, GEOMETRY_FILE, &bytes);
        }
    }
}

/// Applies the saved placement to the hidden main window. The caller shows
/// the window whatever this returns.
pub(crate) fn restore<R: Runtime>(
    window: &WebviewWindow<R>,
    state: &WindowGeometryState,
) -> tauri::Result<()> {
    let monitors: Vec<MonitorArea> = window
        .available_monitors()?
        .iter()
        .filter_map(monitor_area)
        .collect();
    let primary = window
        .primary_monitor()
        .ok()
        .flatten()
        .and_then(|monitor| monitor_area(&monitor));
    let placement = placement(
        state.saved.clone(),
        &monitors,
        primary.as_ref(),
        MINIMUM_SIZE,
        position_supported_here(),
    );
    let maximized = match placement {
        Placement::Default => return Ok(()),
        Placement::Size {
            width,
            height,
            maximized,
        } => {
            window.set_size(LogicalSize::new(width, height))?;
            maximized
        }
        Placement::SizeAndPosition {
            width,
            height,
            x,
            y,
            maximized,
        } => {
            window.set_size(LogicalSize::new(width, height))?;
            window.set_position(LogicalPosition::new(x, y))?;
            maximized
        }
    };
    if maximized {
        window.maximize()?;
    }
    Ok(())
}

/// Shows and focuses the main window after trying to restore its geometry.
/// A restore failure leaves the configured default; the window is never
/// left hidden (wave 3.4 §11 Q7).
pub(crate) fn present<R: Runtime>(window: &WebviewWindow<R>, state: &WindowGeometryState) {
    let _ = restore(window, state);
    let _ = window.show();
    let _ = window.set_focus();
}

/// Follows the main window's normal frame and maximised state in memory,
/// and writes them when the window is about to close.
pub(crate) fn track<R: Runtime>(
    window: &Window<R>,
    event: &WindowEvent,
    state: &WindowGeometryState,
) {
    if window.label() != MAIN_LABEL {
        return;
    }
    match event {
        WindowEvent::Resized(_)
        | WindowEvent::Moved(_)
        | WindowEvent::ScaleFactorChanged { .. } => {
            record(window, state);
        }
        WindowEvent::CloseRequested { .. } => state.persist(),
        _ => {}
    }
}

fn record<R: Runtime>(window: &Window<R>, state: &WindowGeometryState) {
    if window.is_fullscreen().unwrap_or(true) || window.is_minimized().unwrap_or(true) {
        // Full screen is session-only, and a minimised window has no frame.
        return;
    }
    let maximized = window.is_maximized().unwrap_or(false);
    if maximized {
        state.update(|current| {
            if let Some(current) = current {
                current.maximized = true;
            }
        });
        return;
    }
    let Ok(size) = window.inner_size() else {
        return;
    };
    let Ok(scale) = window.scale_factor() else {
        return;
    };
    let position = if position_supported_here() {
        window
            .outer_position()
            .ok()
            .map(|position| (position.x, position.y))
    } else {
        None
    };
    let Some(frame) = logical_frame((size.width, size.height), position, scale) else {
        return;
    };
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .and_then(|monitor| bounded_monitor_name(monitor.name()));
    let geometry = SavedGeometry {
        version: GEOMETRY_VERSION,
        width: frame.width,
        height: frame.height,
        x: frame.position.map(|(x, _)| x),
        y: frame.position.map(|(_, y)| y),
        monitor,
        maximized: false,
    };
    if usable(&geometry) {
        state.update(|current| *current = Some(geometry));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(name: &str, x: f64, y: f64, width: f64, height: f64) -> MonitorArea {
        MonitorArea {
            name: Some(name.into()),
            x,
            y,
            width,
            height,
        }
    }

    fn saved(
        width: f64,
        height: f64,
        position: Option<(f64, f64)>,
        name: Option<&str>,
    ) -> SavedGeometry {
        SavedGeometry {
            version: 1,
            width,
            height,
            x: position.map(|(x, _)| x),
            y: position.map(|(_, y)| y),
            monitor: name.map(str::to_owned),
            maximized: false,
        }
    }

    fn laptop() -> MonitorArea {
        monitor("eDP-1", 0.0, 0.0, 1920.0, 1080.0)
    }

    #[test]
    fn placement_defaults_when_file_missing_corrupt_or_versioned() {
        let monitors = [laptop()];
        let place = |geometry: Option<SavedGeometry>| {
            placement(geometry, &monitors, monitors.first(), MINIMUM_SIZE, true)
        };
        assert_eq!(place(None), Placement::Default);
        let mut versioned = saved(1280.0, 800.0, None, None);
        versioned.version = 2;
        assert_eq!(place(Some(versioned)), Placement::Default);
        assert_eq!(
            place(Some(saved(f64::NAN, 800.0, None, None))),
            Placement::Default
        );
        assert_eq!(
            place(Some(saved(1280.0, 800.0, Some((f64::INFINITY, 0.0)), None))),
            Placement::Default
        );
        assert_eq!(
            place(Some(saved(0.0, 800.0, None, None))),
            Placement::Default
        );

        // Unparseable and oversized files never reach the model.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(GEOMETRY_FILE);
        for contents in [
            "{".to_owned(),
            r#"{"version":1}"#.to_owned(),
            format!(
                r#"{{"version":1,"width":1280,"height":800,"x":null,"y":null,"monitor":"{}","maximized":false}}"#,
                "m".repeat(1100)
            ),
        ] {
            std::fs::write(&path, contents).unwrap();
            assert_eq!(WindowGeometryState::new(directory.path()).saved, None);
        }
        std::fs::write(
            &path,
            r#"{"version":1,"width":1400,"height":900,"x":10,"y":20,"monitor":"eDP-1","maximized":true}"#,
        )
        .unwrap();
        let state = WindowGeometryState::new(directory.path());
        assert_eq!(
            state.saved,
            Some(SavedGeometry {
                version: 1,
                width: 1400.0,
                height: 900.0,
                x: Some(10.0),
                y: Some(20.0),
                monitor: Some("eDP-1".into()),
                maximized: true,
            })
        );
    }

    #[test]
    fn placement_clamps_size_to_minimum_and_to_target_work_area() {
        let monitors = [laptop()];
        assert_eq!(
            placement(
                Some(saved(5000.0, 4000.0, None, Some("eDP-1"))),
                &monitors,
                monitors.first(),
                MINIMUM_SIZE,
                true
            ),
            Placement::Size {
                width: 1920.0,
                height: 1080.0,
                maximized: false
            }
        );
        assert_eq!(
            placement(
                Some(saved(400.0, 300.0, None, None)),
                &monitors,
                monitors.first(),
                MINIMUM_SIZE,
                true
            ),
            Placement::Size {
                width: 900.0,
                height: 600.0,
                maximized: false
            }
        );
    }

    #[test]
    fn placement_restores_position_on_named_secondary_monitor() {
        let monitors = [laptop(), monitor("HDMI-1", -1920.0, 0.0, 1920.0, 1200.0)];
        assert_eq!(
            placement(
                Some(saved(1400.0, 1100.0, Some((-1800.0, 40.0)), Some("HDMI-1"))),
                &monitors,
                monitors.first(),
                MINIMUM_SIZE,
                true
            ),
            Placement::SizeAndPosition {
                width: 1400.0,
                height: 1100.0,
                x: -1800.0,
                y: 40.0,
                maximized: false
            }
        );
    }

    #[test]
    fn placement_falls_back_to_primary_when_saved_monitor_is_gone() {
        let small = monitor("DP-2", 0.0, 0.0, 1366.0, 768.0);
        let monitors = [small.clone()];
        // The secondary display was unplugged: its position is off screen.
        assert_eq!(
            placement(
                Some(saved(1800.0, 1100.0, Some((-1800.0, 40.0)), Some("HDMI-1"))),
                &monitors,
                Some(&small),
                MINIMUM_SIZE,
                true
            ),
            Placement::Size {
                width: 1366.0,
                height: 768.0,
                maximized: false
            }
        );
    }

    #[test]
    fn placement_skips_position_when_unsupported() {
        let monitors = [laptop()];
        assert_eq!(
            placement(
                Some(saved(1280.0, 800.0, Some((100.0, 100.0)), Some("eDP-1"))),
                &monitors,
                monitors.first(),
                MINIMUM_SIZE,
                false
            ),
            Placement::Size {
                width: 1280.0,
                height: 800.0,
                maximized: false
            }
        );
        assert!(!position_supported(Some("wayland-0"), None));
        assert!(!position_supported(Some("wayland-0"), Some("wayland")));
        assert!(position_supported(Some("wayland-0"), Some("x11")));
        assert!(position_supported(None, None));
        assert!(position_supported(Some(""), None));
    }

    #[test]
    fn placement_rejects_off_screen_grab_point() {
        let monitors = [laptop()];
        for position in [
            (1900.0, 100.0),
            (100.0, 1070.0),
            (-60.0, 100.0),
            (100.0, -20.0),
        ] {
            assert_eq!(
                placement(
                    Some(saved(1280.0, 800.0, Some(position), Some("eDP-1"))),
                    &monitors,
                    monitors.first(),
                    MINIMUM_SIZE,
                    true
                ),
                Placement::Size {
                    width: 1280.0,
                    height: 800.0,
                    maximized: false
                },
                "{position:?}"
            );
        }
    }

    #[test]
    fn maximized_restores_normal_frame_then_maximizes() {
        let monitors = [laptop()];
        let mut geometry = saved(1300.0, 850.0, Some((40.0, 30.0)), Some("eDP-1"));
        geometry.maximized = true;
        assert_eq!(
            placement(
                Some(geometry),
                &monitors,
                monitors.first(),
                MINIMUM_SIZE,
                true
            ),
            Placement::SizeAndPosition {
                width: 1300.0,
                height: 850.0,
                x: 40.0,
                y: 30.0,
                maximized: true
            }
        );

        // Maximising keeps the normal frame; only the flag changes.
        let directory = tempfile::tempdir().unwrap();
        let state = WindowGeometryState::new(directory.path());
        state.update(|current| *current = Some(saved(1300.0, 850.0, None, None)));
        state.update(|current| {
            if let Some(current) = current {
                current.maximized = true;
            }
        });
        state.persist();
        let reread = WindowGeometryState::new(directory.path());
        let mut expected = saved(1300.0, 850.0, None, None);
        expected.maximized = true;
        assert_eq!(reread.saved, Some(expected));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(directory.path().join(GEOMETRY_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn logical_frame_is_scale_independent() {
        assert_eq!(
            logical_frame((2560, 1600), Some((200, 100)), 2.0),
            logical_frame((1280, 800), Some((100, 50)), 1.0)
        );
        assert_eq!(
            logical_frame((2560, 1600), None, 2.0),
            Some(LogicalFrame {
                width: 1280.0,
                height: 800.0,
                position: None
            })
        );
        assert_eq!(logical_frame((1280, 800), None, 0.0), None);
        assert_eq!(logical_frame((1280, 800), None, f64::NAN), None);
    }

    #[test]
    fn nothing_is_written_before_a_frame_is_known() {
        let directory = tempfile::tempdir().unwrap();
        WindowGeometryState::new(directory.path()).persist();
        assert!(!directory.path().join(GEOMETRY_FILE).exists());
    }

    #[test]
    fn saved_geometry_is_camel_case() {
        assert_eq!(
            serde_json::to_value(saved(1280.0, 800.0, Some((1.0, 2.0)), Some("eDP-1"))).unwrap(),
            serde_json::json!({
                "version": 1, "width": 1280.0, "height": 800.0, "x": 1.0, "y": 2.0,
                "monitor": "eDP-1", "maximized": false
            })
        );
    }
}
