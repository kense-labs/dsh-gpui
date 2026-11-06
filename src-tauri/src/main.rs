//! Tauri desktop host for the KUI/GPUI DeepSeek Harness client.

mod acp;
mod auth;
mod desktop;
mod preferences;
mod workspace;

use acp::AcpService;
use auth::AuthService;
use tauri::{Manager, RunEvent};

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(desktop::setup)
        .manage(AcpService::default())
        .manage(AuthService::default())
        .invoke_handler(tauri::generate_handler![
            acp::start_service,
            acp::stop_service,
            acp::new_session,
            acp::list_sessions,
            acp::resume_session,
            acp::close_session,
            acp::prompt,
            acp::cancel_session,
            acp::set_config_option,
            acp::respond_permission,
            workspace::list_workspace_files,
            workspace::open_start_tool,
            preferences::load_workspace_preferences,
            preferences::save_workspace_preferences,
            desktop::start_window_dragging,
            auth::start_auth,
            auth::auth_status,
            auth::auth_profile,
            auth::auth_avatar,
            auth::save_api_key,
            auth::remove_api_key,
            auth::start_sign_in,
            auth::cancel_sign_in,
            auth::sign_out,
            auth::open_sign_in,
            auth::open_account_page,
        ])
        .build(tauri::generate_context!())
        .expect("build the DeepSeek Harness GPUI application");

    app.run(|app, event| {
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            app.state::<AcpService>().shutdown_now();
            app.state::<AuthService>().shutdown_now();
        }
    });
}
