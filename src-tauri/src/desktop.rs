//! Native application identity and status-item behavior.

use tauri::{
    App, AppHandle, Manager,
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};

pub(crate) fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let open = MenuItem::with_id(app, "open", "打开 DeepSeek Harness", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 DeepSeek Harness", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let tray = TrayIconBuilder::with_id("deepseek-harness-status")
        .icon(Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?)
        .icon_as_template(true)
        .tooltip("DeepSeek Harness")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    app.manage(tray);
    Ok(())
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[tauri::command]
pub(crate) fn start_window_dragging(window: tauri::Window) -> Result<(), String> {
    window.start_dragging().map_err(|error| error.to_string())
}
