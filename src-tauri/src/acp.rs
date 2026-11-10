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
const SESSION_BRIDGE: &str = include_str!("../../services/acp-session-bridge.mjs");
const ASK_USER_TOOL: &str = include_str!("../../services/vendor/dsh-tool-ask-user/index.mjs");

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
    session_bridge: bool,
}

impl AcpConnection {
    async fn spawn(service_path: &Path, app: AppHandle) -> Result<Arc<Self>, String> {
        let built = service_path.join("apps/cli/lib/bin.js");
        let source = service_path.join("apps/cli/src/bin.ts");
        let node = resolve_node();
        let mut command = Command::new(node);
        command.current_dir(service_path);
        let (entry, profile) = if built.is_file() {
            (built, service_path.join("apps/cli/lib/profile-boot.js"))
        } else if source.is_file() {
            command.args(["--import", "tsx/esm"]);
            (source, service_path.join("apps/cli/src/profile-boot.ts"))
        } else {
            return Err(format!(
                "未找到 dsh-acp 启动入口：{}。请先运行 pnpm install 和 pnpm run build。",
                service_path.display()
            ));
        };
        let session_bridge = profile.is_file();
        if session_bridge {
            let ask_tool =
                serde_json::to_string(ASK_USER_TOOL).map_err(|error| error.to_string())?;
            let bridge = format!("const bundledAskUser = {ask_tool};\n{SESSION_BRIDGE}");
            command
                .args(["--input-type=module", "--eval"])
                .arg(bridge)
                .args(["--", "dsh-gpui-acp"])
                .arg(service_path)
                .arg(profile);
        } else {
            command.arg(entry).args(["--profile", "acp"]);
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
            session_bridge,
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
            let mut params = message.get("params").cloned().unwrap_or_else(|| json!({}));
            if let Some(options) = params.pointer_mut("/update/configOptions") {
                filter_account_models(options);
            }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_providers: Option<Value>,
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
    let model_providers = if connection.session_bridge {
        connection
            .request("_dsh/gpui/session/questions", json!({}))
            .await?;
        connection
            .request("_dsh/gpui/providers/list", json!({}))
            .await
            .ok()
    } else {
        None
    };
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
        model_providers,
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
    let connection = connection(&service).await?;
    let mut reply = connection
        .request("session/new", json!({"cwd": cwd, "mcpServers": []}))
        .await?;
    if let Some(id) = reply["sessionId"].as_str().map(str::to_owned) {
        prepare_session_models(&connection, &id, &mut reply).await;
    }
    Ok(reply)
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
    let connection = connection(&service).await?;
    resume_with_key_models(&connection, &session_id, &cwd).await
}

async fn resume_with_key_models(
    connection: &AcpConnection,
    session_id: &str,
    cwd: &str,
) -> Result<Value, String> {
    let mut reply = connection
        .request(
            "session/resume",
            json!({"sessionId": session_id, "cwd": cwd, "mcpServers": []}),
        )
        .await?;
    prepare_session_models(connection, session_id, &mut reply).await;
    Ok(reply)
}

fn model_address(value: &str) -> Option<[String; 2]> {
    serde_json::from_str(value).ok()
}

fn fallback_key_model(config_options: &Value) -> Option<String> {
    let model = config_options
        .as_array()?
        .iter()
        .find(|option| option["id"] == "model")?;
    let [provider, previous_model] = model_address(model["currentValue"].as_str()?)?;
    if provider != "deepseek-account" {
        return None;
    }
    let group = model["options"]
        .as_array()?
        .iter()
        .find(|group| group["group"] == "deepseek-official")?;
    let choices = group["options"].as_array()?;
    choices
        .iter()
        .find(|choice| {
            choice["value"]
                .as_str()
                .and_then(model_address)
                .is_some_and(|[_, model]| model == previous_model)
        })
        .or_else(|| choices.first())?["value"]
        .as_str()
        .map(str::to_owned)
}

fn filter_account_models(config_options: &mut Value) {
    if let Some(options) = config_options.as_array_mut()
        && let Some(model) = options.iter_mut().find(|option| option["id"] == "model")
        && let Some(choices) = model["options"].as_array_mut()
    {
        choices.retain(|item| {
            item["group"] != "deepseek-account"
                && item["value"]
                    .as_str()
                    .and_then(model_address)
                    .is_none_or(|[provider, _]| provider != "deepseek-account")
        });
    }
}

async fn prepare_session_models(connection: &AcpConnection, session_id: &str, reply: &mut Value) {
    if let Some(value) = fallback_key_model(&reply["configOptions"])
        && let Ok(mut changed) = connection
            .request(
                "session/set_config_option",
                json!({"sessionId":session_id,"configId":"model","value":value}),
            )
            .await
    {
        reply["configOptions"] = changed["configOptions"].take();
    }
    if let Some(options) = reply.get_mut("configOptions") {
        filter_account_models(options);
    }
}

#[tauri::command]
pub(crate) async fn rename_session(
    webview: tauri::Webview,
    session_id: String,
    title: String,
    service: State<'_, AcpService>,
) -> Result<Value, String> {
    if webview.label() != "main" || session_id.is_empty() {
        return Err("session/not-found".into());
    }
    let connection = connection(&service).await?;
    if !connection.session_bridge {
        return Err("当前服务不支持重命名会话。".into());
    }
    connection
        .request(
            "_dsh/gpui/session/rename",
            json!({"sessionId": session_id, "title": title}),
        )
        .await
}

#[tauri::command]
pub(crate) async fn fork_session(
    webview: tauri::Webview,
    session_id: String,
    at_seq: Option<u64>,
    open: bool,
    allow_migration: bool,
    service: State<'_, AcpService>,
) -> Result<Value, String> {
    if webview.label() != "main" || session_id.is_empty() {
        return Err("session/not-found".into());
    }
    let connection = connection(&service).await?;
    if !connection.session_bridge {
        return Err("session/fork-unavailable".into());
    }
    let mut params = json!({"sessionId": session_id, "allowMigration": allow_migration});
    if let Some(seq) = at_seq {
        params["atSeq"] = seq.into();
    }
    let mut child = connection.request("_dsh/gpui/session/fork", params).await?;
    if open {
        let id = child["sessionId"]
            .as_str()
            .ok_or("分支会话缺少 Session ID")?;
        let cwd = child["cwd"].as_str().ok_or("分支会话缺少工作目录")?;
        let resumed = resume_with_key_models(&connection, id, cwd).await?;
        child["configOptions"] = resumed["configOptions"].clone();
    }
    Ok(child)
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
    text: Option<String>,
    prompt: Option<Value>,
    service: State<'_, AcpService>,
) -> Result<Value, String> {
    let blocks = match prompt {
        Some(Value::Array(items)) if !items.is_empty() => Value::Array(items),
        _ => {
            let text = text.unwrap_or_default();
            json!([{"type": "text", "text": text}])
        }
    };
    connection(&service)
        .await?
        .request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": blocks}),
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
    if config_id == "model"
        && model_address(&value).is_some_and(|[provider, _]| provider == "deepseek-account")
    {
        return Err("请选择使用 API Key 的模型。".into());
    }
    let mut reply = connection(&service)
        .await?
        .request(
            "session/set_config_option",
            json!({"sessionId": session_id, "configId": config_id, "value": value}),
        )
        .await?;
    if let Some(options) = reply.get_mut("configOptions") {
        filter_account_models(options);
    }
    Ok(reply)
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

#[tauri::command]
pub(crate) async fn plan_mode(
    webview: tauri::Webview,
    session_id: String,
    active: Option<bool>,
    service: State<'_, AcpService>,
) -> Result<Value, String> {
    if webview.label() != "main" || session_id.is_empty() {
        return Err("plan/unavailable".into());
    }
    let connection = connection(&service).await?;
    if !connection.session_bridge {
        return Err("当前服务不支持计划模式。".into());
    }
    connection
        .request(
            "_dsh/gpui/session/plan-mode",
            json!({"sessionId": session_id, "active": active}),
        )
        .await
}

#[tauri::command]
pub(crate) async fn permission_mode(
    webview: tauri::Webview,
    session_id: String,
    preset: Option<String>,
    acknowledged: Option<bool>,
    service: State<'_, AcpService>,
) -> Result<Value, String> {
    if webview.label() != "main" || session_id.is_empty() {
        return Err("permission/unavailable".into());
    }
    let connection = connection(&service).await?;
    if !connection.session_bridge {
        return Err("当前服务不支持切换权限。".into());
    }
    connection
        .request(
            "_dsh/gpui/session/permission-mode",
            json!({"sessionId": session_id, "preset": preset, "acknowledged": acknowledged}),
        )
        .await
}

#[derive(serde::Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PlanDecision {
    Approve,
    Discuss,
}

#[tauri::command]
pub(crate) async fn respond_plan_review(
    webview: tauri::Webview,
    session_id: String,
    request_id: String,
    decision: PlanDecision,
    service: State<'_, AcpService>,
) -> Result<(), String> {
    if webview.label() != "main" || session_id.is_empty() || request_id.is_empty() {
        return Err("plan/review-closed".into());
    }
    let connection = connection(&service).await?;
    if !connection.session_bridge {
        return Err("当前服务不支持计划审阅。".into());
    }
    connection
        .request(
            "_dsh/gpui/session/plan-review",
            json!({"sessionId": session_id, "requestId": request_id, "decision": decision}),
        )
        .await?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn question_state(
    webview: tauri::Webview,
    session_id: String,
    service: State<'_, AcpService>,
) -> Result<Value, String> {
    if webview.label() != "main" || session_id.is_empty() {
        return Err("question/unavailable".into());
    }
    let connection = connection(&service).await?;
    if !connection.session_bridge {
        return Ok(json!([]));
    }
    connection
        .request(
            "_dsh/gpui/session/questions",
            json!({"sessionId": session_id}),
        )
        .await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ModelProviderAction {
    List,
    Save,
    Remove,
    Discover,
}

#[tauri::command]
pub(crate) async fn model_providers(
    webview: tauri::Webview,
    action: ModelProviderAction,
    draft: Option<Value>,
    service: State<'_, AcpService>,
) -> Result<Value, String> {
    if webview.label() != "main" {
        return Err("仅主窗口可管理模型服务。".into());
    }
    let connection = connection(&service).await?;
    if !connection.session_bridge {
        return Err("当前 dsh-acp 不支持编辑模型服务。".into());
    }
    let method = match action {
        ModelProviderAction::List => "_dsh/gpui/providers/list",
        ModelProviderAction::Save => "_dsh/gpui/providers/save",
        ModelProviderAction::Remove => "_dsh/gpui/providers/remove",
        ModelProviderAction::Discover => "_dsh/gpui/providers/discover",
    };
    connection
        .request(method, draft.unwrap_or_else(|| json!({})))
        .await
}

#[derive(serde::Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum QuestionDecision {
    Answer,
    Cancel,
    Timeout,
}

#[derive(serde::Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RuntimeSettingsPage {
    Shell,
    AgentLoop,
}

#[tauri::command]
pub(crate) async fn runtime_settings(
    webview: tauri::Webview,
    page: RuntimeSettingsPage,
    change: Option<Value>,
    service: State<'_, AcpService>,
) -> Result<Value, String> {
    if webview.label() != "main" {
        return Err("仅主窗口可编辑运行设置。".into());
    }
    let connection = connection(&service).await?;
    if !connection.session_bridge {
        return Ok(Value::Null);
    }
    connection
        .request(
            "_dsh/gpui/runtime-settings",
            json!({"page":page,"change":change}),
        )
        .await
}

#[tauri::command]
pub(crate) async fn respond_question(
    webview: tauri::Webview,
    session_id: String,
    request_id: String,
    decision: QuestionDecision,
    answers: Option<Vec<Value>>,
    service: State<'_, AcpService>,
) -> Result<(), String> {
    if webview.label() != "main" || session_id.is_empty() || request_id.is_empty() {
        return Err("question/unavailable".into());
    }
    let connection = connection(&service).await?;
    if !connection.session_bridge {
        return Err("当前服务不支持问答。".into());
    }
    connection
        .request(
            "_dsh/gpui/session/question-decision",
            json!({"sessionId": session_id, "requestId": request_id, "decision": decision, "answers": answers}),
        )
        .await?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn record_session_feedback(
    webview: tauri::Webview,
    session_id: String,
    text: Option<String>,
    category: Option<FeedbackCategory>,
    service: State<'_, AcpService>,
) -> Result<(), String> {
    if webview.label() != "main" || session_id.is_empty() {
        return Err("session-not-found".into());
    }
    let connection = connection(&service).await?;
    if !connection.session_bridge {
        return Err("feedback-unavailable".into());
    }
    let mut params = json!({"sessionId": session_id});
    if let Some(text) = text {
        params["text"] = text.into();
    }
    if let Some(category) = category {
        params["category"] = json!(category);
    }
    let reply = connection
        .request("_dsh/gpui/session/feedback", params)
        .await?;
    if reply["ok"] == true && reply["value"]["recorded"] == true {
        Ok(())
    } else {
        Err(reply["error"]["code"]
            .as_str()
            .unwrap_or("feedback-failed")
            .to_owned())
    }
}

#[derive(Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FeedbackCategory {
    TaskResult,
    InstructionFollowing,
    ProductInteraction,
    ServiceStability,
    ResourceCost,
    SecurityPrivacyPermission,
    Other,
}

#[derive(Serialize, serde::Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum MessageFeedbackRequest {
    List,
    Put {
        message_id: String,
        rating: MessageRating,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        category: Option<FeedbackCategory>,
        if_version: Option<String>,
    },
    Delete {
        message_id: String,
        if_version: String,
    },
}

#[derive(Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MessageRating {
    Positive,
    Negative,
}

#[tauri::command]
pub(crate) async fn message_feedback(
    webview: tauri::Webview,
    session_id: String,
    request: MessageFeedbackRequest,
    service: State<'_, AcpService>,
) -> Result<Value, String> {
    if webview.label() != "main" || session_id.is_empty() {
        return Err("session-not-found".into());
    }
    let connection = connection(&service).await?;
    if !connection.session_bridge {
        return Err("feedback-unavailable".into());
    }
    let method = match &request {
        MessageFeedbackRequest::List => "_dsh/gpui/message-feedback/list",
        MessageFeedbackRequest::Put { .. } => "_dsh/gpui/message-feedback/put",
        MessageFeedbackRequest::Delete { .. } => "_dsh/gpui/message-feedback/delete",
    };
    let mut params = serde_json::to_value(request).map_err(|error| error.to_string())?;
    if let Value::Object(params) = &mut params {
        params.remove("operation");
        params.insert("sessionId".into(), session_id.into());
    }
    connection.request(method, params).await
}

/// Capture the current ACP process for an export. A later service restart
/// cannot silently redirect the durability request to a different process.
pub(crate) struct SessionFlush(Option<Arc<AcpConnection>>);

impl SessionFlush {
    pub(crate) async fn new(service: &AcpService) -> Self {
        Self(
            service
                .connection
                .lock()
                .await
                .clone()
                .filter(|connection| connection.session_bridge),
        )
    }

    pub(crate) async fn flush(&self, id: &str) -> Result<(), String> {
        if let Some(connection) = &self.0 {
            connection
                .request("_dsh/gpui/session/flush", json!({"sessionId":id}))
                .await?;
        }
        Ok(())
    }
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

/// Resolve a Node.js binary for spawning dsh-acp and the credential helper.
/// GUI launches often have a minimal PATH, so probe known install locations.
pub(crate) fn resolve_node() -> std::ffi::OsString {
    if let Some(value) = std::env::var_os("DSH_NODE") {
        let path = PathBuf::from(&value);
        if path.is_file() || which_exists(&path) {
            return value;
        }
    }
    if which_exists(Path::new("node")) {
        return "node".into();
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    // Prefer a full Node install (fnm/homebrew) over the Harness runtime Node —
    // the latter can miss native addons built for a different ABI.
    let mut candidates = Vec::new();
    if let Ok(entries) = std::fs::read_dir(home.join(".local/share/fnm/node-versions")) {
        let mut versions: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
        versions.sort();
        versions.reverse();
        for version in versions {
            candidates.push(version.join("installation/bin/node"));
        }
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/node"),
        PathBuf::from("/usr/local/bin/node"),
        home.join(".dsh/dsh-runtimes/dsh-primary-runtime/dependencies/node/bin/node"),
    ]);
    for candidate in candidates {
        if candidate.is_file() {
            return candidate.into_os_string();
        }
    }
    // Fall back to bare name so the spawn error still mentions "node".
    "node".into()
}

fn which_exists(path: &Path) -> bool {
    if path.components().count() == 1 {
        let Some(path_env) = std::env::var_os("PATH") else {
            return false;
        };
        for dir in std::env::split_paths(&path_env) {
            let candidate = dir.join(path);
            if candidate.is_file() {
                return true;
            }
        }
        false
    } else {
        path.is_file()
    }
}

pub(crate) fn resolve_service_path(value: Option<&str>) -> Result<PathBuf, String> {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        let path = absolute_path(Path::new(value))?;
        return path
            .canonicalize()
            .map_err(|error| format!("dsh-acp 路径不可访问 {}：{error}", path.display()));
    }
    if let Some(path) = std::env::var_os("DSH_ACP_PATH") {
        let path = PathBuf::from(path);
        if path.is_dir() {
            return path
                .canonicalize()
                .map_err(|error| format!("dsh-acp 路径不可访问 {}：{error}", path.display()));
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
    use super::{
        MessageFeedbackRequest, decode_response, fallback_key_model, filter_account_models,
    };
    use serde_json::json;

    #[test]
    fn existing_account_session_prefers_the_same_model_with_an_api_key() {
        let mut options = json!([{"id":"model","currentValue":"[\"deepseek-account\",\"deepseek-v4-pro\"]",
            "options":[
                {"group":"deepseek-account","options":[{"value":"[\"deepseek-account\",\"deepseek-v4-pro\"]"}]},
                {"group":"deepseek-official","options":[
                    {"value":"[\"deepseek-official\",\"deepseek-v4-flash\"]"},
                    {"value":"[\"deepseek-official\",\"deepseek-v4-pro\"]"}
                ]},
                {"group":"custom","options":[{"value":"[\"custom\",\"private-model\"]"}]}
            ]
        }]);
        let replacement = fallback_key_model(&options).unwrap();
        assert_eq!(replacement, "[\"deepseek-official\",\"deepseek-v4-pro\"]");
        options[0]["currentValue"] = replacement.into();
        filter_account_models(&mut options);
        assert_eq!(options[0]["options"][0]["group"], "deepseek-official");
        assert_eq!(options[0]["options"][1]["group"], "custom");
        assert_eq!(fallback_key_model(&options), None);
    }

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

    #[test]
    fn first_rating_sends_an_explicit_absence_precondition() {
        let request: MessageFeedbackRequest = serde_json::from_value(json!({
            "operation": "put", "messageId": "answer", "rating": "positive",
        }))
        .unwrap();
        let wire = serde_json::to_value(request).unwrap();
        assert_eq!(
            wire,
            json!({
                "operation": "put", "messageId": "answer",
                "rating": "positive", "ifVersion": null,
            })
        );
    }
}
