//! Native application identity and status-item behavior.

#[cfg(target_os = "macos")]
mod document_selection;
#[cfg(target_os = "macos")]
mod system_fonts;
#[cfg(target_os = "macos")]
pub(crate) mod window_chrome;

use std::sync::Mutex;
use tauri::{
    App, AppHandle, LogicalSize, Manager, State,
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};

pub(crate) struct ShellWindow(Mutex<bool>);

pub(crate) fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let open = MenuItem::with_id(app, "open", "打开 KWork", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 KWork", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let tray = TrayIconBuilder::with_id("kwork-status")
        .icon(Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?)
        .icon_as_template(true)
        .tooltip("KWork")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    app.manage(tray);
    app.manage(ShellWindow(Mutex::new(true)));
    #[cfg(target_os = "macos")]
    {
        if let Some(webview) = app.get_webview("main") {
            window_chrome::install(&webview)?;
        }
        crate::browser::install_guest_input(app.handle().clone());
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn configure_shell_window(
    webview: tauri::Webview,
    welcome: bool,
    dark: Option<bool>,
    state: State<'_, ShellWindow>,
) -> Result<(), String> {
    if webview.label() != "main" {
        return Err("Window layout is available to the main interface".into());
    }
    let window = webview.window();
    #[cfg(target_os = "macos")]
    window
        .set_theme(dark.filter(|_| !welcome).map(|dark| {
            if dark {
                tauri::Theme::Dark
            } else {
                tauri::Theme::Light
            }
        }))
        .map_err(|error| error.to_string())?;
    #[cfg(not(target_os = "macos"))]
    let _ = dark;
    let mut current = state.0.lock().map_err(|error| error.to_string())?;
    if *current == welcome {
        #[cfg(target_os = "macos")]
        window_chrome::refresh(&webview, welcome, true).map_err(|error| error.to_string())?;
        return Ok(());
    }
    let apply = || -> tauri::Result<()> {
        if welcome {
            if window.is_fullscreen()? {
                window.set_fullscreen(false)?;
            }
            window.unmaximize()?;
            window.set_min_size(None::<LogicalSize<f64>>)?;
            window.set_size(LogicalSize::new(600., 700.))?;
            window.set_resizable(false)?;
            window.set_maximizable(false)?;
        } else {
            window.set_resizable(true)?;
            window.set_maximizable(true)?;
            window.set_size(LogicalSize::new(1280., 820.))?;
            window.set_min_size(Some(LogicalSize::new(520., 600.)))?;
        }
        window.center()
    };
    apply().map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    window_chrome::configure(&webview, welcome).map_err(|error| error.to_string())?;
    *current = welcome;
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

#[tauri::command]
pub(crate) async fn document_system_font(
    webview: tauri::Webview,
) -> Result<tauri::ipc::Response, String> {
    if webview.label() != "main" {
        return Err("System fonts are available to the main interface".into());
    }
    #[cfg(target_os = "macos")]
    return std::fs::read("/System/Library/Fonts/SFNS.ttf")
        .map(tauri::ipc::Response::new)
        .map_err(|error| error.to_string());
    #[cfg(not(target_os = "macos"))]
    Err("The macOS system font is unavailable on this platform".into())
}

#[tauri::command]
pub(crate) async fn chinese_system_font(
    webview: tauri::Webview,
) -> Result<tauri::ipc::Response, String> {
    if webview.label() != "main" {
        return Err("System fonts are available to the main interface".into());
    }
    #[cfg(target_os = "macos")]
    return tauri::async_runtime::spawn_blocking(system_fonts::chinese_collection)
        .await
        .map_err(|error| error.to_string())?
        .map(tauri::ipc::Response::new);
    #[cfg(not(target_os = "macos"))]
    Err("The macOS system font is unavailable on this platform".into())
}

#[tauri::command]
pub(crate) async fn show_document_copy_menu(
    webview: tauri::Webview,
    text: String,
    x: f64,
    y: f64,
) -> Result<(), String> {
    if webview.label() != "main" || text.is_empty() || !x.is_finite() || !y.is_finite() {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    return document_selection::show(webview, text, x, y);
    #[cfg(not(target_os = "macos"))]
    Err("当前平台暂不支持原生预览复制菜单".into())
}
