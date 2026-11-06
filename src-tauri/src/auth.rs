//! Host-only Harness account service and write-only credential commands.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::OpenerExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};
use url::Url;

use crate::acp::resolve_service_path;

/// Owns the account helper independently of the ACP conversation process.
#[derive(Default)]
pub struct AuthService {
    connection: Mutex<Option<Arc<AuthConnection>>>,
}

struct AuthConnection {
    next_id: AtomicU64,
    writer: Mutex<ChildStdin>,
    child: Mutex<Child>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    account: Mutex<Value>,
    opened_attempt: Mutex<Option<String>>,
    stopping: AtomicBool,
}

impl AuthConnection {
    async fn spawn(service_path: PathBuf, app: AppHandle) -> Result<Arc<Self>, String> {
        let dsh_home = resolve_dsh_home()?;
        std::fs::create_dir_all(&dsh_home)
            .map_err(|error| format!("无法创建 Harness 主目录：{error}"))?;
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../services/account-service.mjs")
            .canonicalize()
            .map_err(|error| format!("账户服务入口不可访问：{error}"))?;
        let node = std::env::var_os("DSH_NODE").unwrap_or_else(|| "node".into());
        let mut child = Command::new(node)
            .arg(script)
            .arg(&service_path)
            .arg(&dsh_home)
            .env("DSH_HOME", &dsh_home)
            .current_dir(&service_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("无法启动 Harness 账户服务：{error}"))?;
        let stdin = child.stdin.take().ok_or("无法打开账户服务标准输入")?;
        let stdout = child.stdout.take().ok_or("无法打开账户服务标准输出")?;
        let stderr = child.stderr.take().ok_or("无法打开账户服务标准错误")?;
        let connection = Arc::new(Self {
            next_id: AtomicU64::new(1),
            writer: Mutex::new(stdin),
            child: Mutex::new(child),
            pending: Mutex::new(HashMap::new()),
            account: Mutex::new(Value::Null),
            opened_attempt: Mutex::new(None),
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
            .map_err(|_| "Harness 账户服务在请求完成前断开".to_string())?
    }

    async fn write(&self, message: Value) -> Result<(), String> {
        let mut encoded = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
        encoded.push(b'\n');
        let mut writer = self.writer.lock().await;
        writer
            .write_all(&encoded)
            .await
            .map_err(|error| format!("写入账户服务失败：{error}"))?;
        writer
            .flush()
            .await
            .map_err(|error| format!("刷新账户服务输入失败：{error}"))
    }

    async fn publish_account(&self, app: &AppHandle, state: Value) {
        let mut current = self.account.lock().await;
        if is_stale_account(&current, &state) {
            return;
        }
        *current = state.clone();
        drop(current);
        emit(app, "account", json!({"state": state.clone()}));
        let waiting =
            state.pointer("/attempt/phase").and_then(Value::as_str) == Some("waiting-browser");
        let attempt_id = state
            .pointer("/attempt/id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if waiting && authorize_url(&state).is_some() {
            let mut opened = self.opened_attempt.lock().await;
            if opened.as_deref() != Some(attempt_id.as_str()) {
                *opened = Some(attempt_id);
                if let Err(error) = open_sign_in_url(app, &state) {
                    emit(app, "auth", json!({"status": "error", "detail": error}));
                }
            }
        }
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
            eprintln!("dsh-account shutdown: {error}");
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
                Err(error) => eprintln!("dsh-account invalid NDJSON message: {error}"),
            },
            Ok(None) => break,
            Err(error) => {
                eprintln!("dsh-account stdout: {error}");
                break;
            }
        }
    }
    connection.fail_pending("Harness 账户服务已断开").await;
    if !connection.stopping.load(Ordering::Acquire) {
        emit(
            &app,
            "auth",
            json!({"status": "exited", "detail": "Harness 账户服务已断开"}),
        );
    }
}

async fn route_message(connection: &Arc<AuthConnection>, app: &AppHandle, message: Value) {
    if message.get("event").and_then(Value::as_str) == Some("account") {
        connection
            .publish_account(app, message.get("state").cloned().unwrap_or(Value::Null))
            .await;
        return;
    }
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
            Err(error.as_str().unwrap_or("Harness 账户操作失败").to_string())
        } else {
            Ok(message.get("result").cloned().unwrap_or_else(|| json!({})))
        };
        let _ = sender.send(result);
    }
}

async fn read_stderr(stderr: impl tokio::io::AsyncRead + Unpin) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        eprintln!("dsh-account: {line}");
    }
}

fn emit(app: &AppHandle, kind: &str, payload: Value) {
    let mut event = json!({"kind": kind});
    if let (Some(target), Some(source)) = (event.as_object_mut(), payload.as_object()) {
        target.extend(source.clone());
    }
    if let Err(error) = app.emit("acp-event", event) {
        eprintln!("dsh-gpui account event emission: {error}");
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
    let connection = AuthConnection::spawn(service_path, app.clone()).await?;
    let status = connection.request("status", json!({})).await?;
    if let Some(account) = status.get("account").cloned() {
        connection.publish_account(&app, account).await;
    }
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
pub async fn auth_profile(service: State<'_, AuthService>) -> Result<Value, String> {
    connection(&service)
        .await?
        .request("profile", json!({}))
        .await
}

#[tauri::command]
pub async fn auth_avatar(url: String, service: State<'_, AuthService>) -> Result<Value, String> {
    connection(&service)
        .await?
        .request("avatar", json!({"url": url}))
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

#[tauri::command]
pub async fn start_sign_in(
    app: AppHandle,
    service: State<'_, AuthService>,
) -> Result<Value, String> {
    let connection = connection(&service).await?;
    let state = connection.request("startSignIn", json!({})).await?;
    connection.publish_account(&app, state.clone()).await;
    Ok(state)
}

#[tauri::command]
pub async fn cancel_sign_in(
    attempt_id: String,
    app: AppHandle,
    service: State<'_, AuthService>,
) -> Result<Value, String> {
    let connection = connection(&service).await?;
    let state = connection
        .request("cancelSignIn", json!({"attemptId": attempt_id}))
        .await?;
    connection.publish_account(&app, state.clone()).await;
    Ok(state)
}

#[tauri::command]
pub async fn sign_out(app: AppHandle, service: State<'_, AuthService>) -> Result<Value, String> {
    let connection = connection(&service).await?;
    let state = connection.request("signOut", json!({})).await?;
    connection.publish_account(&app, state.clone()).await;
    Ok(state)
}

#[tauri::command]
pub async fn open_sign_in(app: AppHandle, service: State<'_, AuthService>) -> Result<(), String> {
    let connection = connection(&service).await?;
    let state = connection.account.lock().await.clone();
    open_sign_in_url(&app, &state)
}

#[tauri::command]
pub async fn open_account_page(
    page: String,
    app: AppHandle,
    service: State<'_, AuthService>,
) -> Result<(), String> {
    let connection = connection(&service).await?;
    let state = connection.account.lock().await.clone();
    let url = account_page_url(&state, &page)?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|error| format!("无法打开账户页面：{error}"))
}

fn account_page_url<'a>(state: &'a Value, page: &str) -> Result<&'a str, String> {
    let key = match page {
        "usage" => "usageUrl",
        "topUp" => "topUpUrl",
        _ => return Err("未知账户页面".to_string()),
    };
    let value = state
        .get("links")
        .and_then(|links| links.get(key))
        .and_then(Value::as_str)
        .ok_or_else(|| "账户页面链接尚未就绪".to_string())?;
    let url = Url::parse(value).map_err(|_| "账户页面链接无效".to_string())?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err("账户页面链接不安全".to_string());
    }
    Ok(value)
}

fn open_sign_in_url(app: &AppHandle, state: &Value) -> Result<(), String> {
    let Some(url) = authorize_url(state) else {
        return Ok(());
    };
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|error| format!("无法打开浏览器授权页：{error}"))
}

fn authorize_url(state: &Value) -> Option<&str> {
    let value = state.pointer("/attempt/authorizeUrl")?.as_str()?;
    let url = Url::parse(value).ok()?;
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.scheme() == "https" || (url.scheme() == "http" && loopback) {
        Some(value)
    } else {
        None
    }
}

fn is_stale_account(current: &Value, incoming: &Value) -> bool {
    let rank = |attempt: &Value| match attempt.get("phase").and_then(Value::as_str) {
        Some("initializing") => 0,
        Some("waiting-browser") => 1,
        Some("exchanging") => 2,
        Some("committing") => 3,
        Some("cancelled" | "expired" | "failed") => 4,
        Some("succeeded") => 5,
        _ => -1,
    };
    let current_attempt = current.get("attempt");
    let incoming_attempt = incoming.get("attempt");
    let same_attempt = current_attempt
        .and_then(|attempt| attempt.get("id"))
        .and_then(Value::as_str)
        .is_some_and(|id| {
            incoming_attempt
                .and_then(|attempt| attempt.get("id"))
                .and_then(Value::as_str)
                == Some(id)
        });
    same_attempt
        && incoming_attempt.is_some_and(|attempt| rank(attempt) < current_attempt.map_or(-1, rank))
}

async fn connection(service: &AuthService) -> Result<Arc<AuthConnection>, String> {
    service
        .connection
        .lock()
        .await
        .clone()
        .ok_or_else(|| "Harness 账户服务尚未连接".to_string())
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

#[cfg(test)]
mod tests {
    use super::{account_page_url, authorize_url, is_stale_account};
    use serde_json::json;

    #[test]
    fn browser_authorization_accepts_https() {
        let state =
            json!({"attempt": {"authorizeUrl": "https://platform.deepseek.com/oauth?state=abc"}});
        assert_eq!(
            authorize_url(&state),
            Some("https://platform.deepseek.com/oauth?state=abc")
        );
    }

    #[test]
    fn browser_authorization_rejects_non_loopback_http_and_userinfo() {
        for value in [
            "http://example.com/oauth",
            "https://user:secret@example.com/oauth",
            "file:///tmp/oauth",
        ] {
            let state = json!({"attempt": {"authorizeUrl": value}});
            assert_eq!(authorize_url(&state), None);
        }
    }

    #[test]
    fn browser_authorization_allows_loopback_development_server() {
        let state = json!({"attempt": {"authorizeUrl": "http://127.0.0.1:3000/oauth"}});
        assert_eq!(authorize_url(&state), Some("http://127.0.0.1:3000/oauth"));
    }

    #[test]
    fn account_phases_never_reverse_within_an_attempt() {
        let initializing = json!({"attempt": {"id": "attempt-1", "phase": "initializing"}});
        let waiting = json!({"attempt": {"id": "attempt-1", "phase": "waiting-browser"}});
        let succeeded = json!({"attempt": {"id": "attempt-1", "phase": "succeeded"}});
        assert!(!is_stale_account(&initializing, &waiting));
        assert!(!is_stale_account(&waiting, &succeeded));
        assert!(is_stale_account(&waiting, &initializing));
        assert!(is_stale_account(&succeeded, &waiting));
    }

    #[test]
    fn a_new_attempt_starts_from_initializing() {
        let previous = json!({"attempt": {"id": "attempt-1", "phase": "failed"}});
        let next = json!({"attempt": {"id": "attempt-2", "phase": "initializing"}});
        assert!(!is_stale_account(&previous, &next));
    }

    #[test]
    fn account_pages_use_only_host_provided_https_links() {
        let state = json!({"links": {
            "usageUrl": "https://platform.deepseek.com/usage",
            "topUpUrl": "https://platform.deepseek.com/top_up"
        }});
        assert_eq!(
            account_page_url(&state, "usage").ok(),
            Some("https://platform.deepseek.com/usage")
        );
        assert_eq!(
            account_page_url(&state, "topUp").ok(),
            Some("https://platform.deepseek.com/top_up")
        );
        assert!(account_page_url(&state, "unknown").is_err());
    }

    #[test]
    fn account_pages_reject_insecure_or_missing_links() {
        let state = json!({"links": {"usageUrl": "http://platform.deepseek.com/usage"}});
        assert!(account_page_url(&state, "usage").is_err());
        assert!(account_page_url(&state, "topUp").is_err());
    }
}
