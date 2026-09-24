//! Every file this app keeps in its app data directory, damaged each way the
//! Wave 4 fault drill lists (corrupted local state): empty, truncated,
//! oversized, garbage, a directory in its place, and a newer version. The
//! app state `lib.rs` builds at launch still loads, and each file is reset,
//! set aside or kept exactly as its module specifies.

use std::{fs, path::Path, sync::Arc};

use uuid::Uuid;

use super::{
    keystore::tests::{CountingStore, Fail},
    notifications::NotificationPreferences,
    planes::spawner::fake::FakeSpawner,
    presentation::{PresentationState, ShellPresentation},
    settings_window::SettingsTarget,
    window_state::WindowGeometryState,
    JetBridge,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Damage {
    Empty,
    Truncated,
    Oversized,
    Garbage,
    Directory,
    Newer,
}

const DAMAGE: [Damage; 6] = [
    Damage::Empty,
    Damage::Truncated,
    Damage::Oversized,
    Damage::Garbage,
    Damage::Directory,
    Damage::Newer,
];

struct AppDataFile {
    name: &'static str,
    /// Contents that load to something other than the default, so a reset
    /// is visible.
    valid: &'static str,
    /// What a newer Jet could write there.
    newer: &'static str,
}

const FILES: [AppDataFile; 9] = [
    AppDataFile {
        name: "client-id",
        valid: "00000000-0000-0000-0000-0000000000c1\n",
        newer: "jet-client-id-v2:00000000-0000-0000-0000-0000000000c1\n",
    },
    AppDataFile {
        name: "planes.json",
        valid: r#"{"version":1,"localIdentity":null,"planes":[{"id":"00000000-0000-0000-0000-000000000002","destination":"alice@build-box","planeIdentity":"00000000-0000-0000-0000-000000000020","credential":"durable","addedAtUnixMs":1}]}"#,
        newer: r#"{"version":2,"localIdentity":null,"planes":[],"fromANewerJet":true}"#,
    },
    AppDataFile {
        name: "last-conversation",
        valid: "00000000-0000-0000-0000-00000000c0de\nlocal\n",
        newer: "00000000-0000-0000-0000-00000000c0de\nlocal\nfrom-a-newer-jet\n",
    },
    AppDataFile {
        name: "desktop-preferences.json",
        valid: r#"{"reopenLastTask":false}"#,
        // Only the default for the known field: strict and lenient readers
        // agree on what this loads to.
        newer: r#"{"reopenLastTask":true,"fromANewerJet":true}"#,
    },
    AppDataFile {
        name: "notification-preferences-v2.json",
        valid: r#"{"enabled":true,"approvals":true,"completion":true,"failure":true,"mutedPlanes":[]}"#,
        newer: r#"{"enabled":true,"approvals":true,"completion":true,"failure":true,"mutedPlanes":[],"fromANewerJet":true}"#,
    },
    AppDataFile {
        name: "settings-window.json",
        valid: r#"{"pane":"safety"}"#,
        newer: r#"{"pane":"from_a_newer_jet"}"#,
    },
    AppDataFile {
        name: "shell-presentation.json",
        valid: r#"{"version":1,"destination":"conversation","sidebarPresented":false,"workPanelPresented":true,"workPanelTab":"run","sidebarWidth":300,"workPanelWidth":420}"#,
        newer: r#"{"version":2,"sidebarPresented":false}"#,
    },
    AppDataFile {
        name: "window-geometry.json",
        valid: r#"{"version":1,"width":1000,"height":700,"x":null,"y":null,"monitor":null,"maximized":false}"#,
        newer: r#"{"version":2,"width":1000,"height":700,"x":null,"y":null,"monitor":null,"maximized":false}"#,
    },
    // Section A's provisioning lock. Its bytes carry no state; nothing at
    // launch reads it, so it only must not stop the app from loading.
    AppDataFile {
        name: "local-service.lock",
        valid: "",
        newer: "from a newer jet",
    },
];

/// Writes `file` damaged by `damage`. Returns the bytes written, or `None`
/// for a directory.
fn damage(app_data: &Path, file: &AppDataFile, damage: Damage) -> Option<Vec<u8>> {
    let path = app_data.join(file.name);
    let bytes = match damage {
        Damage::Directory => {
            fs::create_dir(&path).unwrap();
            fs::write(path.join("inner"), b"not the file").unwrap();
            return None;
        }
        Damage::Empty => Vec::new(),
        Damage::Truncated => file.valid.as_bytes()[..file.valid.len() / 2].to_vec(),
        // Valid contents followed by a megabyte of padding: only the size
        // is wrong, so the bound, not the parser, must refuse it.
        Damage::Oversized => [file.valid.as_bytes(), &vec![b' '; 1 << 20]].concat(),
        Damage::Garbage => vec![0xff, 0x00, b'{', 0xfe, b'\n'],
        Damage::Newer => file.newer.as_bytes().to_vec(),
    };
    fs::write(&path, &bytes).unwrap();
    Some(bytes)
}

/// Everything `lib.rs` builds from the app data directory at launch, with a
/// fake key store and ssh: nothing leaves the process.
struct Launched {
    bridge: JetBridge,
    presentation: PresentationState,
    geometry: WindowGeometryState,
}

fn launch(home: &Path, app_data: &Path) -> Launched {
    Launched {
        bridge: JetBridge::open(
            home,
            app_data,
            Arc::new(CountingStore::new(Fail::Nothing)),
            Arc::new(FakeSpawner::default()),
        ),
        presentation: PresentationState::new(app_data),
        geometry: WindowGeometryState::new(app_data),
    }
}

fn planes_view(launched: &Launched) -> serde_json::Value {
    serde_json::to_value(launched.bridge.planes.snapshot(None).unwrap()).unwrap()
}

/// The valid contents load to their non-default values, so every reset the
/// table below asserts is a real change.
#[test]
fn valid_app_data_loads_what_it_holds() {
    let home = tempfile::tempdir().unwrap();
    let app_data = home.path().join("app-data");
    fs::create_dir(&app_data).unwrap();
    for file in &FILES {
        fs::write(app_data.join(file.name), file.valid).unwrap();
    }
    let launched = launch(home.path(), &app_data);
    let planes = planes_view(&launched);
    assert_eq!(
        planes["identity"]["clientId"],
        "00000000-0000-0000-0000-0000000000c1"
    );
    assert_eq!(planes["planes"].as_array().unwrap().len(), 2);
    assert!(launched
        .bridge
        .conversations
        .restored_selection()
        .unwrap()
        .is_some());
    assert!(!launched.bridge.preferences.reopen_last_task().unwrap());
    assert_ne!(
        launched.bridge.notifications.preferences(),
        NotificationPreferences::default()
    );
    assert_ne!(
        launched.bridge.settings_window.remembered(),
        SettingsTarget::default()
    );
    assert_ne!(
        serde_json::to_value(launched.presentation.loaded()).unwrap()["presentation"],
        serde_json::to_value(ShellPresentation::default()).unwrap()
    );
    assert!(launched.geometry.saved().is_some());
}

#[test]
fn every_app_data_file_survives_every_kind_of_damage() {
    for file in &FILES {
        for kind in DAMAGE {
            let home = tempfile::tempdir().unwrap();
            let app_data = home.path().join("app-data");
            fs::create_dir(&app_data).unwrap();
            let written = damage(&app_data, file, kind);
            let case = format!("{} {kind:?}", file.name);

            // Launch never fails and never panics.
            let launched = launch(home.path(), &app_data);
            let planes = planes_view(&launched);

            match file.name {
                "client-id" => {
                    // Set aside, replaced (the fake keyring names no
                    // identity), saved, and reported.
                    assert_eq!(planes["identity"]["notice"], "identity_replaced", "{case}");
                    assert!(app_data.join("client-id.invalid").exists(), "{case}");
                    let saved = fs::read_to_string(app_data.join("client-id")).unwrap();
                    assert_eq!(
                        Uuid::parse_str(saved.trim()).unwrap().to_string(),
                        planes["identity"]["clientId"],
                        "{case}"
                    );
                }
                "planes.json" => {
                    assert_eq!(planes["planes"].as_array().unwrap().len(), 1, "{case}");
                    if kind == Damage::Newer {
                        // Kept byte for byte, read-only, and reported.
                        assert_eq!(planes["notice"], "registry_newer", "{case}");
                        assert_eq!(fs::read(app_data.join(file.name)).ok(), written, "{case}");
                        assert!(!app_data.join("planes.json.invalid").exists(), "{case}");
                    } else {
                        assert_eq!(planes["notice"], "registry_reset", "{case}");
                        assert!(app_data.join("planes.json.invalid").exists(), "{case}");
                        assert!(!app_data.join(file.name).exists(), "{case}");
                    }
                }
                "last-conversation" => {
                    assert_eq!(
                        launched.bridge.conversations.restored_selection().unwrap(),
                        None,
                        "{case}"
                    );
                }
                "desktop-preferences.json" => {
                    assert!(
                        launched.bridge.preferences.reopen_last_task().unwrap(),
                        "{case}"
                    );
                }
                "notification-preferences-v2.json" => {
                    assert_eq!(
                        launched.bridge.notifications.preferences(),
                        NotificationPreferences::default(),
                        "{case}"
                    );
                }
                "settings-window.json" => {
                    assert_eq!(
                        launched.bridge.settings_window.remembered(),
                        SettingsTarget::default(),
                        "{case}"
                    );
                }
                "shell-presentation.json" => {
                    let loaded = serde_json::to_value(launched.presentation.loaded()).unwrap();
                    assert_eq!(
                        loaded["presentation"],
                        serde_json::to_value(ShellPresentation::default()).unwrap(),
                        "{case}"
                    );
                    // The layout reset is reported, unlike a missing file.
                    assert_eq!(loaded["issue"], "presentation.read_failed", "{case}");
                }
                "window-geometry.json" => {
                    assert_eq!(launched.geometry.saved(), None, "{case}");
                }
                "local-service.lock" => {
                    assert_eq!(fs::read(app_data.join(file.name)).ok(), written, "{case}");
                }
                other => panic!("no expectation for {other}"),
            }

            // Every other file loaded untouched: no notice but its own.
            if file.name != "client-id" {
                assert_eq!(
                    planes["identity"]["notice"],
                    serde_json::Value::Null,
                    "{case}"
                );
            }
            if file.name != "planes.json" {
                assert_eq!(planes["notice"], serde_json::Value::Null, "{case}");
            }
        }
    }
}
