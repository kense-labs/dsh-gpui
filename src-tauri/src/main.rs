//! Tauri desktop host for KWork.

mod acp;
mod auth;
mod browser;
mod desktop;
mod embedded_script;
mod history;
mod preferences;
#[allow(dead_code)]
#[path = "../../ui/src/shortcuts.rs"]
mod shortcut_bindings;
mod shortcuts;
mod terminal;
mod workspace;

use acp::AcpService;
use auth::AuthService;
use browser::BrowserService;
use tauri::{Manager, RunEvent};
use terminal::TerminalService;

fn private_child_env_keys() -> impl Iterator<Item = std::ffi::OsString> {
    std::env::vars_os().filter_map(|(key, _)| {
        let normalized = key.to_string_lossy().to_ascii_uppercase();
        (normalized.starts_with("DSH_")
            || ["KEY", "PASSWORD", "SECRET", "TOKEN"]
                .iter()
                .any(|part| normalized.contains(part)))
        .then_some(key)
    })
}

fn main() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(desktop::setup)
        .manage(AcpService::default())
        .manage(AuthService::default())
        .manage(BrowserService::default())
        .manage(shortcuts::ShortcutService::default())
        .manage(TerminalService::default())
        .manage(workspace::watch::WorkspaceWatches::default())
        .register_uri_scheme_protocol("dsh-preview", |context, request| {
            use tauri::http::{Method, Response, StatusCode};
            let bootstrap = context.webview_label() == "main"
                && request.method() == Method::GET
                && request.uri().path() == "/";
            Response::builder()
                .status(if bootstrap {
                    StatusCode::OK
                } else {
                    StatusCode::NOT_FOUND
                })
                .header("Content-Type", "text/html; charset=utf-8")
                .header("Content-Security-Policy", "sandbox allow-scripts")
                .body(
                    if bootstrap {
                        include_str!("../../ui/html-preview.html")
                    } else {
                        ""
                    }
                    .as_bytes(),
                )
                .expect("valid HTML preview response")
        })
        .on_page_load(browser::shell_page_load)
        .on_window_event(browser::shell_window_event);
    #[cfg(target_os = "macos")]
    let builder = builder
        .on_window_event(desktop::window_chrome::handle_event)
        .on_web_content_process_terminate(browser::shell_process_terminated);
    let app = builder
        .invoke_handler(tauri::generate_handler![
            acp::model_providers,
            acp::runtime_settings,
            acp::web_settings,
            acp::plugin_center,
            acp::plugin_install,
            acp::schedule_tasks,
            acp::start_service,
            acp::stop_service,
            acp::new_session,
            acp::list_sessions,
            acp::resume_session,
            acp::rename_session,
            acp::fork_session,
            history::load_session_history,
            history::plan::read_session_plans,
            history::export::session_markdown,
            history::export::archive::download_session_log,
            history::peek_session_titles,
            history::list_local_sessions,
            acp::close_session,
            acp::set_session_archived,
            acp::prompt,
            acp::cancel_session,
            acp::set_config_option,
            acp::respond_permission,
            acp::plan_mode,
            acp::permission_mode,
            acp::respond_plan_review,
            acp::question_state,
            acp::respond_question,
            acp::record_session_feedback,
            acp::message_feedback,
            workspace::pick_workspace_directory,
            workspace::list_workspace_files,
            workspace::document::read_workspace_text,
            workspace::document::read_workspace_html,
            workspace::document::read_workspace_html_asset,
            workspace::watch::watch_workspace_directory,
            workspace::watch::watch_workspace_file,
            workspace::watch::watch_workspace_html_asset,
            workspace::watch::unwatch_workspace_path,
            workspace::open_start_tool,
            #[cfg(target_os = "macos")]
            workspace::applications::workspace_applications,
            #[cfg(target_os = "macos")]
            workspace::applications::open_workspace_application,
            workspace::open_browser_url,
            browser::browser_shell_generation,
            browser::present_embedded_browser,
            browser::close_embedded_browser,
            browser::embedded_browser_action,
            browser::focus_browser_shell,
            #[cfg(target_os = "macos")]
            browser::set_html_preview_focus,
            preferences::load_workspace_preferences,
            preferences::save_workspace_preferences,
            preferences::load_developer_tools,
            preferences::save_developer_tools,
            shortcuts::load_shortcuts,
            shortcuts::save_shortcuts,
            shortcuts::close_shortcut_window,
            #[cfg(target_os = "macos")]
            shortcuts::set_shortcut_recording,
            desktop::start_window_dragging,
            desktop::configure_shell_window,
            desktop::document_system_font,
            desktop::chinese_system_font,
            desktop::show_document_copy_menu,
            terminal::terminal_font,
            terminal::terminal_shells,
            terminal::start_terminal,
            terminal::attach_terminal,
            terminal::rename_terminal,
            terminal::write_terminal,
            terminal::resize_terminal,
            terminal::stop_terminal,
            auth::start_auth,
            auth::auth_status,
            auth::save_api_key,
            auth::remove_api_key,
        ])
        .build(tauri::generate_context!())
        .expect("build KWork");

    app.run(|app, event| {
        #[cfg(target_os = "macos")]
        if matches!(event, RunEvent::Exit) {
            browser::uninstall_guest_input();
        }
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            app.state::<AcpService>().shutdown_now();
            app.state::<AuthService>().shutdown_now();
            terminal::shutdown(&app.state::<TerminalService>());
        }
    });
}
