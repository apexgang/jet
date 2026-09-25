mod jet;

use jet::{
    local_service::LocalServiceState, presentation::PresentationState, updates::AppUpdateState,
    window_state, JetBridge,
};
use tauri::{Manager, RunEvent, WindowEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before Tauri reads them, and before any thread starts.
    jet::updates::clear_inherited_appimage_variables();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let home_directory = app.path().home_dir()?;
            let app_data_directory = app.path().app_data_dir()?;
            // The bridge creates the app data directory (0700) first.
            let bridge = JetBridge::for_local_plane(&home_directory, &app_data_directory)?;
            let service = LocalServiceState::for_app(
                app.handle(),
                &bridge,
                &home_directory,
                &app_data_directory,
            );
            // The updater plugin exists only in release builds that carry
            // `plugins.updater`; development builds start without it.
            let updates_configured = jet::updates::register(app.handle());
            app.manage(AppUpdateState::for_app(
                app.handle(),
                updates_configured,
                service.subscribe(),
                &home_directory,
            ));
            app.manage(service);
            app.manage(bridge);
            app.manage(PresentationState::new(&app_data_directory));
            app.manage(window_state::WindowGeometryState::new(&app_data_directory));
            if let Some(window) = app.get_webview_window(window_state::MAIN_LABEL) {
                window_state::present(&window, &app.state());
            }
            // Provisioning runs in the background; setup never waits for it.
            jet::local_service::launch(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let Some(geometry) = window.try_state::<window_state::WindowGeometryState>() {
                window_state::track(window, event, &geometry);
            }
            if matches!(event, WindowEvent::Destroyed) {
                if let Some(service) = window.try_state::<LocalServiceState>() {
                    service.window_destroyed(window.label());
                }
                if let Some(updates) = window.try_state::<AppUpdateState>() {
                    updates.window_destroyed(window.label());
                }
            }
            if matches!(event, WindowEvent::Destroyed)
                && window.label() == jet::settings_window::SETTINGS_LABEL
            {
                if let Some(bridge) = window.try_state::<JetBridge>() {
                    bridge.settings_window_closed();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            jet::open_plane_feed,
            jet::close_plane_feed,
            jet::planes::list_planes,
            jet::planes::load_plane_detail,
            jet::pairing::load_pairing,
            jet::pairing::set_pairing_gate,
            jet::pairing::open_pairing_offer,
            jet::pairing::confirm_pairing_request,
            jet::pairing::prepare_paired_client_change,
            jet::pairing::execute_paired_client_change,
            jet::enrollment::add_remote_plane,
            jet::enrollment::repair_remote_plane,
            jet::enrollment::claim_remote_pairing,
            jet::enrollment::complete_remote_pairing,
            jet::enrollment::cancel_remote_pairing,
            jet::enrollment::forget_remote_plane,
            jet::delivery::load_deliveries,
            jet::delivery::prepare_delivery,
            jet::delivery::execute_delivery,
            jet::delivery::prepare_delivery_acknowledgement,
            jet::notifications::load_notification_settings,
            jet::notifications::set_notification_settings,
            jet::settings_window::open_settings,
            jet::settings_window::watch_settings_navigation,
            jet::settings_window::remember_settings_pane,
            jet::settings_window::close_settings,
            jet::preferences::load_desktop_preferences,
            jet::preferences::set_desktop_preferences,
            jet::settings::load_settings,
            jet::settings::prepare_setting_change,
            jet::settings::apply_settings_change,
            jet::settings::load_work_context,
            jet::settings::watch_settings_changes,
            jet::agents::load_agents,
            jet::agents::prepare_account_bind,
            jet::agents::load_account_detail,
            jet::agents::load_usage_history,
            jet::agents::prepare_auto_continue,
            jet::agents::prepare_account_unbind,
            jet::agents::prepare_craft_disable,
            jet::agents::pick_local_craft_source,
            jet::agents::discover_craft,
            jet::extensions::load_extension_catalog,
            jet::extensions::inspect_extension,
            jet::extensions::prepare_extension_change,
            jet::extensions::load_extension_change,
            jet::load_setup,
            jet::preview_project,
            jet::register_project,
            jet::preview_project_removal,
            jet::remove_project,
            jet::bind_harness_account,
            jet::load_conversations,
            jet::search_conversations,
            jet::load_conversation,
            jet::create_conversation,
            jet::start_run,
            jet::submit_turn,
            jet::load_run_supervision,
            jet::withdraw_turn,
            jet::interrupt_turn,
            jet::stop_run,
            jet::authorize_approval_retry,
            jet::load_work_panel,
            jet::load_more_changes,
            jet::load_patch_chunk,
            jet::load_work_file,
            jet::save_work_file,
            jet::submit_file_review,
            jet::open_workspace_terminal,
            jet::close_workspace_terminal,
            jet::attach_workspace_terminal,
            jet::send_terminal_input,
            jet::resize_workspace_terminal,
            jet::detach_workspace_terminal,
            jet::system::collect_disposable_storage,
            jet::system::load_system_health,
            jet::retention::load_trash,
            jet::retention::load_trash_status,
            jet::retention::resolve_conversation_names,
            jet::retention::preview_trash,
            jet::retention::trash_conversation,
            jet::retention::restore_conversation,
            jet::retention::autodelete::load_autodelete_rules,
            jet::retention::autodelete::change_autodelete_rule,
            jet::system::recovery::prepare_recovery_action,
            jet::system::recovery::execute_recovery_action,
            jet::audit::load_security_audit,
            jet::audit::export_security_audit,
            jet::presentation::load_shell_presentation,
            jet::presentation::save_shell_presentation,
            jet::window_mode::toggle_main_window_fullscreen,
            jet::window_mode::close_main_window,
            jet::window_mode::quit_jet,
            jet::local_service::load_local_service,
            jet::local_service::watch_local_service,
            jet::local_service::repair_local_service,
            jet::local_service::prepare_local_service_rollback,
            jet::local_service::execute_local_service_rollback,
            jet::updates::load_app_update,
            jet::updates::watch_app_update,
            jet::updates::check_app_update,
            jet::updates::install_app_update,
            jet::updates::restart_after_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the Jet desktop application")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                if let Some(geometry) = app.try_state::<window_state::WindowGeometryState>() {
                    geometry.persist();
                }
            }
        });
}
