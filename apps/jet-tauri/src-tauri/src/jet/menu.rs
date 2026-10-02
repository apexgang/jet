//! Native desktop menus. Commands describe UI intent; all mutations still use
//! the existing validated IPC boundaries and confirmation surfaces.
use tauri::{
    menu::{Menu, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder},
    Emitter, Manager,
};

pub(crate) fn install(app: &tauri::App) -> tauri::Result<()> {
    let item = |id: &str, text: &str, key: &str| {
        MenuItemBuilder::with_id(id, text)
            .accelerator(key)
            .build(app)
    };
    let application = SubmenuBuilder::new(app, "Jet")
        .item(&item("settings", "Settings…", "CmdOrCtrl+,")?)
        .separator()
        .item(&PredefinedMenuItem::quit(app, Some("Quit Jet"))?)
        .build()?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&item("new-task", "New task", "CmdOrCtrl+N")?)
        .item(&item("project", "Add Project…", "CmdOrCtrl+Shift+O")?)
        .separator()
        .item(&PredefinedMenuItem::close_window(
            app,
            Some("Close window"),
        )?)
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .item(&PredefinedMenuItem::undo(app, None)?)
        .item(&PredefinedMenuItem::redo(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::cut(app, None)?)
        .item(&PredefinedMenuItem::copy(app, None)?)
        .item(&PredefinedMenuItem::paste(app, None)?)
        .item(&PredefinedMenuItem::select_all(app, None)?)
        .separator()
        .item(&item("search", "Find a task…", "CmdOrCtrl+K")?)
        .build()?;
    let view = SubmenuBuilder::new(app, "View")
        .text("sidebar", "Toggle sidebar")
        .text("details", "Toggle work details")
        .separator()
        .text("changes", "Changes")
        .text("files", "Files")
        .text("terminal", "Terminal")
        .text("run", "Run details")
        .text("delivery", "Deliver changes")
        .separator()
        .text("planes", "Connections")
        .text("trash", "Trash")
        .build()?;
    app.set_menu(Menu::with_items(app, &[&application, &file, &edit, &view])?)?;
    app.on_menu_event(|app, event| {
        let command = event.id().as_ref();
        if matches!(
            command,
            "settings"
                | "new-task"
                | "project"
                | "search"
                | "sidebar"
                | "details"
                | "changes"
                | "files"
                | "terminal"
                | "run"
                | "delivery"
                | "planes"
                | "trash"
        ) {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
                let _ = window.emit("jet:desktop-command", command);
            }
        }
    });
    Ok(())
}
