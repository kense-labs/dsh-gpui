//! Present the Session ZIP destination as an AppKit save sheet.

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSModalResponseCancel, NSModalResponseOK, NSSavePanel, NSWindow};
use objc2_foundation::{NSString, NSURL};
use std::cell::RefCell;
use std::path::PathBuf;
use tauri::Manager;

const DIALOG_ERROR: &str = "无法打开 Session ZIP 保存对话框。";

pub(super) async fn choose_destination(
    webview: tauri::Webview,
    filename: &str,
) -> Result<Option<PathBuf>, String> {
    let window = webview.window().clone();
    let downloads = webview.app_handle().path().download_dir().ok();
    let name = filename.to_owned();
    let (send, receive) = tokio::sync::oneshot::channel();
    window
        .clone()
        .run_on_main_thread(move || {
            let Some(mtm) = MainThreadMarker::new() else {
                let _ = send.send(Err(DIALOG_ERROR.to_owned()));
                return;
            };
            let native = window.ns_window().ok().and_then(|window| {
                // SAFETY: this closure runs on the main thread while Tauri owns the
                // window; `ns_window` returns that window's `NSWindow` pointer, so the
                // cast is to the correct type and `as_ref` rejects a null handle.
                unsafe { window.cast::<NSWindow>().as_ref() }
            });
            let Some(native) = native else {
                let _ = send.send(Err(DIALOG_ERROR.to_owned()));
                return;
            };
            let panel = NSSavePanel::savePanel(mtm);
            panel.setNameFieldStringValue(&NSString::from_str(&name));
            panel.setCanCreateDirectories(true);
            if let Some(directory) = downloads {
                let directory = NSString::from_str(&directory.to_string_lossy());
                let url = NSURL::fileURLWithPath_isDirectory(&directory, true);
                panel.setDirectoryURL(Some(&url));
            }
            let send = RefCell::new(Some(send));
            let retained = panel.clone();
            let complete = RcBlock::new(move |response| {
                let result = match response {
                    value if value == NSModalResponseCancel => Ok(None),
                    value if value == NSModalResponseOK => retained
                        .URL()
                        .and_then(|url| url.path())
                        .map(|path| Some(PathBuf::from(path.to_string())))
                        .ok_or_else(|| DIALOG_ERROR.to_owned()),
                    _ => Err(DIALOG_ERROR.to_owned()),
                };
                if let Some(send) = send.borrow_mut().take() {
                    let _ = send.send(result);
                }
            });
            panel.beginSheetModalForWindow_completionHandler(native, &complete);
        })
        .map_err(|_| DIALOG_ERROR.to_owned())?;
    receive.await.map_err(|_| DIALOG_ERROR.to_owned())?
}
