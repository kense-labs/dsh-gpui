//! Atomic shortcut preferences and the accepted native routing table.

use crate::shortcut_bindings::{Document, Edit, Keymap, Platform, Snapshot};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Mutex, RwLock},
};
use tauri::{Manager, Webview};

pub(crate) const PLATFORM: Platform = if cfg!(target_os = "macos") {
    Platform::Mac
} else if cfg!(target_os = "windows") {
    Platform::Windows
} else {
    Platform::Linux
};

struct Accepted {
    snapshot: Snapshot,
    #[cfg(target_os = "macos")]
    keys: Keymap,
}

impl Accepted {
    fn new(snapshot: Snapshot) -> Self {
        Self {
            #[cfg(target_os = "macos")]
            keys: Keymap::new(&snapshot.document, PLATFORM),
            snapshot,
        }
    }
}

pub(crate) struct ShortcutService {
    state: RwLock<Accepted>,
    transaction: Mutex<()>,
    #[cfg(target_os = "macos")]
    recording: Mutex<(u64, u32, bool)>,
}

impl Default for ShortcutService {
    fn default() -> Self {
        Self {
            state: RwLock::new(Accepted::new(Snapshot::default())),
            transaction: Mutex::new(()),
            #[cfg(target_os = "macos")]
            recording: Mutex::default(),
        }
    }
}

impl ShortcutService {
    fn snapshot(&self) -> Snapshot {
        self.state
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .snapshot
            .clone()
    }

    fn publish(&self, snapshot: Snapshot) -> Snapshot {
        *self
            .state
            .write()
            .unwrap_or_else(|error| error.into_inner()) = Accepted::new(snapshot.clone());
        snapshot
    }

    fn load(&self, path: &Path) -> Snapshot {
        let _transaction = self
            .transaction
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut snapshot = self.snapshot();
        snapshot.revision += 1;
        match read_document(path) {
            Ok(document) => {
                snapshot.document = document;
                snapshot.error = None;
            }
            Err(error) => snapshot.error = Some(error.into()),
        }
        self.publish(snapshot)
    }

    fn save(&self, path: &Path, revision: u64, edit: &Edit) -> Result<Snapshot, String> {
        let _transaction = self
            .transaction
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut snapshot = self.snapshot();
        if snapshot.revision != revision {
            return Err("快捷键配置已更新。请重新打开快捷键后再保存。".into());
        }
        if snapshot.revision == 0 {
            return Err("快捷键尚未准备好，请稍后重试。".into());
        }
        if let Some(error) = snapshot.error {
            return Err(error);
        }
        let candidate = snapshot.document.edit(edit, PLATFORM)?;
        write_document(path, &candidate)
            .map_err(|_| "保存失败。原快捷键和当前草稿已保留，请重试。")?;
        snapshot.document = candidate;
        snapshot.revision += 1;
        Ok(self.publish(snapshot))
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn with_keys<T>(&self, read: impl FnOnce(&Keymap, u64) -> T) -> T {
        let state = self.state.read().unwrap_or_else(|error| error.into_inner());
        read(&state.keys, state.snapshot.revision)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn recording(&self, shell: u64) -> Option<u32> {
        let (owner, revision, active) = *self
            .recording
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        (owner == shell && active).then_some(revision)
    }
}

fn read_document(path: &Path) -> Result<Document, &'static str> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Document::default()),
        Err(_) => return Err("无法读取 keybindings.json。请检查访问权限后重新启动 Harness。"),
    };
    let invalid =
        "keybindings.json 的快捷键配置内容损坏。请先备份并修复该配置，然后重新启动 Harness。";
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| invalid)?;
    if value["schemaVersion"]
        .as_f64()
        .is_some_and(|version| version > 2.)
    {
        return Err("keybindings.json 的快捷键配置由更新版本创建。请升级 Harness 后重试。");
    }
    let document: Document = serde_json::from_value(value).map_err(|_| invalid)?;
    document.valid().then_some(document).ok_or(invalid)
}

fn write_document(path: &Path, document: &Document) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("shortcut directory unavailable"))?;
    fs::create_dir_all(directory)?;
    let temporary = directory.join(format!(".keybindings-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| {
        serde_json::to_writer_pretty(&mut file, document)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn settings_path(webview: &Webview) -> Result<PathBuf, String> {
    if webview.label() != "main" {
        return Err("只有主界面可以访问快捷键设置。".into());
    }
    webview
        .app_handle()
        .path()
        .app_config_dir()
        .map(|directory| directory.join("keybindings.json"))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn load_shortcuts(webview: Webview) -> Result<Snapshot, String> {
    let path = settings_path(&webview)?;
    tokio::task::spawn_blocking(move || Ok(webview.state::<ShortcutService>().load(&path)))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn save_shortcuts(
    webview: Webview,
    revision: u64,
    edit: Edit,
) -> Result<Snapshot, String> {
    let path = settings_path(&webview)?;
    tokio::task::spawn_blocking(move || {
        webview
            .state::<ShortcutService>()
            .save(&path, revision, &edit)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) fn close_shortcut_window(webview: Webview, revision: u64) -> Result<(), String> {
    if webview.label() != "main" {
        return Ok(());
    }
    let service = webview.state::<ShortcutService>();
    let accepted = service
        .state
        .read()
        .unwrap_or_else(|error| error.into_inner());
    if revision == 0 || accepted.snapshot.revision != revision {
        return Ok(());
    }
    drop(accepted);
    let window = webview.window();
    if window.is_focused().unwrap_or(false) {
        window.close().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub(crate) fn set_shortcut_recording(
    webview: Webview,
    shell: u64,
    revision: u32,
    active: bool,
) -> bool {
    if webview.label() != "main"
        || shell != crate::browser::current_browser_shell(webview.app_handle())
    {
        return false;
    }
    let service = webview.state::<ShortcutService>();
    let mut recording = service
        .recording
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if recording.0 != shell || recording.1 < revision {
        *recording = (shell, revision, active);
    }
    recording.0 == shell && recording.1 == revision
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shortcut_bindings::{ALT, Command, Gesture, HeldKeys, META};

    #[test]
    fn atomically_rebinds_chords_and_preserves_other_profiles() {
        let directory =
            std::env::temp_dir().join(format!("dsh-shortcuts-{}", uuid::Uuid::new_v4()));
        let path = directory.join("keybindings.json");
        let service = ShortcutService::default();
        let mut document = Document::default();
        document
            .profiles
            .insert("web:macos".into(), Default::default());
        write_document(&path, &document).unwrap();
        let first = service.load(&path);
        let primary = if PLATFORM == Platform::Mac {
            META
        } else {
            crate::shortcut_bindings::CONTROL
        };
        let chord = Gesture::new("KeyJ", PLATFORM.chords().then_some("KeyH"), primary).unwrap();
        let edit = Edit::Set {
            id: "page.refresh".into(),
            binding: Some(chord.to_binding()),
        };
        let saved = service.save(&path, first.revision, &edit).unwrap();
        assert!(
            service
                .save(&path, first.revision, &Edit::ResetAll)
                .is_err()
        );
        assert!(saved.document.profiles.contains_key("web:macos"));
        let map = Keymap::new(&read_document(&path).unwrap(), PLATFORM);
        assert_eq!(map.find("KeyR", None, primary), None);
        assert_eq!(
            map.find("KeyF", None, primary | ALT),
            Some(Command::ForkSession)
        );
        let mut held = HeldKeys::default();
        if PLATFORM.chords() {
            assert_eq!(held.press("KeyJ", primary, &map, true), None);
        }
        assert_eq!(
            held.press(
                if PLATFORM.chords() { "KeyH" } else { "KeyJ" },
                primary,
                &map,
                PLATFORM.chords()
            ),
            Some(Command::Refresh)
        );
        let conflict = Edit::Set {
            id: "page.refresh".into(),
            binding: Some(Gesture::new("KeyW", None, primary).unwrap().to_binding()),
        };
        assert!(
            service
                .save(&path, saved.revision, &conflict)
                .unwrap_err()
                .contains("关闭页面")
        );
        let fixed = Edit::Set {
            id: "page.refresh".into(),
            binding: Some(Gesture::new("Equal", None, primary).unwrap().to_binding()),
        };
        assert!(
            service
                .save(&path, saved.revision, &fixed)
                .unwrap_err()
                .contains("增大正文字号")
        );
        let removed = service
            .save(
                &path,
                saved.revision,
                &Edit::Set {
                    id: "page.refresh".into(),
                    binding: None,
                },
            )
            .unwrap();
        assert_eq!(
            Keymap::new(&removed.document, PLATFORM).find("KeyR", None, primary),
            None
        );
        let reset = service
            .save(&path, removed.revision, &Edit::ResetAll)
            .unwrap();
        assert_eq!(
            Keymap::new(&reset.document, PLATFORM).find("KeyR", None, primary),
            Some(Command::Refresh)
        );
        assert_eq!(reset.document.modified_count(PLATFORM), 0);
        fs::write(&path, br#"{"schemaVersion":3,"profiles":{}}"#).unwrap();
        let future = service.load(&path);
        assert!(
            future
                .error
                .as_deref()
                .is_some_and(|error| error.contains("更新版本"))
        );
        assert!(
            service
                .save(&path, future.revision, &Edit::ResetAll)
                .is_err()
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            br#"{"schemaVersion":3,"profiles":{}}"#
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
