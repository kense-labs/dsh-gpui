//! ACP stdio transport and Tauri commands for DeepSeek Harness.

use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};

const EVENT_NAME: &str = "acp-event";

/// Owns the single ACP connection shared by all windows and sessions.
#[derive(Default)]
pub struct AcpService {
    connection: Mutex<Option<Arc<AcpConnection>>>,
}

struct AcpConnection {
    next_id: AtomicU64,
    writer: Mutex<ChildStdin>,
    child: Mutex<Child>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    stopping: AtomicBool,
}

impl AcpConnection {
    async fn spawn(service_path: &Path, app: AppHandle) -> Result<Arc<Self>, String> {
        let built = service_path.join("apps/cli/lib/bin.js");
        let source = service_path.join("apps/cli/src/bin.ts");
        let node = std::env::var_os("DSH_NODE").unwrap_or_else(|| "node".into());
        let mut command = Command::new(node);
        command.current_dir(service_path);
        if built.is_file() {
            command.arg(&built).args(["--profile", "acp"]);
        } else if source.is_file() {
            command
                .args(["--import", "tsx/esm"])
                .arg(&source)
                .args(["--profile", "acp"]);
        } else {
            return Err(format!(
                "未找到 dsh-acp 启动入口：{}。请先运行 pnpm install 和 pnpm run build。",
                service_path.display()
            ));
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| {
            format!(
                "无法启动 dsh-acp：{error}。请确认 Node.js 24.9+ 已安装，也可通过 DSH_NODE 指定路径。"
            )
        })?;
        let stdin = child.stdin.take().ok_or("无法打开 dsh-acp 标准输入")?;
        let stdout = child.stdout.take().ok_or("无法打开 dsh-acp 标准输出")?;
        let stderr = child.stderr.take().ok_or("无法打开 dsh-acp 标准错误")?;
        let connection = Arc::new(Self {
            next_id: AtomicU64::new(1),
            writer: Mutex::new(stdin),
            child: Mutex::new(child),
            pending: Mutex::new(HashMap::new()),
            stopping: AtomicBool::new(false),
        });

        tokio::spawn(read_stdout(connection.clone(), stdout, app.clone()));
        tokio::spawn(read_stderr(stderr));
        emit(&app, "service", json!({"status": "running"}));
        Ok(connection)
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if let Err(error) = self.write(message).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        receiver
            .await
            .map_err(|_| "dsh-acp 在请求完成前断开了连接".to_string())?
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write(json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    async fn respond(&self, id: Value, result: Value) -> Result<(), String> {
        self.write(json!({"jsonrpc": "2.0", "id": id, "result": result}))
            .await
    }

    async fn write(&self, message: Value) -> Result<(), String> {
        let mut encoded = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
        encoded.push(b'\n');
        let mut writer = self.writer.lock().await;
        writer
            .write_all(&encoded)
            .await
            .map_err(|error| format!("写入 dsh-acp 失败：{error}"))?;
        writer
            .flush()
            .await
            .map_err(|error| format!("刷新 dsh-acp 输入失败：{error}"))
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
            eprintln!("dsh-acp shutdown: {error}");
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
    connection: Arc<AcpConnection>,
    stdout: impl tokio::io::AsyncRead + Unpin,
    app: AppHandle,
) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) if line.trim().is_empty() => {}
            Ok(Some(line)) => match serde_json::from_str::<Value>(&line) {
                Ok(message) => route_message(&connection, &app, message).await,
                Err(error) => eprintln!("dsh-acp invalid JSON-RPC message: {error}"),
            },
            Ok(None) => break,
            Err(error) => {
                eprintln!("dsh-acp stdout: {error}");
                break;
            }
        }
    }
    connection.fail_pending("dsh-acp 已断开连接").await;
    if !connection.stopping.load(Ordering::Acquire) {
        emit(
            &app,
            "service",
            json!({"status": "exited", "detail": "dsh-acp 已断开连接"}),
        );
    }
}

async fn route_message(connection: &Arc<AcpConnection>, app: &AppHandle, message: Value) {
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id").cloned();
    match (method, id) {
        (Some("session/request_permission"), Some(id)) => {
            let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
            emit(
                app,
                "permission",
                json!({
                    "requestId": id,
                    "sessionId": params.get("sessionId").cloned().unwrap_or(Value::Null),
                    "toolCall": params.get("toolCall").cloned().unwrap_or_else(|| json!({})),
                    "options": params.get("options").cloned().unwrap_or_else(|| json!([])),
                }),
            );
        }
        (Some(_), Some(id)) => {
            let response = json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "client method is not supported"}
            });
            if let Err(error) = connection.write(response).await {
                eprintln!("dsh-acp method rejection: {error}");
            }
        }
        (Some("session/update"), None) => {
            let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
            emit(
                app,
                "session_update",
                json!({
                    "sessionId": params.get("sessionId").cloned().unwrap_or(Value::Null),
                    "update": params.get("update").cloned().unwrap_or(Value::Null),
                }),
            );
        }
        (Some(_), None) => {}
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else {
                eprintln!("dsh-acp response has a non-numeric id");
                return;
            };
            let sender = connection.pending.lock().await.remove(&id);
            if let Some(sender) = sender {
                let _ = sender.send(decode_response(&message));
            }
        }
        (None, None) => eprintln!("dsh-acp message has neither method nor id"),
    }
}

async fn read_stderr(stderr: impl tokio::io::AsyncRead + Unpin) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        eprintln!("dsh-acp: {line}");
    }
}

fn decode_response(message: &Value) -> Result<Value, String> {
    if let Some(error) = message.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(-1);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown JSON-RPC error");
        let detail = error.get("data").map(|data| {
            if let Some(text) = data.as_str() {
                format!("：{text}")
            } else {
                format!("：{data}")
            }
        });
        return Err(format!(
            "ACP {code}: {message}{}",
            detail.unwrap_or_default()
        ));
    }
    Ok(message.get("result").cloned().unwrap_or_else(|| json!({})))
}

fn emit(app: &AppHandle, kind: &str, payload: Value) {
    let mut event = json!({"kind": kind});
    if let (Some(target), Some(source)) = (event.as_object_mut(), payload.as_object()) {
        target.extend(source.clone());
    }
    if let Err(error) = app.emit(EVENT_NAME, event) {
        eprintln!("dsh-gpui event emission: {error}");
    }
}

/// Resolved paths and the ACP initialize result returned to the UI.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartServiceReply {
    pub agent: Value,
    pub workspace: String,
    pub service_path: String,
}

#[tauri::command]
pub async fn start_service(
    service_path: Option<String>,
    workspace: Option<String>,
    app: AppHandle,
    service: State<'_, AcpService>,
) -> Result<StartServiceReply, String> {
    let service_path = resolve_service_path(service_path.as_deref())?;
    let workspace = resolve_workspace(workspace.as_deref())?;
    let mut slot = service.connection.lock().await;
    if let Some(previous) = slot.take() {
        previous.stop().await;
    }
    emit(&app, "service", json!({"status": "starting"}));
    let connection = AcpConnection::spawn(&service_path, app.clone()).await?;
    let agent = connection
        .request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {"name": "dsh-gpui", "version": env!("CARGO_PKG_VERSION")}
            }),
        )
        .await?;
    emit(
        &app,
        "service",
        json!({"status": "running", "workspace": workspace.display().to_string()}),
    );
    *slot = Some(connection);
    Ok(StartServiceReply {
        agent,
        workspace: workspace.display().to_string(),
        service_path: service_path.display().to_string(),
    })
}

#[tauri::command]
pub async fn stop_service(service: State<'_, AcpService>) -> Result<(), String> {
    if let Some(connection) = service.connection.lock().await.take() {
        connection.stop().await;
    }
    Ok(())
}

#[tauri::command]
pub async fn new_session(cwd: String, service: State<'_, AcpService>) -> Result<Value, String> {
    connection(&service)
        .await?
        .request("session/new", json!({"cwd": cwd, "mcpServers": []}))
        .await
}

#[tauri::command]
pub async fn list_sessions(
    cwd: Option<String>,
    cursor: Option<String>,
    service: State<'_, AcpService>,
) -> Result<Value, String> {
    connection(&service)
        .await?
        .request("session/list", json!({"cwd": cwd, "cursor": cursor}))
        .await
}

#[tauri::command]
pub async fn resume_session(
    session_id: String,
    cwd: String,
    service: State<'_, AcpService>,
) -> Result<Value, String> {
    connection(&service)
        .await?
        .request(
            "session/resume",
            json!({"sessionId": session_id, "cwd": cwd, "mcpServers": []}),
        )
        .await
}

#[tauri::command]
pub async fn close_session(
    session_id: String,
    service: State<'_, AcpService>,
) -> Result<Value, String> {
    connection(&service)
        .await?
        .request("session/close", json!({"sessionId": session_id}))
        .await
}

#[tauri::command]
pub async fn prompt(
    session_id: String,
    text: String,
    service: State<'_, AcpService>,
) -> Result<Value, String> {
    connection(&service)
        .await?
        .request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]}),
        )
        .await
}

#[tauri::command]
pub async fn cancel_session(
    session_id: String,
    service: State<'_, AcpService>,
) -> Result<(), String> {
    connection(&service)
        .await?
        .notify("session/cancel", json!({"sessionId": session_id}))
        .await
}

#[tauri::command]
pub async fn set_config_option(
    session_id: String,
    config_id: String,
    value: String,
    service: State<'_, AcpService>,
) -> Result<Value, String> {
    connection(&service)
        .await?
        .request(
            "session/set_config_option",
            json!({"sessionId": session_id, "configId": config_id, "value": value}),
        )
        .await
}

#[tauri::command]
pub async fn respond_permission(
    request_id: Value,
    option_id: String,
    service: State<'_, AcpService>,
) -> Result<(), String> {
    connection(&service)
        .await?
        .respond(
            request_id,
            json!({"outcome": {"outcome": "selected", "optionId": option_id}}),
        )
        .await
}

async fn connection(service: &AcpService) -> Result<Arc<AcpConnection>, String> {
    service
        .connection
        .lock()
        .await
        .clone()
        .ok_or_else(|| "dsh-acp 尚未连接".to_string())
}

fn resolve_workspace(value: Option<&str>) -> Result<PathBuf, String> {
    let path = match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => absolute_path(Path::new(value))?,
        None => std::env::current_dir().map_err(|error| error.to_string())?,
    };
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("工作区不可访问 {}：{error}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!("工作区不是目录：{}", canonical.display()));
    }
    Ok(canonical)
}

fn resolve_service_path(value: Option<&str>) -> Result<PathBuf, String> {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        let path = absolute_path(Path::new(value))?;
        return path
            .canonicalize()
            .map_err(|error| format!("dsh-acp 路径不可访问 {}：{error}", path.display()));
    }
    if let Some(path) = std::env::var_os("DSH_ACP_PATH") {
        let path = PathBuf::from(path);
        if path.is_dir() {
            return Ok(path);
        }
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest.join("../../dsh-acp"),
        manifest.join("../dsh-acp"),
        PathBuf::from("./dsh-acp"),
        PathBuf::from("../dsh-acp"),
    ];
    candidates
        .iter()
        .find(|path| path.is_dir())
        .and_then(|path| path.canonicalize().ok())
        .ok_or_else(|| {
            "未找到 ./dsh-acp。请在设置中填写服务路径，或设置 DSH_ACP_PATH。".to_string()
        })
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path))
    }
}

impl AcpService {
    /// Best-effort synchronous cleanup during Tauri's terminal run event.
    pub fn shutdown_now(&self) {
        if let Ok(mut slot) = self.connection.try_lock()
            && let Some(connection) = slot.take()
        {
            connection.stop_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::decode_response;
    use serde_json::json;

    #[test]
    fn response_returns_the_result_without_transport_fields() {
        let response = json!({"jsonrpc": "2.0", "id": 4, "result": {"sessionId": "abc"}});
        assert_eq!(
            decode_response(&response).expect("valid response"),
            json!({"sessionId": "abc"})
        );
    }

    #[test]
    fn response_preserves_acp_error_detail() {
        let response = json!({
            "jsonrpc": "2.0",
            "id": 4,
            "error": {"code": -32602, "message": "Invalid params", "data": "cwd must be absolute"}
        });
        let error = decode_response(&response).expect_err("invalid response");
        assert!(error.contains("-32602"));
        assert!(error.contains("cwd must be absolute"));
    }
}
