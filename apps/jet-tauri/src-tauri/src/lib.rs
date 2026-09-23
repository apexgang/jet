mod jet;

use jet::JetBridge;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let home_directory = app.path().home_dir()?;
            let app_data_directory = app.path().app_data_dir()?;
            app.manage(JetBridge::for_local_plane(
                &home_directory,
                &app_data_directory,
            )?);
            Ok(())
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
            jet::delivery::load_deliveries,
            jet::delivery::prepare_delivery,
            jet::delivery::execute_delivery,
            jet::delivery::prepare_delivery_acknowledgement,
            jet::notifications::load_notification_settings,
            jet::notifications::set_notification_settings,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Jet desktop application");
}
