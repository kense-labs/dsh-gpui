//! Tauri desktop host for the KUI/GPUI DeepSeek Harness client.

mod acp;

use acp::AcpService;
use tauri::{Manager, RunEvent};

fn main() {
    let app = tauri::Builder::default()
        .manage(AcpService::default())
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
        ])
        .build(tauri::generate_context!())
        .expect("build the DeepSeek Harness GPUI application");

    app.run(|app, event| {
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            app.state::<AcpService>().shutdown_now();
        }
    });
}
