//! ACP session updates projected into GPUI presentation state.

use serde_json::Value;

/// Validate a draft without retaining or returning the credential value.
pub fn validate_api_key(value: &str) -> Result<(), &'static str> {
    if value.is_empty() {
        return Err("请输入 API Key。");
    }
    if !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        || value.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit())
        })
        || (matches!(value.as_bytes().first(), Some(b'\'' | b'"' | b'`'))
            && value.as_bytes().last() == value.as_bytes().first())
    {
        return Err("请输入 API Key 本身，不要包含引号、空格或环境变量赋值。");
    }
    Ok(())
}

/// ACP wraps every agent failure as JSON-RPC `-32603 Internal error`, so the
/// client has to recover credential failures from the detail text. Without
/// this, a user-fixable expired key is reported as an internal crash and the
/// UI tells the user to retry something that can never succeed.
pub fn is_auth_failure(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("authenticationfails")
        || lower.contains("authentication fails")
        || lower.contains("invalid api key")
        || (lower.contains("api key") && lower.contains("invalid"))
        || lower.contains("unauthorized")
}

/// Build the message shown when a prompt fails. The draft is always restored
/// by the caller, so both variants can safely mention it.
pub fn prompt_failure_message(error: &str) -> String {
    if is_auth_failure(error) {
        "API Key 已失效或未配置，请在「更多 → 设置 → API Key」更新密钥。草稿已保留。".to_owned()
    } else {
        format!("发送失败：{error}。草稿已保留，请重试。")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryRole {
    User,
    Assistant,
    Thought,
    Tool,
    QuestionReply,
    Plan,
    /// 官方 ModelRetryItem：模型请求重试行。
    Retry,
    /// 官方 GenericCommandCard：斜杠指令的结果行（`command/run` + `command/done`）。
    Command,
    /// 官方 ContextInjectionRow：上下文注入行（技能目录等）。
    Context,
    /// 官方后台任务行：`job/registered` 建立，`job/settled` 结算。
    Job,
    System,
    Error,
}

/// One message, thought, tool invocation, or local status row.
#[derive(Clone, Debug)]
pub struct ChatEntry {
    pub key: String,
    pub external_id: Option<String>,
    pub role: EntryRole,
    pub title: String,
    pub text: String,
    pub input: String,
    pub status: String,
    pub(crate) tool: Option<crate::tool_calls::ToolCall>,
    /// Wall-clock epoch milliseconds when the row appeared (user hover time).
    pub at_ms: Option<f64>,
}

impl ChatEntry {
    pub fn local(role: EntryRole, key: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            external_id: None,
            role,
            title: String::new(),
            text: text.into(),
            input: String::new(),
            status: String::new(),
            tool: None,
            at_ms: None,
        }
    }

    pub(crate) fn submitted_plan(plan: crate::submitted_plan::SubmittedPlan) -> Self {
        let mut entry = Self::local(
            EntryRole::Plan,
            format!("plan-{}", plan.call_id),
            plan.document.markdown,
        );
        entry.external_id = Some(plan.call_id);
        entry.title = plan.document.title;
        entry
    }
}

/// State produced by an ACP update in addition to conversation rows.
#[derive(Default)]
pub struct UpdateEffect {
    pub usage: Option<(u64, u64)>,
    pub config_options: Option<Vec<Value>>,
    /// Host extension: durable turn/end for autonomous goal rounds.
    pub turn_ended: Option<String>,
    /// Host extension: a waking turn-trigger was projected.
    pub turn_triggered: bool,
}

/// Merge one committed ACP update without duplicating same-message content blocks.
pub fn apply_update(entries: &mut Vec<ChatEntry>, update: &Value) -> UpdateEffect {
    let mut effect = UpdateEffect::default();
    match update.get("sessionUpdate").and_then(Value::as_str) {
        Some("agent_message_chunk") => {
            merge_content(entries, update, EntryRole::Assistant);
        }
        Some("agent_thought_chunk") => {
            merge_content(entries, update, EntryRole::Thought);
        }
        Some("gpui_question_record" | "gpui_ptc_dispatch") => {
            let event = &update["event"];
            if matches!(
                event["type"].as_str(),
                Some("tool/ptc-dispatch" | "tool/ptc-dispatch-start")
            ) {
                crate::tool_calls::apply_dispatch(entries, event);
            } else {
                crate::questions::apply_question_call_event(entries, event);
            }
            if let Some(entry) =
                crate::questions::reply_entry(event, event.get("questionArguments"))
                && !entries.iter().any(|existing| existing.key == entry.key)
            {
                entries.push(entry);
            }
        }
        Some("tool_call") => {
            let id = string_field(update, "toolCallId");
            if id.is_empty() {
                return effect;
            }
            let title = string_field(update, "title");
            let tool = Some(crate::tool_calls::ToolCall::root(title.clone()));
            let input = update.get("rawInput").map(pretty_value).unwrap_or_default();
            let status = string_field(update, "status");
            let at_ms = update.get("time").and_then(Value::as_f64);
            if let Some(entry) = entries.iter_mut().find(|entry| {
                entry.role == EntryRole::Tool
                    && !entry.is_subcall()
                    && entry.external_id.as_deref() == Some(id.as_str())
            }) {
                entry.title = title;
                entry.tool = tool;
                entry.input = input;
                entry.at_ms = at_ms.or(entry.at_ms);
                if entry.is_running_tool() {
                    entry.status = status;
                }
            } else {
                entries.push(ChatEntry {
                    key: format!("tool-{id}"),
                    external_id: Some(id),
                    role: EntryRole::Tool,
                    title,
                    text: String::new(),
                    input,
                    status,
                    tool,
                    at_ms,
                });
            }
        }
        Some("tool_call_update") => {
            let id = string_field(update, "toolCallId");
            let output = update
                .get("content")
                .and_then(Value::as_array)
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|block| block.get("content"))
                        .map(content_text)
                        .filter(|text| !text.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if let Some(entry) = entries.iter_mut().find(|entry| {
                entry.role == EntryRole::Tool
                    && !entry.is_subcall()
                    && entry.external_id.as_deref() == Some(id.as_str())
            }) {
                if update.get("content").is_some() {
                    entry.text = output;
                }
                if !matches!(entry.status.as_str(), "ASK_CANCELLED" | "ASK_ABORTED")
                    && let Some(status) = update["status"].as_str()
                {
                    entry.status = status.to_owned();
                }
                if let Some(title) = update.get("title").and_then(Value::as_str) {
                    entry.title = title.to_string();
                }
            }
        }
        Some("usage_update") => {
            let used = update.get("used").and_then(Value::as_u64).unwrap_or(0);
            let size = update.get("size").and_then(Value::as_u64).unwrap_or(0);
            effect.usage = Some((used, size));
        }
        // Host/ACP 扩展：非人类 waking 消息 → 官方 TurnTrigger 卡。
        Some("turn_trigger") => {
            let id = update
                .get("messageId")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("trigger-{}", entries.len()));
            let source = update
                .get("sourceKind")
                .and_then(Value::as_str)
                .unwrap_or("goal");
            if matches!(
                source,
                "runtime-context" | "compact-checkpoint" | "user-question-reply"
            ) {
                return effect;
            }
            let raw = update
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let (title, text) = if source == "goal" || raw.contains("<goal_round>") {
                let objective = raw
                    .split("Objective:")
                    .nth(1)
                    .map(|rest| {
                        rest.trim()
                            .trim_start_matches('"')
                            .lines()
                            .next()
                            .unwrap_or(rest)
                            .trim()
                            .trim_matches('"')
                            .trim_end_matches('\\')
                            .to_string()
                    })
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| raw.clone());
                ("goal".to_string(), objective)
            } else {
                (format!("trigger:{source}"), raw)
            };
            if text.trim().is_empty() {
                return effect;
            }
            effect.turn_triggered = true;
            if let Some(entry) = entries.iter_mut().find(|entry| {
                entry.role == EntryRole::User && entry.external_id.as_deref() == Some(id.as_str())
            }) {
                entry.title = title;
                entry.text = text;
            } else {
                entries.push(ChatEntry {
                    key: format!("trigger-{id}"),
                    external_id: Some(id),
                    role: EntryRole::User,
                    title,
                    text,
                    input: String::new(),
                    status: String::new(),
                    tool: None,
                    at_ms: update.get("time").and_then(Value::as_f64),
                });
            }
        }
        // Host/ACP 扩展：模型重试行（官方 ModelRetryItem）。
        Some("model_retry") => {
            let id = update
                .get("retryId")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("retry-{}", entries.len()));
            let title = update
                .get("statusLabel")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    let retry = update.get("retry").and_then(Value::as_u64).unwrap_or(1);
                    let maximum = update
                        .get("maximum")
                        .map(|value| match value {
                            Value::String(text) => text.clone(),
                            Value::Number(number) => number.to_string(),
                            _ => "∞".to_string(),
                        })
                        .unwrap_or_else(|| "5".to_string());
                    format!("{retry}/{maximum}")
                });
            let status = string_field(update, "retryState");
            let seconds = update
                .get("delaySeconds")
                .and_then(Value::as_u64)
                .or_else(|| {
                    update
                        .get("delayMs")
                        .and_then(Value::as_u64)
                        .map(|ms| (ms + 999) / 1000)
                })
                .unwrap_or(1);
            let text = update
                .get("failure")
                .and_then(|failure| failure.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if let Some(entry) = entries.iter_mut().find(|entry| {
                entry.role == EntryRole::Retry && entry.external_id.as_deref() == Some(id.as_str())
            }) {
                entry.title = title;
                entry.status = status;
                entry.input = seconds.to_string();
                entry.text = text;
            } else {
                entries.push(ChatEntry {
                    key: format!("retry-{id}"),
                    external_id: Some(id),
                    role: EntryRole::Retry,
                    title,
                    text,
                    input: seconds.to_string(),
                    status,
                    tool: None,
                    at_ms: None,
                });
            }
        }
        // Host/ACP 扩展：后台任务行。
        // `job/registered` 按 jobId 建立，`job/settled` 结算同一行；
        // 状态语义与文案对齐官方 ui-jobs 的 statusLabel。
        Some("gpui_job_registered" | "gpui_job_settled") => {
            let settled = update["sessionUpdate"] == "gpui_job_settled";
            let id = string_field(update, "jobId");
            if id.is_empty() {
                return effect;
            }
            let label = string_field(update, "label");
            let kind = string_field(update, "kind");
            if let Some(entry) = entries.iter_mut().find(|entry| {
                entry.role == EntryRole::Job && entry.external_id.as_deref() == Some(id.as_str())
            }) {
                if settled {
                    entry.status = job_status_label(&update).to_owned();
                    entry.text = job_detail_text(&update);
                    entry.input = job_finished_at(&update);
                }
            } else {
                let at_ms = update["startedAt"]
                    .as_f64()
                    .or_else(|| update.get("time").and_then(Value::as_f64));
                entries.push(ChatEntry {
                    key: format!("job-{id}"),
                    external_id: Some(id),
                    role: EntryRole::Job,
                    title: if label.is_empty() { kind } else { label },
                    text: job_detail_text(&update),
                    input: if settled {
                        job_finished_at(&update)
                    } else {
                        String::new()
                    },
                    status: if settled {
                        job_status_label(&update).to_owned()
                    } else {
                        "running".to_owned()
                    },
                    tool: None,
                    at_ms,
                });
            }
        }
        // Host/ACP 扩展：上下文注入行（官方 ContextInjectionRow）。
        // 技能目录是常驻的一类；`entries` 是该行展开后的内容，`update`
        // 表示这是替换目录而不是首次发布。
        Some("context_injection") => {
            let id = string_field(update, "messageId");
            if id.is_empty() {
                return effect;
            }
            let source = string_field(update, "sourceKind");
            let text = context_entries_text(update);
            let title = if source.is_empty() {
                "skill-catalog".to_owned()
            } else {
                source
            };
            if let Some(entry) = entries.iter_mut().find(|entry| {
                entry.role == EntryRole::Context
                    && entry.external_id.as_deref() == Some(id.as_str())
            }) {
                entry.title = title;
                entry.text = text;
                entry.status = if update["update"] == true {
                    "update".to_owned()
                } else {
                    String::new()
                };
            } else {
                entries.push(ChatEntry {
                    key: format!("context-{id}"),
                    external_id: Some(id),
                    role: EntryRole::Context,
                    title,
                    text,
                    input: String::new(),
                    status: if update["update"] == true {
                        "update".to_owned()
                    } else {
                        String::new()
                    },
                    tool: None,
                    at_ms: update.get("time").and_then(Value::as_f64),
                });
            }
        }
        // Host/ACP 扩展：斜杠指令结果行（官方 GenericCommandCard）。
        // `command/run` 建立行，`command/done` 回填结果；用 commandId 配对。
        Some("gpui_command_run" | "gpui_command_done") => {
            let run = update["sessionUpdate"] == "gpui_command_run";
            let id = string_field(update, "commandId");
            if id.is_empty() {
                return effect;
            }
            let name = string_field(update, "name");
            let at_ms = update.get("time").and_then(Value::as_f64);
            if let Some(entry) = entries.iter_mut().find(|entry| {
                entry.role == EntryRole::Command
                    && entry.external_id.as_deref() == Some(id.as_str())
            }) {
                if run {
                    if !name.is_empty() {
                        entry.title = name;
                    }
                    if let Some(args) = update.get("args").and_then(Value::as_str) {
                        entry.input = args.to_owned();
                    }
                } else {
                    // done：kind 决定成功/失败，text 是展开后的正文。
                    entry.status = string_field(update, "kind");
                    if let Some(text) = update.get("text").and_then(Value::as_str) {
                        entry.text = text.to_owned();
                    }
                }
                entry.at_ms = entry.at_ms.or(at_ms);
            } else if run || !name.is_empty() {
                // 历史窗口可能只有 done；官方同样按 done 建行。
                entries.push(ChatEntry {
                    key: format!("command-{id}"),
                    external_id: Some(id),
                    role: EntryRole::Command,
                    title: name,
                    text: update
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    input: update
                        .get("args")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    status: if run {
                        String::new()
                    } else {
                        string_field(update, "kind")
                    },
                    tool: None,
                    at_ms,
                });
            }
        }
        Some("config_option_update") => {
            effect.config_options = update
                .get("configOptions")
                .and_then(Value::as_array)
                .cloned();
        }
        // Host/ACP 扩展：自主轮次边界（无 open prompt RPC 时也要收尾）。
        Some("turn_end") => {
            effect.turn_ended = update
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| Some("completed".to_string()));
        }
        _ => {}
    }
    effect
}

fn merge_content(entries: &mut Vec<ChatEntry>, update: &Value, role: EntryRole) {
    let message_id = update
        .get("messageId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let text = update.get("content").map(content_text).unwrap_or_default();
    if text.is_empty() {
        return;
    }
    let existing = entries.iter_mut().find(|entry| {
        entry.role == role
            && match (&entry.external_id, &message_id) {
                (Some(left), Some(right)) => left == right,
                (None, None) => false,
                _ => false,
            }
    });
    if let Some(entry) = existing {
        entry.text.push_str(&text);
    } else {
        let key = message_id
            .clone()
            .unwrap_or_else(|| format!("{}-{}", role_name(role), entries.len()));
        entries.push(ChatEntry {
            key: format!("{}-{key}", role_name(role)),
            external_id: message_id,
            role,
            title: String::new(),
            text,
            input: String::new(),
            status: String::new(),
            tool: None,
            at_ms: None,
        });
    }
}

fn content_text(content: &Value) -> String {
    match content.get("type").and_then(Value::as_str) {
        Some("text") => string_field(content, "text"),
        Some("resource_link") => {
            let name = content
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("resource");
            let uri = content.get("uri").and_then(Value::as_str).unwrap_or("");
            format!("[{name}]({uri})")
        }
        Some("resource") => content
            .get("resource")
            .and_then(|resource| resource.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("[嵌入资源]")
            .to_string(),
        Some("image") => "[图片]".to_string(),
        Some("audio") => "[音频]".to_string(),
        _ => String::new(),
    }
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn pretty_value(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        text.to_string()
    } else {
        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
    }
}

/// 后台任务的结束时间戳（毫秒）。它以 JSON 数字到达，`string_field` 只读字符串，
/// 因此单独提取；解析不出时留空表示「仍在运行」。
///
/// 存成整数毫秒而不是数字的 Display 形式：serde_json 会把整数型 f64 渲染成
/// `...1009.0`，而时长只关心毫秒值。
pub fn job_finished_at(update: &Value) -> String {
    let millis = match update.get("finishedAt") {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) => text.parse::<f64>().ok(),
        _ => None,
    };
    match millis {
        Some(millis) => format!("{}", millis as i64),
        None => String::new(),
    }
}

/// 后台任务的终态。缺省按官方 wire 联合类型里的 `completed` 处理。
pub fn job_status_label(update: &Value) -> &'static str {
    match update["status"].as_str() {
        Some("running") => "running",
        Some("stopping") => "stopping",
        Some("killed") => "killed",
        Some("failed") => "failed",
        _ => "completed",
    }
}

/// 后台任务行的副标题：官方 `jobDetail` = 进行中的 `progress`，否则 `detail`。
pub fn job_detail_text(update: &Value) -> String {
    for key in ["progress", "detail"] {
        if let Some(text) = update[key].as_str()
            && !text.is_empty()
        {
            return text.to_owned();
        }
    }
    String::new()
}

/// 上下文注入行展开后的正文：官方 `SkillCatalogSource.entries` 的逐条呈现。
///
/// 实时事件把 `entries` 放在顶层，历史回放则嵌在 `data.source.entries`；两种来源
/// 共用这一个函数，避免同一行在两处渲染出不同内容。
pub fn context_entries_text(value: &Value) -> String {
    let source = value.get("source").unwrap_or(value);
    let mut lines: Vec<String> = source["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let name = entry["name"].as_str()?;
            Some(match entry["description"].as_str() {
                Some(description) if !description.is_empty() => {
                    format!("{name} — {description}")
                }
                _ => name.to_owned(),
            })
        })
        .collect();
    if source["incomplete"] == true {
        lines.push("（本次发现的目录不完整）".to_owned());
    }
    lines.join("\n")
}

fn role_name(role: EntryRole) -> &'static str {
    match role {
        EntryRole::User => "user",
        EntryRole::Assistant => "assistant",
        EntryRole::Thought => "thought",
        EntryRole::Tool => "tool",
        EntryRole::QuestionReply => "question-reply",
        EntryRole::Plan => "plan",
        EntryRole::Retry => "retry",
        EntryRole::Command => "command",
        EntryRole::Context => "context",
        EntryRole::Job => "job",
        EntryRole::System => "system",
        EntryRole::Error => "error",
    }
}

/// Adopt the durable identity of one logged human message.
///
/// The transcript renders a submitted prompt optimistically before the Host
/// logs it; when the durable `user/message` arrives the local row only needs its
/// identity. A message admitted from the Inbox (a steering item, or a queued row
/// that just became a turn) has no local row yet and is appended at the position
/// the Host gave it. `display` is the preformatted row text for that new row.
///
/// @returns whether a new row was appended.
pub fn adopt_logged_user_message(
    entries: &mut Vec<ChatEntry>,
    message_id: &str,
    display: String,
    at_ms: Option<f64>,
) -> bool {
    if entries
        .iter()
        .any(|entry| entry.external_id.as_deref() == Some(message_id))
    {
        return false;
    }
    if let Some(entry) = entries
        .iter_mut()
        .rev()
        .find(|entry| entry.role == EntryRole::User && entry.external_id.is_none())
    {
        entry.external_id = Some(message_id.to_owned());
        entry.at_ms = at_ms;
        return false;
    }
    let mut entry = ChatEntry::local(EntryRole::User, format!("user-{message_id}"), display);
    entry.external_id = Some(message_id.to_owned());
    entry.at_ms = at_ms;
    entries.push(entry);
    true
}

#[cfg(test)]
mod tests {
    use super::{
        ChatEntry, EntryRole, adopt_logged_user_message, apply_update, context_entries_text,
        is_auth_failure, job_detail_text, job_status_label, prompt_failure_message,
        validate_api_key,
    };
    use serde_json::json;

    #[test]
    fn a_logged_user_message_adopts_the_optimistic_row() {
        // Normal send: the local row is already visible, so the durable event
        // must only hand it the Host identity — never a second bubble.
        let mut entries = Vec::new();
        let mut local =
            ChatEntry::local(EntryRole::User, "local-1", "带附件的显示文案 [图片] a.png");
        local.at_ms = Some(1.0);
        entries.push(local);
        let appended = adopt_logged_user_message(&mut entries, "m-1", "[图片]".into(), Some(9.0));
        assert!(!appended);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].external_id.as_deref(), Some("m-1"));
        // The optimistic display keeps attachment names the log does not carry.
        assert_eq!(entries[0].text, "带附件的显示文案 [图片] a.png");
        assert_eq!(entries[0].at_ms, Some(9.0));
    }

    #[test]
    fn an_admitted_inbox_message_appends_its_own_row() {
        // Steering: the Host logs the message mid-turn with no local row
        // before it, so the row appears where the log put it.
        let mut entries = vec![ChatEntry::local(EntryRole::Assistant, "a-1", "正在处理")];
        let appended = adopt_logged_user_message(&mut entries, "m-2", "补充说明".into(), Some(5.0));
        assert!(appended);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].role, EntryRole::User);
        assert_eq!(entries[1].text, "补充说明");
        assert_eq!(entries[1].external_id.as_deref(), Some("m-2"));
    }

    #[test]
    fn a_replayed_user_message_identity_is_idempotent() {
        let mut entries = Vec::new();
        assert!(adopt_logged_user_message(
            &mut entries,
            "m-3",
            "第一条".into(),
            Some(1.0)
        ));
        assert!(!adopt_logged_user_message(
            &mut entries,
            "m-3",
            "第一条".into(),
            Some(1.0)
        ));
        assert_eq!(entries.len(), 1);
    }

    #[test]
    #[test]
    fn a_background_job_registers_and_settles_on_one_row() {
        // Payloads copied from a real `job/registered` / `job/settled` pair in
        // ~/.dsh so the row stays pinned to what the host writes.
        let mut entries = Vec::new();
        apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "gpui_job_registered",
                "jobId": "bash-09d541eae396400a",
                "kind": "bash",
                "label": "echo \"=== OS ===\"; uname -a",
                "startedAt": 1794453670976f64,
                "time": 1794453670976f64
            }),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, EntryRole::Job);
        assert_eq!(entries[0].status, "running");
        assert_eq!(entries[0].title, "echo \"=== OS ===\"; uname -a");
        assert_eq!(entries[0].at_ms, Some(1794453670976.0));

        apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "gpui_job_settled",
                "jobId": "bash-09d541eae396400a",
                "kind": "bash",
                "label": "echo \"=== OS ===\"; uname -a",
                "startedAt": 1794453670976f64,
                "finishedAt": 1794453671009f64,
                "status": "completed",
                "cause": "producer",
                "detail": "exit code: 127"
            }),
        );
        // Settling updates the same row rather than appending another.
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, "completed");
        assert_eq!(entries[0].text, "exit code: 127");
        assert_eq!(entries[0].input, "1794453671009");
    }

    #[test]
    fn a_job_falls_back_to_its_kind_when_the_label_is_missing() {
        let mut entries = Vec::new();
        apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "gpui_job_registered",
                "jobId": "j1",
                "kind": "bash"
            }),
        );
        assert_eq!(entries[0].title, "bash");
        // A settled event with no status is the wire union's default.
        let mut entries = Vec::new();
        apply_update(
            &mut entries,
            &json!({"sessionUpdate": "gpui_job_settled", "jobId": "j2", "kind": "bash"}),
        );
        assert_eq!(entries[0].status, "completed");
    }

    #[test]
    fn job_status_and_detail_cover_the_official_wire_union() {
        assert_eq!(job_status_label(&json!({"status": "running"})), "running");
        assert_eq!(job_status_label(&json!({"status": "stopping"})), "stopping");
        assert_eq!(job_status_label(&json!({"status": "killed"})), "killed");
        assert_eq!(job_status_label(&json!({"status": "failed"})), "failed");
        assert_eq!(
            job_status_label(&json!({"status": "completed"})),
            "completed"
        );
        assert_eq!(job_status_label(&json!({})), "completed");
        // `jobDetail` prefers progress while running, then detail.
        assert_eq!(job_detail_text(&json!({"progress": "3/10"})), "3/10");
        assert_eq!(
            job_detail_text(&json!({"detail": "exit code: 127"})),
            "exit code: 127"
        );
        assert_eq!(
            job_detail_text(&json!({"progress": "a", "detail": "b"})),
            "a"
        );
        assert_eq!(job_detail_text(&json!({})), "");
    }

    #[test]
    fn a_skill_catalog_becomes_one_collapsible_context_row() {
        let mut entries = Vec::new();
        let effect = apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "context_injection",
                "messageId": "ctx-1",
                "sourceKind": "skill-catalog",
                "form": "catalog",
                "entries": [
                    {"name": "ppt-generation", "description": "生成演示文稿"},
                    {"name": "ask-form", "description": "表单提问"},
                ],
                "time": 1.0
            }),
        );
        // An injection is not a turn trigger: it must not start a reply.
        assert!(!effect.turn_triggered);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, EntryRole::Context);
        assert_eq!(entries[0].title, "skill-catalog");
        assert_eq!(
            entries[0].text,
            "ppt-generation — 生成演示文稿\nask-form — 表单提问"
        );
        // The row is expandable because it has a body.
        assert!(!entries[0].text.trim().is_empty());

        // A replacement catalog updates the same row instead of adding another.
        apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "context_injection",
                "messageId": "ctx-1",
                "sourceKind": "skill-catalog",
                "update": true,
                "entries": [{"name": "only-one", "description": "唯一"}]
            }),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "only-one — 唯一");
        assert_eq!(entries[0].status, "update");
    }

    #[test]
    fn context_entry_text_reads_live_and_replayed_payloads() {
        // Live event: entries sit at the top level.
        assert_eq!(
            context_entries_text(&json!({
                "entries": [{"name": "a", "description": "A"}]
            })),
            "a — A"
        );
        // Replayed event: they sit under `source`.
        assert_eq!(
            context_entries_text(&json!({
                "source": {"kind": "skill-catalog", "entries": [{"name": "b", "description": "B"}]}
            })),
            "b — B"
        );
        // A name without a description still renders.
        assert_eq!(
            context_entries_text(&json!({"entries": [{"name": "c"}]})),
            "c"
        );
        // An incomplete discovery says so rather than looking authoritative.
        assert_eq!(
            context_entries_text(&json!({
                "entries": [{"name": "d", "description": "D"}], "incomplete": true
            })),
            "d — D\n（本次发现的目录不完整）"
        );
        // An empty catalog yields no body at all.
        assert_eq!(context_entries_text(&json!({"entries": []})), "");
        assert_eq!(context_entries_text(&json!({})), "");
    }

    #[test]
    fn auth_failures_get_an_actionable_message_instead_of_a_retry_hint() {
        let sample = "ACP -32603: Internal error; turn failed: AuthenticationFails. Your api key: sk-****3E7l is invalid (request_id: b1806464-4a3b-4e51-9691-1ac)";
        assert!(is_auth_failure(sample));
        let message = prompt_failure_message(sample);
        assert!(message.contains("API Key"));
        assert!(message.contains("设置"));
        assert!(
            !message.contains("请重试"),
            "auth failure must not suggest a plain retry"
        );
        assert!(
            !message.contains("Internal error"),
            "raw internal error text must be hidden"
        );
    }

    #[test]
    fn ordinary_failures_keep_the_original_detail_and_retry_hint() {
        let sample = "ACP -32000: connection reset";
        assert!(!is_auth_failure(sample));
        assert_eq!(
            prompt_failure_message(sample),
            "发送失败：ACP -32000: connection reset。草稿已保留，请重试。"
        );
    }

    #[test]
    fn api_key_validation_accepts_keys_and_rejects_assignments_and_quotes() {
        assert_eq!(validate_api_key("sk-test-not-a-real-key"), Ok(()));
        for value in ["", "DEEPSEEK_API_KEY=secret", "'secret'", "two words"] {
            assert!(
                validate_api_key(value).is_err(),
                "rejected draft: {value:?}"
            );
        }
    }

    #[test]
    fn turn_trigger_projects_goal_objective() {
        let mut entries = Vec::new();
        let effect = apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "turn_trigger",
                "messageId": "g1",
                "sourceKind": "goal",
                "content": "<goal_round>\nObjective: \"Fix the bug\"\nRound: 1/20\n\nContinue...</goal_round>",
                "time": 1_700_000_000_000f64
            }),
        );
        assert!(effect.turn_triggered);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "goal");
        assert_eq!(entries[0].text, "Fix the bug");
        assert_eq!(entries[0].role, EntryRole::User);
    }

    #[test]
    fn a_live_goal_round_becomes_a_trigger_card_with_its_objective() {
        // The `user/message` a goal round writes, with `content` flattened by the
        // bridge exactly as `contentBlocks` does. Reproduced from a real session
        // so a live round renders the same card history replay produces.
        let mut entries = Vec::new();
        let effect = apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "turn_trigger",
                "messageId": "goal-message-1",
                "sourceKind": "goal",
                "content": "<goal_round>\nObjective: \"帮我算出圆周率第100位\"\n\
                            Round: 1/256\n\nContinue working toward the objective in this same \
                            session.</goal_round>",
                "time": 1794454152069f64
            }),
        );
        assert!(effect.turn_triggered, "a live goal round must start a turn");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "goal");
        // The card shows the objective, not the whole `<goal_round>` envelope.
        assert_eq!(entries[0].text, "帮我算出圆周率第100位");
        assert_eq!(entries[0].at_ms, Some(1794454152069.0));
    }

    #[test]
    fn a_goal_completion_notice_is_not_swallowed_as_internal_context() {
        // `tool-goal` reports completion through a `notice` message. It is not one
        // of the internally filtered kinds, so it reaches the transcript.
        let mut entries = Vec::new();
        let effect = apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "turn_trigger",
                "messageId": "goal-notice-1",
                "sourceKind": "tool-goal",
                "form": "notice",
                "content": "complete: 帮我算出圆周率第100位"
            }),
        );
        assert!(effect.turn_triggered);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "trigger:tool-goal");
        assert_eq!(entries[0].text, "complete: 帮我算出圆周率第100位");
    }

    #[test]
    fn internal_context_and_durable_question_replies_do_not_start_turns() {
        let mut entries = Vec::new();
        for source in [
            "runtime-context",
            "compact-checkpoint",
            "user-question-reply",
        ] {
            let effect = apply_update(
                &mut entries,
                &json!({
                    "sessionUpdate": "turn_trigger",
                    "sourceKind": source,
                    "content": "This is not a waking message."
                }),
            );
            assert!(!effect.turn_triggered);
        }
        assert!(entries.is_empty());
    }

    #[test]
    fn turn_end_marks_effect() {
        let mut entries = Vec::new();
        let effect = apply_update(
            &mut entries,
            &json!({"sessionUpdate": "turn_end", "turn": 3, "reason": "completed"}),
        );
        assert_eq!(effect.turn_ended.as_deref(), Some("completed"));
        assert!(entries.is_empty());
    }

    #[test]
    fn committed_blocks_from_one_message_are_coalesced() {
        let mut entries = Vec::new();
        for text in ["你好", "，世界"] {
            apply_update(
                &mut entries,
                &json!({
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": "m1",
                    "content": {"type": "text", "text": text}
                }),
            );
        }
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, EntryRole::Assistant);
        assert_eq!(entries[0].text, "你好，世界");
    }

    #[test]
    fn tool_result_updates_the_original_invocation() {
        let mut entries = Vec::new();
        apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "tool_call",
                "toolCallId": "call-1",
                "title": "read_file",
                "status": "in_progress",
                "rawInput": {"path": "README.md"}
            }),
        );
        apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "call-1",
                "status": "completed",
                "content": [{"type": "content", "content": {"type": "text", "text": "done"}}]
            }),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, "completed");
        assert_eq!(entries[0].text, "done");
        assert!(entries[0].input.contains("README.md"));
    }

    #[test]
    fn usage_and_configuration_are_returned_separately() {
        let mut entries = Vec::new();
        let usage = apply_update(
            &mut entries,
            &json!({"sessionUpdate": "usage_update", "used": 120, "size": 1000}),
        );
        assert_eq!(usage.usage, Some((120, 1000)));
        let config = apply_update(
            &mut entries,
            &json!({"sessionUpdate": "config_option_update", "configOptions": [{"id": "model"}]}),
        );
        assert_eq!(config.config_options.map(|options| options.len()), Some(1));
    }

    #[test]
    fn model_retry_updates_upsert_retry_rows() {
        let mut entries = Vec::new();
        apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "model_retry",
                "retryId": "r1",
                "retry": 1,
                "maximum": 5,
                "retryState": "started",
                "delayMs": 1000,
                "failure": {"message": "timeout"}
            }),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, EntryRole::Retry);
        assert_eq!(entries[0].title, "1/5");
        assert_eq!(entries[0].status, "started");
        assert_eq!(entries[0].input, "1");
        assert_eq!(entries[0].text, "timeout");
        apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "model_retry",
                "retryId": "r1",
                "retry": 2,
                "maximum": 5,
                "retryState": "active",
                "delaySeconds": 3,
                "failure": {"message": "timeout"}
            }),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "2/5");
        assert_eq!(entries[0].status, "active");
        assert_eq!(entries[0].input, "3");
    }

    #[test]
    fn command_events_pair_by_command_id_and_carry_the_full_result() {
        let mut entries = Vec::new();
        apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "gpui_command_run",
                "commandId": "c1",
                "name": "goal",
                "args": "帮我算出圆周率第100位"
            }),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, EntryRole::Command);
        assert_eq!(entries[0].title, "goal");
        assert_eq!(entries[0].input, "帮我算出圆周率第100位");
        // `run` 未结算：摘要按「正在执行指令」渲染。
        assert!(entries[0].status.is_empty());

        let result = "Goal created\nStatus: active\nObjective: 帮我算出圆周率第100位\n\
                      Rounds: 0/256\nActivation: armed\n\n\
                      Commands: /goal edit <objective>, /goal pause, /goal clear";
        apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "gpui_command_done",
                "commandId": "c1",
                "name": "goal",
                "kind": "success",
                "text": result
            }),
        );
        // 同一个 commandId 回填原行，不新增行。
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, "success");
        assert_eq!(entries[0].text, result);
        assert_eq!(entries[0].title, "goal");

        // 官方：多行结果才可展开；摘要取首行。
        assert!(entries[0].text.contains('\n'));
        assert_eq!(entries[0].text.lines().next(), Some("Goal created"));

        // 失败结果同样按 commandId 落回同一行。
        apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "gpui_command_done",
                "commandId": "c1",
                "kind": "error",
                "text": "The goal command is not valid for the current state."
            }),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, "error");
    }

    #[test]
    fn a_done_without_its_run_still_builds_a_row() {
        // 历史窗口截断时可能只翻到 done；官方 fallbackState 同样建行。
        let mut entries = Vec::new();
        apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "gpui_command_done",
                "commandId": "c9",
                "name": "compact",
                "kind": "success",
                "text": "Compacted."
            }),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, EntryRole::Command);
        assert_eq!(entries[0].title, "compact");
        assert_eq!(entries[0].status, "success");
    }
}
