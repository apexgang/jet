fn main() {
    // ASVS 1.2.2: register only the app commands used by the main webview so
    // Tauri generates explicit allow/deny permission pairs for each boundary.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "open_plane_feed",
            "load_setup",
            "preview_project",
            "register_project",
            "preview_project_removal",
            "remove_project",
            "bind_harness_account",
            "load_run_supervision",
            "withdraw_turn",
            "interrupt_turn",
            "stop_run",
            "authorize_approval_retry",
        ]),
    ))
    .expect("failed to build the Tauri command manifest");
}
