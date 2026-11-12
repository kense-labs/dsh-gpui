//! Host-side PTY sessions for the right-dock embedded terminal.

mod screen;
mod shell;

use portable_pty::{ChildKiller, MasterPty, PtySize, native_pty_system};
use screen::ScreenStore;
use serde::Serialize;
use serde_json::json;
use shell::ShellProfile;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

struct LiveTerminal {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

#[derive(Default)]
pub struct TerminalService {
    inner: Mutex<TerminalState>,
}

#[derive(Default)]
struct TerminalState {
    records: HashMap<String, TerminalRecord>,
    closed: HashSet<String>,
    screens: Option<ScreenStore>,
}

struct TerminalRecord {
    session: String,
    title: String,
    live: Option<LiveTerminal>,
    size: PtySize,
    sequence: u64,
    screen_error: Option<String>,
    exit: Option<TerminalExit>,
}

const MAX_SESSION_TERMINALS: usize = 8;

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Operation {
    Output { data: String },
    Resize { cols: u16, rows: u16 },
    Exit { code: Option<u32> },
    Rename { title: String },
}

#[derive(Clone, Serialize)]
struct TerminalExit {
    code: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSnapshot {
    title: String,
    cols: u16,
    rows: u16,
    sequence: u64,
    screen: String,
    exit: Option<TerminalExit>,
}

impl TerminalRecord {
    fn snapshot(&self, screen: String) -> TerminalSnapshot {
        TerminalSnapshot {
            title: self.title.clone(),
            cols: self.size.cols,
            rows: self.size.rows,
            sequence: self.sequence,
            screen,
            exit: self.exit.clone(),
        }
    }
}

impl TerminalState {
    fn screens(&mut self, app: &AppHandle) -> Result<&ScreenStore, String> {
        if self.screens.is_none() {
            self.screens = Some(ScreenStore::load(app)?);
        }
        self.screens.as_ref().ok_or_else(|| "终端画面不可用".into())
    }

    fn snapshot(&self, id: &str, record: &TerminalRecord) -> Result<TerminalSnapshot, String> {
        if let Some(error) = &record.screen_error {
            return Err(error.clone());
        }
        let screens = self.screens.as_ref().ok_or("终端画面不可用")?;
        Ok(record.snapshot(screens.snapshot(id)?))
    }

    fn emit_operation(&mut self, app: &AppHandle, terminal_id: &str, operation: Operation) {
        let (Some(record), Some(screens)) =
            (self.records.get_mut(terminal_id), self.screens.as_ref())
        else {
            return;
        };
        let update = match &operation {
            Operation::Output { data } => screens.write(terminal_id, data),
            Operation::Resize { cols, rows } => {
                record.size = terminal_size(*cols, *rows);
                screens.resize(terminal_id, record.size)
            }
            Operation::Exit { code } => {
                record.live = None;
                record.exit = Some(TerminalExit { code: *code });
                Ok(())
            }
            Operation::Rename { title } => {
                record.title.clone_from(title);
                Ok(())
            }
        };
        if let Err(error) = update {
            record.screen_error = Some(error);
        }
        record.sequence += 1;
        let _ = app.emit_to(
            "main",
            "acp-event",
            json!({
                "kind": "terminal", "terminalId": terminal_id,
                "sequence": record.sequence, "operation": operation,
            }),
        );
    }
}

#[tauri::command]
pub async fn terminal_font() -> Result<tauri::ipc::Response, String> {
    if cfg!(target_os = "macos") {
        std::fs::read("/System/Library/Fonts/Menlo.ttc")
            .map(tauri::ipc::Response::new)
            .map_err(|error| error.to_string())
    } else if cfg!(windows) {
        // Ship the platform monospace face when available; otherwise the
        // frontend falls back to its bundled font without an error toast.
        for path in [
            r"C:\Windows\Fonts\consola.ttf",
            r"C:\Windows\Fonts\cascadiamono.ttf",
        ] {
            if let Ok(bytes) = std::fs::read(path) {
                return Ok(tauri::ipc::Response::new(bytes));
            }
        }
        Ok(tauri::ipc::Response::new(Vec::<u8>::new()))
    } else {
        Ok(tauri::ipc::Response::new(Vec::<u8>::new()))
    }
}

#[tauri::command]
pub async fn terminal_shells() -> Result<Vec<ShellProfile>, String> {
    shell::discover()
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_terminal(
    app: AppHandle,
    service: State<'_, TerminalService>,
    terminal_id: String,
    session: String,
    cwd: String,
    cols: u16,
    rows: u16,
    shell_path: Option<String>,
) -> Result<TerminalSnapshot, String> {
    if session.is_empty() || uuid::Uuid::parse_str(&terminal_id).is_err() {
        return Err("终端标识无效".into());
    }
    let mut state = service.inner.lock().map_err(|_| "终端服务锁定失败")?;
    if state.closed.contains(&terminal_id) {
        return Err("终端已关闭。".into());
    }
    if let Some(record) = state.records.get(&terminal_id) {
        return if record.session == session {
            state.snapshot(&terminal_id, record)
        } else {
            Err("终端属于另一个会话".into())
        };
    }
    if state
        .records
        .values()
        .filter(|record| record.session == session)
        .count()
        >= MAX_SESSION_TERMINALS
    {
        return Err("终端数量已达上限，请关闭不用的终端后重试。已退出的终端也计入数量。".into());
    }
    let root = if cwd.trim().is_empty() {
        std::env::var("HOME").unwrap_or_else(|_| "/".into())
    } else {
        cwd
    };
    let root = Path::new(&root);
    if !root.is_dir() {
        return Err(format!("工作区不存在：{}", root.display()));
    }
    let shell = shell::discover()?
        .into_iter()
        .find(|profile| shell_path.as_ref().is_none_or(|path| path == &profile.path))
        .ok_or("所选 Shell 在当前执行环境中不可用")?;

    let size = terminal_size(cols, rows);
    let pair = native_pty_system()
        .openpty(size)
        .map_err(|error| format!("无法创建终端：{error}"))?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("无法读取终端输出：{error}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("无法写入终端：{error}"))?;
    let screens = state.screens(&app)?;
    screens.open(&terminal_id, size)?;

    let cmd = shell.command(root, &session);
    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(child) => child,
        Err(error) => {
            let _ = screens.close(&terminal_id);
            return Err(format!("无法启动 shell：{error}"));
        }
    };
    drop(pair.slave);
    let record = TerminalRecord {
        session,
        title: shell.name,
        size,
        sequence: 0,
        screen_error: None,
        exit: None,
        live: Some(LiveTerminal {
            writer: Arc::new(Mutex::new(writer)),
            master: pair.master,
            killer: child.clone_killer(),
        }),
    };
    let snapshot = record.snapshot(String::new());
    state.records.insert(terminal_id.clone(), record);
    drop(state);

    std::thread::spawn(move || {
        let service = app.state::<TerminalService>();
        let emit_chunk = |chunk: &str| {
            if let Ok(mut state) = service.inner.lock() {
                state.emit_operation(&app, &terminal_id, Operation::Output { data: chunk.into() });
            }
        };
        let mut buffer = [0_u8; 4096 + 3];
        let mut pending = 0;
        loop {
            match reader.read(&mut buffer[pending..]) {
                Ok(0) => break,
                Ok(n) => {
                    let len = pending + n;
                    let end = complete_utf8_prefix(&buffer[..len]);
                    if end > 0 {
                        emit_chunk(&String::from_utf8_lossy(&buffer[..end]));
                    }
                    buffer.copy_within(end..len, 0);
                    pending = len - end;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        if pending > 0 {
            emit_chunk(&String::from_utf8_lossy(&buffer[..pending]));
        }
        let code = child.wait().ok().map(|status| status.exit_code());
        if let Ok(mut state) = service.inner.lock() {
            state.emit_operation(&app, &terminal_id, Operation::Exit { code });
        }
    });

    Ok(snapshot)
}

#[tauri::command]
pub async fn attach_terminal(
    service: State<'_, TerminalService>,
    terminal_id: String,
    session: String,
) -> Result<Option<TerminalSnapshot>, String> {
    let state = service.inner.lock().map_err(|_| "终端服务锁定失败")?;
    state
        .records
        .get(&terminal_id)
        .filter(|record| record.session == session)
        .map(|record| state.snapshot(&terminal_id, record))
        .transpose()
}

#[tauri::command]
pub async fn rename_terminal(
    app: AppHandle,
    service: State<'_, TerminalService>,
    terminal_id: String,
    session: String,
    title: String,
) -> Result<(), String> {
    let title = terminal_title(&title)?;
    let mut state = service.inner.lock().map_err(|_| "终端服务锁定失败")?;
    let record = state
        .records
        .get(&terminal_id)
        .filter(|record| record.session == session)
        .ok_or("终端不存在")?;
    if record.title != title {
        state.emit_operation(
            &app,
            &terminal_id,
            Operation::Rename {
                title: title.into(),
            },
        );
    }
    Ok(())
}

fn terminal_title(title: &str) -> Result<&str, String> {
    // Match the installed client's String.trim() and input maxlength.
    let trimmed =
        title.trim_matches(|ch: char| ch == '\u{feff}' || (ch.is_whitespace() && ch != '\u{0085}'));
    if trimmed.is_empty() || title.encode_utf16().count() > 120 {
        return Err("Terminal title must contain 1–120 characters".into());
    }
    Ok(trimmed)
}

fn terminal_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.clamp(1, 200),
        cols: cols.clamp(2, 500),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Keep an incomplete UTF-8 suffix for the next read. Invalid bytes can be decoded now.
fn complete_utf8_prefix(bytes: &[u8]) -> usize {
    let mut offset = 0;
    while let Err(error) = std::str::from_utf8(&bytes[offset..]) {
        offset += error.valid_up_to();
        let Some(invalid_len) = error.error_len() else {
            return offset;
        };
        offset += invalid_len;
    }
    bytes.len()
}

#[tauri::command]
pub async fn write_terminal(
    service: State<'_, TerminalService>,
    terminal_id: String,
    data: String,
) -> Result<(), String> {
    let writer = {
        let state = service.inner.lock().map_err(|_| "终端服务锁定失败")?;
        state
            .records
            .get(&terminal_id)
            .and_then(|record| record.live.as_ref())
            .map(|terminal| terminal.writer.clone())
            .ok_or("终端不存在或已退出")?
    };
    let mut writer = writer.lock().map_err(|_| "终端写入锁定失败")?;
    writer
        .write_all(data.as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|error| format!("写入终端失败：{error}"))
}

#[tauri::command]
pub async fn resize_terminal(
    app: AppHandle,
    service: State<'_, TerminalService>,
    terminal_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let mut state = service.inner.lock().map_err(|_| "终端服务锁定失败")?;
    let record = state
        .records
        .get(&terminal_id)
        .ok_or("终端不存在或已退出")?;
    let terminal = record.live.as_ref().ok_or("终端不存在或已退出")?;
    let size = terminal_size(cols, rows);
    if terminal.master.get_size().ok() != Some(size) {
        terminal
            .master
            .resize(size)
            .map_err(|error| format!("调整终端尺寸失败：{error}"))?;
    }
    if record.size != size {
        state.emit_operation(
            &app,
            &terminal_id,
            Operation::Resize {
                cols: size.cols,
                rows: size.rows,
            },
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn stop_terminal(
    service: State<'_, TerminalService>,
    terminal_id: String,
) -> Result<(), String> {
    if uuid::Uuid::parse_str(&terminal_id).is_err() {
        return Err("终端标识无效".into());
    }
    let record = {
        let mut state = service.inner.lock().map_err(|_| "终端服务锁定失败")?;
        let record = state.records.remove(&terminal_id);
        if record.is_some()
            && let Some(screens) = &state.screens
        {
            let _ = screens.close(&terminal_id);
        }
        state.closed.insert(terminal_id);
        record
    };
    if let Some(mut terminal) = record.and_then(|record| record.live) {
        let _ = terminal.killer.kill();
    }
    Ok(())
}

/// Shared handle so Exit can kill leftover shells.
pub fn shutdown(service: &TerminalService) {
    if let Ok(mut state) = service.inner.lock() {
        let records = std::mem::take(&mut state.records);
        let screens = state.screens.take();
        drop(state);
        drop(screens);
        for record in records.into_values() {
            if let Some(mut terminal) = record.live {
                let _ = terminal.killer.kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_names_use_the_browser_whitespace_and_utf16_limit() {
        assert_eq!(terminal_title("\u{feff} 开发\u{a0}").unwrap(), "开发");
        assert_eq!(terminal_title("\u{85}").unwrap(), "\u{85}");
        assert!(terminal_title(" \u{feff}\t").is_err());
        assert!(terminal_title(&"🧪".repeat(60)).is_ok());
        assert!(terminal_title(&format!(" {}", "🧪".repeat(60))).is_err());
        assert!(terminal_title(&"🧪".repeat(61)).is_err());
    }

    #[test]
    fn terminal_chunks_carry_only_an_incomplete_utf8_suffix() {
        let text = "a中🧪".as_bytes();
        for (length, expected) in [(1, 1), (2, 1), (4, 4), (5, 4), (7, 4), (8, 8)] {
            assert_eq!(complete_utf8_prefix(&text[..length]), expected);
        }
        assert_eq!(complete_utf8_prefix(b"\xffx\xe4\xb8"), 2);
        assert_eq!(complete_utf8_prefix(b"\xffx"), 2);
    }
}
