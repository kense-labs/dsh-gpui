//! Native subscriptions for workspace directories and file paths.

use super::{
    document::{DocumentSnapshot, document_path, html_asset_path},
    tree_target,
};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, Runtime, Webview};

#[derive(Clone, Copy, Deserialize, Eq, Hash, PartialEq)]
pub(crate) struct WatchKey {
    shell: u64,
    id: u64,
}

#[derive(Default)]
pub(crate) struct WorkspaceWatches(Mutex<HashMap<WatchKey, RecommendedWatcher>>);

/// A replaced shell cannot leave subscriptions running or claim another shell's id.
pub(crate) fn retire_shell<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let service = app.state::<WorkspaceWatches>();
        let Ok(mut watches) = service.0.lock() else {
            return;
        };
        let shell = crate::browser::current_browser_shell(&app);
        let retired: Vec<_> = watches
            .extract_if(|key, _| key.shell != shell)
            .map(|(_, watch)| watch)
            .collect();
        drop(watches);
        drop(retired);
    });
}

#[tauri::command]
pub(crate) async fn watch_workspace_directory(
    webview: Webview,
    workspace: String,
    directory: String,
    key: WatchKey,
) -> Result<(), String> {
    if webview.label() != "main" {
        return Err("只有主界面可以监听工作区文件".into());
    }
    let app = webview.app_handle().clone();
    tokio::task::spawn_blocking(move || {
        if key.shell != crate::browser::current_browser_shell(&app) {
            return Ok(());
        }
        let (_, path) = tree_target(&workspace, &directory)?;
        let app_for_event = app.clone();
        let watcher =
            watch_directory(path, move || {
                let _ = app_for_event.emit_to("main", "acp-event", json!({
                "kind": "workspace_directory_changed", "shell": key.shell, "watchId": key.id,
            }));
            })
            .map_err(|error| error.to_string())?;
        register(&app, key, watcher)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Serialize)]
pub(crate) struct DocumentObservation {
    sequence: u64,
    #[serde(flatten)]
    snapshot: DocumentSnapshot,
}

fn observe(path: &Path, sequence: &Mutex<u64>) -> DocumentObservation {
    // The initial observation and events share the same ordering, even if an
    // event reaches the webview ahead of the command reply.
    let mut sequence = sequence
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *sequence += 1;
    DocumentObservation {
        sequence: *sequence,
        snapshot: DocumentSnapshot::observe(path),
    }
}

#[tauri::command]
pub(crate) async fn watch_workspace_file(
    webview: Webview,
    workspace: String,
    path: String,
    key: WatchKey,
) -> Result<DocumentObservation, String> {
    follow_workspace_file(webview, key, false, move || {
        document_path(&workspace, &path).map_err(|error| format!("{error:?}"))
    })
    .await
}

#[tauri::command]
pub(crate) async fn watch_workspace_html_asset(
    webview: Webview,
    workspace: String,
    base_file: String,
    path: String,
    key: WatchKey,
) -> Result<DocumentObservation, String> {
    follow_workspace_file(webview, key, true, move || {
        html_asset_path(&workspace, &base_file, &path).map_err(|error| format!("{error:?}"))
    })
    .await
}

async fn follow_workspace_file(
    webview: Webview,
    key: WatchKey,
    asset: bool,
    resolve: impl FnOnce() -> Result<PathBuf, String> + Send + 'static,
) -> Result<DocumentObservation, String> {
    if webview.label() != "main" {
        return Err("只有主界面可以监听工作区文件".into());
    }
    let app = webview.app_handle().clone();
    tokio::task::spawn_blocking(move || {
        if key.shell != crate::browser::current_browser_shell(&app) {
            return Err("工作区界面已切换".into());
        }
        let path = resolve()?;
        let sequence = Arc::new(Mutex::new(0));
        let event_sequence = sequence.clone();
        let event_path = path.clone();
        let event_app = app.clone();
        let notify = move || {
            let observation = observe(&event_path, &event_sequence);
            let _ = event_app.emit_to("main", "acp-event", json!({
                "kind": "workspace_file_changed", "shell": key.shell, "watchId": key.id, "observation": observation,
            }));
        };
        let watcher = if asset {
            let mut entry = path.as_path();
            while let Some(parent) = entry.parent() {
                if parent.is_dir() {
                    break;
                }
                entry = parent;
            }
            watch_file(entry.to_owned(), notify)
        } else {
            watch_file(path.clone(), notify)
        }.map_err(|error| error.to_string())?;
        register(&app, key, watcher)?;
        Ok(observe(&path, &sequence))
    })
    .await
    .map_err(|error| error.to_string())?
}

fn register(app: &AppHandle, key: WatchKey, watcher: RecommendedWatcher) -> Result<(), String> {
    let service = app.state::<WorkspaceWatches>();
    let mut watches = service.0.lock().map_err(|_| "工作区监听状态锁失败")?;
    if key.shell == crate::browser::current_browser_shell(app) {
        watches.entry(key).or_insert(watcher);
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn unwatch_workspace_path(webview: Webview, key: WatchKey) -> Result<(), String> {
    if webview.label() != "main" {
        return Err("只有主界面可以关闭工作区监听".into());
    }
    let app = webview.app_handle().clone();
    tokio::task::spawn_blocking(move || {
        let service = app.state::<WorkspaceWatches>();
        let watch = service
            .0
            .lock()
            .map_err(|_| "工作区监听状态锁失败")?
            .remove(&key);
        drop(watch);
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

fn watch_directory(
    path: PathBuf,
    changed: impl Fn() + Send + 'static,
) -> notify::Result<RecommendedWatcher> {
    let directory = path.clone();
    watch(
        &path,
        move |event| affects_directory(&directory, event),
        changed,
    )
}

fn watch_file(
    path: PathBuf,
    changed: impl Fn() + Send + 'static,
) -> notify::Result<RecommendedWatcher> {
    // Follow the address through an atomic save or deletion and recreation.
    let parent = path
        .parent()
        .ok_or_else(notify::Error::path_not_found)?
        .to_owned();
    watch(
        &parent,
        move |event| {
            event.need_rescan()
                || (!matches!(event.kind, EventKind::Access(_)) && event.paths.contains(&path))
        },
        changed,
    )
}

fn watch(
    directory: &Path,
    affects: impl Fn(&Event) -> bool + Send + 'static,
    changed: impl Fn() + Send + 'static,
) -> notify::Result<RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
        if event.as_ref().map_or(true, &affects) {
            changed();
        }
    })?;
    watcher.watch(directory, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

fn affects_directory(directory: &Path, event: &Event) -> bool {
    event.need_rescan()
        || (!matches!(event.kind, EventKind::Access(_))
            && event
                .paths
                .iter()
                .any(|path| path == directory || path.parent() == Some(directory)))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    #[ignore = "requires a macOS FSEvents connection outside the restricted sandbox"]
    fn fsevents_delivers_changes_for_direct_entries_until_the_watch_is_dropped() {
        let fixture =
            Fixture(std::env::temp_dir().join(format!("dsh-watch-{}", uuid::Uuid::new_v4())));
        let child = fixture.0.join("child");
        fs::create_dir_all(&child).unwrap();
        let root = fixture.0.canonicalize().unwrap();
        let child = root.join("child");

        let (root_tx, root_rx) = mpsc::channel();
        let root_watch = watch_directory(root.clone(), move || {
            let _ = root_tx.send(());
        })
        .unwrap();
        let (child_tx, child_rx) = mpsc::channel();
        let child_watch = watch_directory(child.clone(), move || {
            let _ = child_tx.send(());
        })
        .unwrap();
        let timeout = Duration::from_secs(6);
        let quiet = Duration::from_millis(350);

        let inner = child.join("inner.txt");
        fs::write(&inner, "nested change").unwrap();
        child_rx
            .recv_timeout(timeout)
            .expect("child received its file creation");
        assert_eq!(root_rx.recv_timeout(quiet), Err(RecvTimeoutError::Timeout));

        let direct = root.join("direct.txt");
        fs::write(&direct, "new").unwrap();
        root_rx
            .recv_timeout(timeout)
            .expect("root received file creation");
        while root_rx.recv_timeout(quiet).is_ok() {}
        fs::write(&direct, "updated").unwrap();
        root_rx
            .recv_timeout(timeout)
            .expect("root received file content modification");
        while root_rx.recv_timeout(quiet).is_ok() {}
        let moved = root.join("renamed.txt");
        fs::rename(&direct, &moved).unwrap();
        root_rx
            .recv_timeout(timeout)
            .expect("root received file rename");
        while root_rx.recv_timeout(quiet).is_ok() {}
        fs::remove_file(&moved).unwrap();
        root_rx
            .recv_timeout(timeout)
            .expect("root received file removal");

        drop(child_watch);
        while child_rx.try_recv().is_ok() {}
        assert_eq!(
            child_rx.recv_timeout(quiet),
            Err(RecvTimeoutError::Disconnected)
        );
        drop(root_watch);
    }

    #[test]
    #[ignore = "requires a macOS FSEvents connection outside the restricted sandbox"]
    fn file_watch_tracks_its_path_across_atomic_replacement_and_recreation() {
        use super::super::document::ReadFailure;

        let fixture =
            Fixture(std::env::temp_dir().join(format!("dsh-file-watch-{}", uuid::Uuid::new_v4())));
        fs::create_dir(&fixture.0).unwrap();
        let root = fixture.0.canonicalize().unwrap();
        let path = root.join("selected.txt");
        let sequence = Arc::new(Mutex::new(0));
        let callback_sequence = sequence.clone();
        let callback_path = path.clone();
        let (tx, rx) = mpsc::channel();
        let watcher = watch_file(path.clone(), move || {
            let _ = tx.send(observe(&callback_path, &callback_sequence));
        })
        .unwrap();

        let missing = observe(&path, &sequence);
        assert!(matches!(
            missing.snapshot,
            DocumentSnapshot::Failed {
                failure: ReadFailure::NotFound
            }
        ));
        fs::write(root.join("unrelated.txt"), "elsewhere").unwrap();
        let quiet = Duration::from_millis(350);
        assert!(matches!(
            rx.recv_timeout(quiet),
            Err(RecvTimeoutError::Timeout)
        ));

        let next_live = |after, previous: &str| {
            let deadline = std::time::Instant::now() + Duration::from_secs(6);
            loop {
                let observation = rx
                    .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                    .expect("file metadata changed");
                if observation.sequence > after
                    && let DocumentSnapshot::Live { version, .. } = observation.snapshot
                    && version != previous
                {
                    return (observation.sequence, version);
                }
            }
        };
        fs::write(&path, "first").unwrap();
        let (first_seq, first_version) = next_live(missing.sequence, "");
        let replacement = root.join("save.tmp");
        fs::write(&replacement, "atomic replacement").unwrap();
        fs::rename(&replacement, &path).unwrap();
        let (replacement_seq, replacement_version) = next_live(first_seq, &first_version);

        fs::remove_file(&path).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        let deleted = loop {
            let notice = rx
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .expect("removal metadata");
            if notice.sequence > replacement_seq
                && matches!(
                    notice.snapshot,
                    DocumentSnapshot::Failed {
                        failure: ReadFailure::NotFound
                    }
                )
            {
                break notice.sequence;
            }
        };
        fs::write(&path, "recreated").unwrap();
        next_live(deleted, &replacement_version);

        drop(watcher);
        while rx.try_recv().is_ok() {}
        assert!(matches!(
            rx.recv_timeout(quiet),
            Err(RecvTimeoutError::Disconnected)
        ));
    }
}
