mod jet;

use jet::JetBridge;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
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
            jet::load_setup,
            jet::preview_project,
            jet::register_project,
            jet::preview_project_removal,
            jet::remove_project,
            jet::bind_harness_account,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Jet desktop application");
}
