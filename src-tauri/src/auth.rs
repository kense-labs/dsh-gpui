//! Host-only Harness API Key service and write-only credential commands.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};

use crate::acp::resolve_service_path;

/// Owns the credential helper independently of the ACP conversation process.
#[derive(Default)]
pub struct AuthService {
    connection: Mutex<Option<Arc<AuthConnection>>>,
}

struct AuthConnection {
    next_id: AtomicU64,
    writer: Mutex<ChildStdin>,
    child: Mutex<Child>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    stopping: AtomicBool,
}

impl AuthConnection {
    async fn spawn(service_path: PathBuf, app: AppHandle) -> Result<Arc<Self>, String> {
        let dsh_home = resolve_dsh_home()?;
        std::fs::create_dir_all(&dsh_home)
            .map_err(|error| format!("无法创建 Harness 主目录：{error}"))?;
        let node = crate::acp::resolve_node();
        let mut child = Command::new(node)
            .args(["--input-type=module", "--eval"])
            .arg(include_str!("../../services/credential-service.mjs"))
            .args(["--", "dsh-gpui-credentials"])
            .arg(&service_path)
            .arg(&dsh_home)
            .env("DSH_HOME", &dsh_home)
            .current_dir(&service_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("无法启动 Harness 凭据服务：{error}"))?;
        let stdin = child.stdin.take().ok_or("无法打开凭据服务标准输入")?;
        let stdout = child.stdout.take().ok_or("无法打开凭据服务标准输出")?;
        let stderr = child.stderr.take().ok_or("无法打开凭据服务标准错误")?;
        let connection = Arc::new(Self {
            next_id: AtomicU64::new(1),
            writer: Mutex::new(stdin),
            child: Mutex::new(child),
            pending: Mutex::new(HashMap::new()),
            stopping: AtomicBool::new(false),
        });
        tokio::spawn(read_stdout(connection.clone(), stdout, app));
        tokio::spawn(read_stderr(stderr));
        Ok(connection)
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        if let Err(error) = self
            .write(json!({"id": id, "method": method, "params": params}))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        receiver
            .await
            .map_err(|_| "Harness 凭据服务在请求完成前断开".to_string())?
    }

    async fn write(&self, message: Value) -> Result<(), String> {
        let mut encoded = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
        encoded.push(b'\n');
        let mut writer = self.writer.lock().await;
        writer
            .write_all(&encoded)
            .await
            .map_err(|error| format!("写入凭据服务失败：{error}"))?;
        writer
            .flush()
            .await
            .map_err(|error| format!("刷新凭据服务输入失败：{error}"))
    }

    async fn fail_pending(&self, reason: &str) {
        let pending = std::mem::take(&mut *self.pending.lock().await);
        for (_, sender) in pending {
            let _ = sender.send(Err(reason.to_string()));
        }
    }

    async fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        let mut child = self.child.lock().await;
        if let Err(error) = child.kill().await
            && child.id().is_some()
        {
            eprintln!("dsh-credentials shutdown: {error}");
        }
    }

    fn stop_now(&self) {
        self.stopping.store(true, Ordering::Release);
        if let Ok(mut child) = self.child.try_lock() {
            let _ = child.start_kill();
        }
    }
}

async fn read_stdout(
    connection: Arc<AuthConnection>,
    stdout: impl tokio::io::AsyncRead + Unpin,
    app: AppHandle,
) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) if line.trim().is_empty() => {}
            Ok(Some(line)) => match serde_json::from_str::<Value>(&line) {
                Ok(message) => route_message(&connection, &app, message).await,
                Err(error) => eprintln!("dsh-credentials invalid NDJSON message: {error}"),
            },
            Ok(None) => break,
            Err(error) => {
                eprintln!("dsh-credentials stdout: {error}");
                break;
            }
        }
    }
    connection.fail_pending("Harness 凭据服务已断开").await;
    if !connection.stopping.load(Ordering::Acquire) {
        emit(
            &app,
            "auth",
            json!({"status": "exited", "detail": "Harness 凭据服务已断开"}),
        );
    }
}

async fn route_message(connection: &Arc<AuthConnection>, app: &AppHandle, message: Value) {
    if message.get("event").and_then(Value::as_str) == Some("fatal") {
        emit(
            app,
            "auth",
            json!({"status": "error", "detail": message.get("error")}),
        );
        return;
    }
    let Some(id) = message.get("id").and_then(Value::as_u64) else {
        return;
    };
    let sender = connection.pending.lock().await.remove(&id);
    if let Some(sender) = sender {
        let result = if let Some(error) = message.get("error") {
            Err(error.as_str().unwrap_or("Harness 凭据操作失败").to_string())
        } else {
            Ok(message.get("result").cloned().unwrap_or_else(|| json!({})))
        };
        let _ = sender.send(result);
    }
}

async fn read_stderr(stderr: impl tokio::io::AsyncRead + Unpin) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        eprintln!("dsh-credentials: {line}");
    }
}

fn emit(app: &AppHandle, kind: &str, payload: Value) {
    let mut event = json!({"kind": kind});
    if let (Some(target), Some(source)) = (event.as_object_mut(), payload.as_object()) {
        target.extend(source.clone());
    }
    if let Err(error) = app.emit("acp-event", event) {
        eprintln!("dsh-gpui credential event emission: {error}");
    }
}

#[tauri::command]
pub async fn start_auth(
    service_path: Option<String>,
    app: AppHandle,
    service: State<'_, AuthService>,
) -> Result<Value, String> {
    let service_path = resolve_service_path(service_path.as_deref())?;
    let mut slot = service.connection.lock().await;
    if let Some(previous) = slot.take() {
        previous.stop().await;
    }
    let connection = AuthConnection::spawn(service_path, app).await?;
    let status = match connection.request("status", json!({})).await {
        Ok(status) => status,
        Err(error) => {
            connection.stop().await;
            return Err(error);
        }
    };
    *slot = Some(connection);
    Ok(status)
}

#[tauri::command]
pub async fn auth_status(service: State<'_, AuthService>) -> Result<Value, String> {
    connection(&service)
        .await?
        .request("status", json!({}))
        .await
}

#[tauri::command]
pub async fn save_api_key(value: String, service: State<'_, AuthService>) -> Result<Value, String> {
    connection(&service)
        .await?
        .request("saveApiKey", json!({"value": value}))
        .await
}

#[tauri::command]
pub async fn remove_api_key(service: State<'_, AuthService>) -> Result<Value, String> {
    connection(&service)
        .await?
        .request("removeApiKey", json!({}))
        .await
}

async fn connection(service: &AuthService) -> Result<Arc<AuthConnection>, String> {
    service
        .connection
        .lock()
        .await
        .clone()
        .ok_or_else(|| "Harness 凭据服务尚未连接".to_string())
}

fn resolve_dsh_home() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("DSH_HOME") {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or("无法确定用户主目录，请设置 DSH_HOME")?;
    Ok(PathBuf::from(home).join(".dsh"))
}

impl AuthService {
    /// Best-effort synchronous cleanup during Tauri's terminal run event.
    pub fn shutdown_now(&self) {
        if let Ok(mut slot) = self.connection.try_lock()
            && let Some(connection) = slot.take()
        {
            connection.stop_now();
        }
    }
}
