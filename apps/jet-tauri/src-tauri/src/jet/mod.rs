pub(crate) mod agents;
mod channels;
mod client;
mod conversations;
pub(crate) mod delivery;
pub(crate) mod enrollment;
mod errors;
pub(crate) mod extensions;
mod identity;
mod keystore;
pub(crate) mod ledger;
#[cfg(test)]
mod live_e2e;
pub(crate) mod notifications;
pub(crate) mod pairing;
mod pairing_transcript;
pub(crate) mod planes;
pub(crate) mod preferences;
pub(crate) mod retention;
mod run_control;
pub(crate) mod settings;
pub(crate) mod settings_window;
mod setup;
pub(crate) mod system;
mod work_panel;

use std::{io, path::Path, sync::Arc, time::Duration};

use client::PlaneClient;
use tauri::{ipc::Channel, State};

use self::{
    channels::{ConnectionSnapshot, FeedRegistry, PlaneUpdate},
    errors::PublicError,
    planes::{PlaneBinding, PlaneRegistry},
};

/// Native-only authority for every registered Plane. None of these values
/// cross the webview boundary; the webview holds only opaque Plane handles.
pub(crate) struct JetBridge {
    planes: Arc<PlaneRegistry>,
    feeds: Arc<FeedRegistry>,
    delivery: delivery::DeliveryState,
    pairing: pairing::PairingState,
    enrollment: enrollment::EnrollmentState,
    notifications: std::sync::Arc<notifications::NotificationState>,
    setup: setup::SetupState,
    conversations: conversations::ConversationState,
    run_control: run_control::RunControlState,
    work_panel: work_panel::WorkPanelState,
    settings_window: settings_window::SettingsWindowState,
    settings: settings::SettingsState,
    agents: agents::AgentsState,
    extensions: extensions::ExtensionsState,
    preferences: preferences::PreferencesState,
    system: system::SystemState,
    retention: retention::RetentionState,
}

impl JetBridge {
    pub(crate) fn for_local_plane(
        home_directory: &Path,
        app_data_directory: &Path,
    ) -> io::Result<Self> {
        let client_id = identity::load_or_create(app_data_directory)?;
        let socket = home_directory.join(".jet/runtime/jetd.sock");
        let local = PlaneClient::new(
            socket,
            client_id,
            [
                Duration::from_millis(100),
                Duration::from_millis(500),
                Duration::from_secs(1),
            ],
            Duration::from_millis(500),
        );
        let planes = PlaneRegistry::open(
            local,
            Some(app_data_directory),
            Arc::new(planes::spawner::SystemSsh),
            Arc::new(keystore::IdentityKeys::new(keystore::platform_store())),
        );
        Ok(Self::with_planes(planes, app_data_directory))
    }

    fn with_planes(planes: PlaneRegistry, app_data_directory: &Path) -> Self {
        let notifications = std::sync::Arc::new(notifications::NotificationState::new(
            app_data_directory,
            |plane| planes.contains(plane),
        ));
        Self {
            planes: Arc::new(planes),
            feeds: Arc::new(FeedRegistry::default()),
            delivery: delivery::DeliveryState::default(),
            pairing: pairing::PairingState::default(),
            enrollment: enrollment::EnrollmentState::default(),
            notifications,
            setup: setup::SetupState::default(),
            conversations: conversations::ConversationState::new(app_data_directory),
            run_control: run_control::RunControlState::default(),
            work_panel: work_panel::WorkPanelState::default(),
            settings_window: settings_window::SettingsWindowState::new(app_data_directory),
            settings: settings::SettingsState::default(),
            agents: agents::AgentsState::default(),
            extensions: extensions::ExtensionsState::default(),
            preferences: preferences::PreferencesState::new(app_data_directory),
            system: system::SystemState::default(),
            retention: retention::RetentionState::default(),
        }
    }

    /// A bridge whose remote Planes use `spawner` and `keys` and whose local
    /// Plane socket does not exist. Nothing leaves the process.
    #[cfg(test)]
    pub(crate) fn for_test(
        app_data_directory: &Path,
        client_id: uuid::Uuid,
        spawner: Arc<dyn planes::spawner::SshSpawner>,
        keys: Arc<keystore::IdentityKeys>,
    ) -> Self {
        let local = PlaneClient::new(
            app_data_directory.join("missing-jetd.sock"),
            client_id,
            [Duration::from_millis(1)],
            Duration::from_millis(1),
        );
        let planes = PlaneRegistry::open(local, Some(app_data_directory), spawner, keys);
        Self::with_planes(planes, app_data_directory)
    }

    /// The local Plane: Project setup and new tasks stay local in Wave 3.1.
    fn local(&self) -> &PlaneClient {
        self.planes.local()
    }

    /// Resolves an explicit webview Plane handle; `None` means `local`.
    fn plane(&self, plane_id: Option<&str>) -> Result<(PlaneBinding, PlaneClient), PublicError> {
        self.planes.resolve(plane_id)
    }

    /// The client for a stored binding, or `plane.review_moved`.
    fn bound(&self, binding: &PlaneBinding) -> Result<PlaneClient, PublicError> {
        self.planes.bound(binding)
    }

    /// The task to reopen at launch, unless this computer's desktop
    /// preferences say not to. The selection file is still kept current.
    fn restorable_selection(&self) -> Result<Option<(uuid::Uuid, planes::PlaneId)>, PublicError> {
        if !self.preferences.reopen_last_task()? {
            return Ok(None);
        }
        self.conversations.restored_selection()
    }

    /// The Settings window was destroyed: drop what only it could receive.
    pub(crate) fn settings_window_closed(&self) {
        self.settings_window.window_destroyed();
        self.settings.window_destroyed();
    }

    /// Records what a failure proves about its Plane and names the Plane.
    fn settle(&self, binding: &PlaneBinding, error: PublicError) -> PublicError {
        self.planes.settle(binding.plane, error)
    }
}

#[tauri::command]
pub(crate) async fn open_plane_feed(
    app: tauri::AppHandle,
    bridge: State<'_, JetBridge>,
    plane_id: Option<String>,
    on_update: Channel<PlaneUpdate>,
    after: Option<String>,
    reset: Option<bool>,
) -> Result<ConnectionSnapshot, PublicError> {
    channels::open_plane_feed(app, bridge, plane_id, on_update, after, reset).await
}

#[tauri::command]
pub(crate) fn close_plane_feed(
    bridge: State<'_, JetBridge>,
    feed_id: String,
) -> Result<(), PublicError> {
    channels::close_plane_feed(&bridge, feed_id)
}

#[tauri::command]
pub(crate) async fn load_setup(
    bridge: State<'_, JetBridge>,
) -> Result<setup::SetupSnapshot, PublicError> {
    setup::load_setup(bridge).await
}

#[tauri::command]
pub(crate) async fn preview_project(
    bridge: State<'_, JetBridge>,
    path: String,
) -> Result<setup::ProjectPreviewView, PublicError> {
    setup::preview_project(bridge, path).await
}

#[tauri::command]
pub(crate) async fn register_project(
    bridge: State<'_, JetBridge>,
    preview_id: String,
) -> Result<setup::MutationResult, PublicError> {
    setup::register_project(bridge, preview_id).await
}

#[tauri::command]
pub(crate) async fn preview_project_removal(
    bridge: State<'_, JetBridge>,
    project_id: String,
) -> Result<setup::ProjectRemovalPreviewView, PublicError> {
    setup::preview_project_removal(bridge, project_id).await
}

#[tauri::command]
pub(crate) async fn remove_project(
    bridge: State<'_, JetBridge>,
    preview_id: String,
    typed_name: String,
    permanent: bool,
) -> Result<setup::MutationResult, PublicError> {
    setup::remove_project(bridge, preview_id, typed_name, permanent).await
}

#[tauri::command]
pub(crate) async fn bind_harness_account(
    bridge: State<'_, JetBridge>,
    provider: String,
) -> Result<setup::MutationResult, PublicError> {
    setup::bind_harness_account(bridge, provider).await
}

#[tauri::command]
pub(crate) async fn load_conversations(
    bridge: State<'_, JetBridge>,
    plane_id: Option<String>,
    next_page: Option<String>,
) -> Result<conversations::ConversationPageView, PublicError> {
    conversations::load_conversations(bridge, plane_id, next_page).await
}

#[tauri::command]
pub(crate) async fn search_conversations(
    bridge: State<'_, JetBridge>,
    plane_id: Option<String>,
    text: String,
) -> Result<conversations::SearchResultView, PublicError> {
    conversations::search_conversations(bridge, plane_id, text).await
}

#[tauri::command]
pub(crate) async fn load_conversation(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    plane_id: Option<String>,
) -> Result<conversations::ConversationDetailView, PublicError> {
    conversations::load_conversation(bridge, conversation_id, plane_id).await
}

#[tauri::command]
pub(crate) async fn create_conversation(
    bridge: State<'_, JetBridge>,
    project_id: String,
) -> Result<conversations::ConversationRowView, PublicError> {
    conversations::create_conversation(bridge, project_id).await
}

#[tauri::command]
pub(crate) async fn start_run(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    craft: String,
    prompt: String,
    plane_id: Option<String>,
) -> Result<conversations::StartResultView, PublicError> {
    conversations::start_run(bridge, conversation_id, craft, prompt, plane_id).await
}

#[tauri::command]
pub(crate) async fn submit_turn(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    prompt: String,
    plane_id: Option<String>,
) -> Result<conversations::TurnResultView, PublicError> {
    conversations::submit_turn(bridge, conversation_id, prompt, plane_id).await
}

#[tauri::command]
pub(crate) async fn load_run_supervision(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    run_id: Option<String>,
    plane_id: Option<String>,
) -> Result<run_control::RunSupervisionView, PublicError> {
    run_control::load_run_supervision(bridge, conversation_id, run_id, plane_id).await
}

#[tauri::command]
pub(crate) async fn withdraw_turn(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    turn_id: String,
    plane_id: Option<String>,
) -> Result<run_control::TurnView, PublicError> {
    run_control::withdraw_turn(bridge, conversation_id, turn_id, plane_id).await
}

#[tauri::command]
pub(crate) async fn interrupt_turn(
    bridge: State<'_, JetBridge>,
    run_id: String,
    plane_id: Option<String>,
) -> Result<run_control::CommandAcceptedView, PublicError> {
    run_control::interrupt_turn(bridge, run_id, plane_id).await
}

#[tauri::command]
pub(crate) async fn stop_run(
    bridge: State<'_, JetBridge>,
    run_id: String,
    plane_id: Option<String>,
) -> Result<run_control::CommandAcceptedView, PublicError> {
    run_control::stop_run(bridge, run_id, plane_id).await
}

#[tauri::command]
pub(crate) async fn authorize_approval_retry(
    bridge: State<'_, JetBridge>,
    run_id: String,
    review_id: String,
    plane_id: Option<String>,
) -> Result<run_control::ApprovalRetryView, PublicError> {
    run_control::authorize_approval_retry(bridge, run_id, review_id, plane_id).await
}

// The IPC argument list is the webview contract; the body forwards it as one
// request value.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn load_work_panel(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    run_id: String,
    scope_kind: String,
    turn: Option<u32>,
    from_turn: Option<u32>,
    to_turn: Option<u32>,
    plane_id: Option<String>,
) -> Result<work_panel::WorkPanelSnapshot, PublicError> {
    work_panel::load_work_panel(
        bridge,
        work_panel::WorkPanelRequest {
            conversation_id,
            run_id,
            scope_kind,
            turn,
            from_turn,
            to_turn,
        },
        plane_id,
    )
    .await
}

#[tauri::command]
pub(crate) async fn load_more_changes(
    bridge: State<'_, JetBridge>,
    page_id: String,
) -> Result<work_panel::ChangePageView, PublicError> {
    work_panel::load_more_changes(bridge, page_id).await
}

#[tauri::command]
pub(crate) async fn load_patch_chunk(
    bridge: State<'_, JetBridge>,
    artifact_read_id: String,
) -> Result<work_panel::ArtifactChunkView, PublicError> {
    work_panel::load_patch_chunk(bridge, artifact_read_id).await
}

#[tauri::command]
pub(crate) async fn load_work_file(
    bridge: State<'_, JetBridge>,
    file_id: String,
) -> Result<work_panel::EditableFileView, PublicError> {
    work_panel::load_work_file(bridge, file_id).await
}

#[tauri::command]
pub(crate) async fn save_work_file(
    bridge: State<'_, JetBridge>,
    file_id: String,
    content: String,
) -> Result<work_panel::FileSavedView, PublicError> {
    work_panel::save_work_file(bridge, file_id, content).await
}

#[tauri::command]
pub(crate) async fn submit_file_review(
    bridge: State<'_, JetBridge>,
    file_id: String,
    line: u32,
    comment: String,
) -> Result<work_panel::ReviewSubmittedView, PublicError> {
    work_panel::submit_file_review(bridge, file_id, line, comment).await
}

#[tauri::command]
pub(crate) async fn open_workspace_terminal(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    rows: u16,
    columns: u16,
    plane_id: Option<String>,
) -> Result<work_panel::TerminalView, PublicError> {
    work_panel::open_workspace_terminal(bridge, conversation_id, rows, columns, plane_id).await
}

#[tauri::command]
pub(crate) async fn close_workspace_terminal(
    bridge: State<'_, JetBridge>,
    terminal_id: String,
) -> Result<work_panel::TerminalView, PublicError> {
    work_panel::close_workspace_terminal(bridge, terminal_id).await
}

#[tauri::command]
pub(crate) async fn attach_workspace_terminal(
    bridge: State<'_, JetBridge>,
    terminal_id: String,
    on_update: Channel<work_panel::TerminalUpdate>,
) -> Result<(), PublicError> {
    work_panel::attach_workspace_terminal(bridge, terminal_id, on_update).await
}

#[tauri::command]
pub(crate) async fn send_terminal_input(
    bridge: State<'_, JetBridge>,
    terminal_id: String,
    input: String,
) -> Result<(), PublicError> {
    work_panel::send_terminal_input(bridge, terminal_id, input).await
}

#[tauri::command]
pub(crate) async fn resize_workspace_terminal(
    bridge: State<'_, JetBridge>,
    terminal_id: String,
    rows: u16,
    columns: u16,
) -> Result<(), PublicError> {
    work_panel::resize_workspace_terminal(bridge, terminal_id, rows, columns).await
}

#[tauri::command]
pub(crate) fn detach_workspace_terminal(
    bridge: State<'_, JetBridge>,
    terminal_id: String,
) -> Result<(), PublicError> {
    work_panel::detach_workspace_terminal(bridge, terminal_id)
}

#[cfg(test)]
mod manifest_tests {
    use std::collections::BTreeSet;

    fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        let from = source.find(start).expect("list start") + start.len();
        let length = source[from..].find(end).expect("list end");
        &source[from..from + length]
    }

    /// The app commands a capability grants, and the windows it applies to.
    fn capability(source: &str) -> (BTreeSet<String>, Vec<String>) {
        let capability: serde_json::Value = serde_json::from_str(source).unwrap();
        let granted = capability["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|permission| permission.as_str())
            .filter(|permission| !permission.contains(':'))
            .map(|permission| {
                assert!(
                    permission.starts_with("allow-"),
                    "unexpected app permission {permission}"
                );
                permission["allow-".len()..].replace('-', "_")
            })
            .collect();
        let windows = capability["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|window| window.as_str().unwrap().to_owned())
            .collect();
        (granted, windows)
    }

    /// Commands both windows may call: read-only, no agent content.
    const SHARED: [&str; 3] = [
        "load_desktop_preferences",
        "list_planes",
        "load_plane_detail",
    ];

    /// Commands only the Settings window may call.
    const SETTINGS_ONLY: [&str; 24] = [
        "watch_settings_navigation",
        "remember_settings_pane",
        "close_settings",
        "set_desktop_preferences",
        "load_notification_settings",
        "set_notification_settings",
        "load_settings",
        "prepare_setting_change",
        "apply_settings_change",
        "load_work_context",
        "watch_settings_changes",
        "load_agents",
        "prepare_account_bind",
        "load_account_detail",
        "load_usage_history",
        "prepare_auto_continue",
        "prepare_account_unbind",
        "prepare_craft_disable",
        "pick_local_craft_source",
        "discover_craft",
        "load_extension_catalog",
        "inspect_extension",
        "prepare_extension_change",
        "load_extension_change",
    ];

    /// Wave 3.3 commands granted to the main window only.
    /// `resolve_conversation_names` joins the Settings window with its first
    /// call site there (auto-delete candidates).
    const WAVE_3_3_MAIN_ONLY: [&str; 7] = [
        "collect_disposable_storage",
        "load_trash",
        "load_trash_status",
        "resolve_conversation_names",
        "preview_trash",
        "trash_conversation",
        "restore_conversation",
    ];

    /// `generate_handler!` names, the `build.rs` manifest and the union of
    /// the capability grants must be the same set, every command must have
    /// a generated permission file (tauri-conventions §3.1), and each command
    /// is granted to exactly the window that uses it (wave 3.2 §5).
    #[test]
    fn handlers_manifest_and_capability_grants_are_the_same_set() {
        let handlers: BTreeSet<String> =
            between(include_str!("../lib.rs"), "generate_handler![", "]")
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|path| path.rsplit("::").next().unwrap().to_owned())
                .collect();

        let manifest: BTreeSet<String> =
            between(include_str!("../../build.rs"), ".commands(&[", "])")
                .split(',')
                .map(|entry| entry.trim().trim_matches('"'))
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect();

        let (main, main_windows) = capability(include_str!("../../capabilities/default.json"));
        let (settings, settings_windows) =
            capability(include_str!("../../capabilities/settings.json"));
        assert_eq!(main_windows, ["main"]);
        assert_eq!(settings_windows, ["settings"]);

        let configuration: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(
            configuration["app"]["security"]["capabilities"],
            serde_json::json!(["main-plane-setup", "settings-window"])
        );

        let granted: BTreeSet<String> = main.union(&settings).cloned().collect();
        assert_eq!(handlers, manifest);
        assert_eq!(handlers, granted);

        let both: BTreeSet<String> = main.intersection(&settings).cloned().collect();
        let shared: BTreeSet<String> = SHARED.iter().map(|name| (*name).to_owned()).collect();
        assert_eq!(both, shared, "only read-only commands are shared");
        let settings_only: BTreeSet<String> = SETTINGS_ONLY
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        assert_eq!(
            settings
                .difference(&shared)
                .cloned()
                .collect::<BTreeSet<_>>(),
            settings_only
        );
        // Wave 3.3 commands and the windows that call them (wave 3.3 §5). A
        // command reaches the Settings window only with its call site there.
        for main_only in WAVE_3_3_MAIN_ONLY {
            assert!(main.contains(main_only), "{main_only}");
            assert!(!settings.contains(main_only), "{main_only}");
            assert!(handlers.contains(main_only), "{main_only}");
        }
        for main_only in ["open_settings", "open_plane_feed", "bind_harness_account"] {
            assert!(main.contains(main_only), "{main_only}");
            assert!(!settings.contains(main_only), "{main_only}");
        }

        let generated = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("permissions")
            .join("autogenerated");
        for command in &handlers {
            let toml = std::fs::read_to_string(generated.join(format!("{command}.toml")))
                .unwrap_or_else(|_| panic!("missing generated permission for {command}"));
            assert!(
                toml.starts_with("# Automatically generated - DO NOT EDIT!"),
                "{command}.toml was not generated by tauri-build"
            );
        }
    }

    /// The Settings window receives no feed, no dialog and no window-core
    /// permission (wave 3.2 §5).
    #[test]
    fn settings_window_grants_no_plugin_or_core_permission() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../../capabilities/settings.json")).unwrap();
        for permission in capability["permissions"].as_array().unwrap() {
            let permission = permission.as_str().unwrap();
            assert!(!permission.contains(':'), "{permission}");
        }
    }
}

#[cfg(test)]
mod restoration_tests {
    use std::sync::Arc;

    use uuid::Uuid;

    use super::{
        keystore::{
            tests::{CountingStore, Fail},
            IdentityKeys,
        },
        planes::{spawner::fake::FakeSpawner, PlaneId},
        JetBridge,
    };

    fn bridge(directory: &std::path::Path) -> JetBridge {
        JetBridge::for_test(
            directory,
            Uuid::from_u128(1),
            Arc::new(FakeSpawner::default()),
            Arc::new(IdentityKeys::new(Arc::new(CountingStore::new(
                Fail::Nothing,
            )))),
        )
    }

    /// Turning off "Reopen the last task" hides the saved selection from the
    /// Plane registry snapshot without deleting it.
    #[test]
    fn restoration_follows_the_desktop_preference() {
        let directory = tempfile::tempdir().unwrap();
        let conversation = Uuid::from_u128(9);
        std::fs::write(
            directory.path().join("last-conversation"),
            format!("{conversation}\nlocal\n"),
        )
        .unwrap();

        let restoring = bridge(directory.path());
        assert_eq!(
            restoring.restorable_selection().unwrap(),
            Some((conversation, PlaneId::Local))
        );

        std::fs::write(
            directory.path().join("desktop-preferences.json"),
            r#"{"reopenLastTask":false}"#,
        )
        .unwrap();
        let declining = bridge(directory.path());
        assert_eq!(declining.restorable_selection().unwrap(), None);
        assert_eq!(
            declining.conversations.restored_selection().unwrap(),
            Some((conversation, PlaneId::Local))
        );
    }
}
