fn main() {
    // ASVS 1.2.2: register the app command so Tauri generates an explicit
    // allow/deny permission pair instead of granting it to every webview.
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&["open_plane_feed"])),
    )
    .expect("failed to build the Tauri command manifest");
}
