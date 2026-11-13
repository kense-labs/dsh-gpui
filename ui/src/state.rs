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
    /// 官方 SystemPromptRow：本轮完整系统提示词的折叠行。
    SystemPrompt,
    System,
    Error,
    /// 官方 TurnMaxTokensItem：本轮撞到输出 token 上限的警告行。
    Warning,
    /// 官方 ContextInjectionRow 的工具变更变体：`developer/message` 的工具增删通知。
    ToolChange,
    /// 官方 CompactionItem：模型历史被压缩后的标记行。
    Compaction,
    /// 官方 DeliverablesTail：本轮 `present` 交付的文件卡片。
    Deliverable,
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
            let source = update
                .get("sourceKind")
                .and_then(Value::as_str)
                .unwrap_or("goal");
            // The ACP transport names every non-human message `turn_trigger`.
            // Only waking producers open a Turn; injected model history becomes
            // the collapsed context row the durable projection also builds, or
            // live and replayed views would disagree (and a step's time snapshot
            // would surface as a giant user bubble).
            if !is_trigger_source(source) {
                if source != "user-question-reply" {
                    apply_context_injection(entries, update);
                }
                return effect;
            }
            let id = update
                .get("messageId")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("trigger-{}", entries.len()));
            // Variant-selecting source facts: a subagent's stop reason or a
            // webhook's provider (official turnTriggerDetails).
            let detail = match source {
                "subagent" | "subagent-settled" => update
                    .get("stopReason")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                "webhook" | "github" => update
                    .get("provider")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                _ => String::new(),
            };
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
                if !detail.is_empty() {
                    entry.status = detail;
                }
            } else {
                entries.push(ChatEntry {
                    key: format!("trigger-{id}"),
                    external_id: Some(id),
                    role: EntryRole::User,
                    title,
                    text,
                    input: String::new(),
                    status: detail,
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
            // Official retry details show the scheduled delay in milliseconds; the
            // summary's countdown seconds derive from the same number.
            let delay_ms = update
                .get("delayMs")
                .and_then(|value| {
                    value
                        .as_u64()
                        .or_else(|| value.as_f64().map(|ms| ms.round() as u64))
                })
                .or_else(|| {
                    update
                        .get("delaySeconds")
                        .and_then(Value::as_u64)
                        .map(|seconds| seconds.saturating_mul(1000))
                })
                .unwrap_or(1000);
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
                entry.input = delay_ms.to_string();
                entry.text = text;
            } else {
                entries.push(ChatEntry {
                    key: format!("retry-{id}"),
                    external_id: Some(id),
                    role: EntryRole::Retry,
                    title,
                    text,
                    input: delay_ms.to_string(),
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
        Some("context_injection") => apply_context_injection(entries, update),
        // Host/ACP 扩展：压缩标记行（官方 CompactionItem）。摘要事件先到，缓存
        // 起来供紧随其后的 checkpoint 使用；两者都以 compactionId 去重。
        Some("gpui_compaction_summary") => apply_compaction_summary(update),
        Some("gpui_compaction") => apply_compaction_entry(entries, update),
        // Host/ACP 扩展：`deliverables/presented`（官方 DeliverablesTail）。
        // 交付卡片属于轮次尾部，由 web 层暂存到回合结束再落行。
        Some("gpui_deliverables") => {}
        // Host/ACP 扩展：系统提示词折叠行（官方 SystemPromptRow）。系统提示词
        // 是模型可见的完整文本，只作为可展开的一行出现，不参与任何状态判断。
        Some("gpui_system_prompt") => {
            let text = update
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned();
            if text.is_empty() {
                return effect;
            }
            let seq = update
                .get("seq")
                .and_then(Value::as_u64)
                .map(|seq| seq.to_string())
                .unwrap_or_else(|| format!("{}", entries.len()));
            let key = format!("system-prompt-{seq}");
            let title = if update["update"] == true {
                "系统提示词更新"
            } else {
                "系统提示词"
            };
            if let Some(entry) = entries.iter_mut().find(|entry| entry.key == key) {
                entry.text = text;
                entry.title = title.to_owned();
                return effect;
            }
            entries.push(ChatEntry {
                key,
                external_id: update
                    .get("seq")
                    .and_then(Value::as_u64)
                    .map(|seq| seq.to_string()),
                role: EntryRole::SystemPrompt,
                title: title.to_owned(),
                text,
                input: String::new(),
                status: String::new(),
                tool: None,
                at_ms: update.get("time").and_then(Value::as_f64),
            });
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
/// Friendly label for one durable context-injection producer kind.
///
/// The official row always titles itself 上下文注入 and names the producer in
/// its detail slot; the raw source kind (`time-context`, `skill-catalog`, …) is
/// an internal identifier a reader should not have to decode.
pub fn context_source_label(kind: &str) -> &str {
    match kind {
        "skill-catalog" => "技能目录",
        "time-context" => "时间上下文",
        "runtime-context" => "运行时上下文",
        "compact-checkpoint" => "压缩快照",
        "session-reference" => "跨会话召回",
        "agent-instructions" | "instructions" => "智能体指令",
        "directives" => "指令",
        "skill-invocation" => "已载入技能",
        "tool-goal" => "目标更新",
        "tool-jobs" => "后台任务通知",
        "user-approval" => "审批结果",
        "model-selection" => "模型选择",
        "user-question-reply" => "问题回复",
        "" => "上下文注入",
        other => other,
    }
}

/// Which model-facing role a logged non-user message plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContextRole {
    #[default]
    Inject,
    /// Material lifted out of another session's log.
    Recall,
}

/// Header of one context-injection row (official `ContextProducerView` + title).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContextHeader {
    pub role: ContextRole,
    /// Localized row title: 上下文注入 or 跨会话召回.
    pub title: &'static str,
    /// Producer label; empty when the source carries no readable kind.
    pub label: String,
    /// Collapsed one-line account, only for the `notice` form.
    pub summary: Option<String>,
}

/// One context form body, mirroring the official `ContextBody` unions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextBodyView {
    /// Published skill catalog: name + description rows.
    Catalog {
        entries: Vec<(String, String)>,
        replaced: bool,
        truncated: Option<usize>,
    },
    /// Named runtime/time snapshot contributions.
    Snapshot { sections: Vec<(String, String)> },
    /// A notice's model-facing text.
    Notice { text: String },
    /// Reconciled instruction files (`(path, localized action)`), then text.
    Instructions {
        changes: Vec<(String, &'static str)>,
        text: String,
    },
    /// Cross-agent relay: sender session id, then text.
    Relay { sender: String, text: String },
    /// Cross-session recall: completeness per referenced session, then text.
    Recall {
        sessions: Vec<(String, u64, u64, bool)>,
        text: String,
    },
    /// The default body: model-facing text as recorded.
    Opaque { text: String },
}

fn context_source<'a>(value: &'a Value) -> &'a Value {
    value
        .get("source")
        .filter(|source| source.is_object())
        .unwrap_or(value)
}

/// Model-facing content recorded with a context message, in block order.
fn context_model_text(value: &Value) -> String {
    value
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn joined_non_empty(values: impl IntoIterator<Item = String>) -> Option<String> {
    let labels: Vec<String> = values
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect();
    (!labels.is_empty()).then(|| labels.join(", "))
}

/// Header of one context row from its stored payload.
///
/// Known producers replace the bare kind with what the durable source actually
/// names (instruction paths, referenced session labels, the loaded skill);
/// every other producer keeps its durable kind verbatim, like the official
/// projection.
pub fn context_header(payload: &str) -> ContextHeader {
    let value: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
    let source = context_source(&value);
    let kind = source
        .get("kind")
        .and_then(Value::as_str)
        .or_else(|| value.get("sourceKind").and_then(Value::as_str))
        .unwrap_or_default();
    let form = source
        .get("form")
        .and_then(Value::as_str)
        .or_else(|| value.get("form").and_then(Value::as_str));
    let summary = (form == Some("notice"))
        .then(|| {
            source
                .get("summary")
                .and_then(Value::as_str)
                .filter(|summary| !summary.trim().is_empty())
                .map(str::to_owned)
        })
        .flatten();
    let strings = |list: &str, field: &str| {
        source
            .get(list)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get(field).and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let (role, title, label) = match kind {
        "session-reference" => (
            ContextRole::Recall,
            "跨会话召回",
            joined_non_empty(strings("references", "label")).unwrap_or_else(|| kind.to_owned()),
        ),
        "agent-instructions" => (
            ContextRole::Inject,
            "上下文注入",
            joined_non_empty(strings("changes", "path")).unwrap_or_else(|| kind.to_owned()),
        ),
        "skill-invocation" => (
            ContextRole::Inject,
            "上下文注入",
            source
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| kind.to_owned()),
        ),
        "" => (ContextRole::Inject, "上下文注入", String::new()),
        other => (ContextRole::Inject, "上下文注入", other.to_owned()),
    };
    ContextHeader {
        role,
        title,
        label,
        summary,
    }
}

/// Body of one context row from its stored payload.
///
/// Every form is read all-or-nothing: a declared form whose records are not
/// fully readable falls back to the opaque body rather than presenting a
/// confident but incomplete account.
pub fn context_body_view(payload: &str, fallback: &str) -> ContextBodyView {
    let value: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
    let source = context_source(&value);
    let text = context_model_text(&value);
    let text = if text.trim().is_empty() {
        fallback.to_owned()
    } else {
        text
    };
    let opaque = || ContextBodyView::Opaque { text: text.clone() };
    let form = source
        .get("form")
        .and_then(Value::as_str)
        .or_else(|| value.get("form").and_then(Value::as_str));
    match form {
        Some("catalog") => {
            let Some(entries) = source.get("entries").and_then(Value::as_array) else {
                return opaque();
            };
            let mut out = Vec::with_capacity(entries.len());
            for entry in entries {
                let (Some(name), Some(description)) = (
                    entry.get("name").and_then(Value::as_str),
                    entry.get("description").and_then(Value::as_str),
                ) else {
                    return opaque();
                };
                if name.is_empty() {
                    return opaque();
                }
                out.push((name.to_owned(), description.to_owned()));
            }
            const MAX_ENTRIES: usize = 200;
            let truncated = (out.len() > MAX_ENTRIES).then(|| out.len() - MAX_ENTRIES);
            out.truncate(MAX_ENTRIES);
            ContextBodyView::Catalog {
                entries: out,
                replaced: source.get("update") == Some(&Value::Bool(true)),
                truncated,
            }
        }
        Some("snapshot") => {
            let Some(sections) = source.get("sections").and_then(Value::as_array) else {
                return opaque();
            };
            let mut out = Vec::with_capacity(sections.len());
            for section in sections {
                let (Some(name), Some(section_text)) = (
                    section.get("name").and_then(Value::as_str),
                    section.get("text").and_then(Value::as_str),
                ) else {
                    return opaque();
                };
                if name.is_empty() {
                    return opaque();
                }
                out.push((name.to_owned(), section_text.to_owned()));
            }
            if out.is_empty() {
                return opaque();
            }
            ContextBodyView::Snapshot { sections: out }
        }
        Some("notice") => ContextBodyView::Notice { text },
        Some("instructions") => {
            let Some(changes) = source.get("changes").and_then(Value::as_array) else {
                return opaque();
            };
            let baseline = source.get("baseline") == Some(&Value::Bool(true));
            let mut out = Vec::with_capacity(changes.len());
            for change in changes {
                let (Some(path), Some(action)) = (
                    change.get("path").and_then(Value::as_str),
                    change.get("action").and_then(Value::as_str),
                ) else {
                    return opaque();
                };
                if path.is_empty() {
                    return opaque();
                }
                let label = match action {
                    "remove" => "已移除",
                    "set" if baseline => "已载入",
                    "set" => "已新增",
                    "replace" if baseline => "已载入",
                    "replace" => "已更新",
                    _ => return opaque(),
                };
                if !out
                    .iter()
                    .any(|(seen, _): &(String, &'static str)| *seen == path)
                {
                    out.push((path.to_owned(), label));
                }
            }
            if out.is_empty() {
                return opaque();
            }
            ContextBodyView::Instructions { changes: out, text }
        }
        Some("relay") => {
            let sender = source
                .get("senderSessionId")
                .and_then(Value::as_str)
                .filter(|sender| !sender.is_empty());
            match sender {
                Some(sender) => ContextBodyView::Relay {
                    sender: sender.to_owned(),
                    text,
                },
                None => opaque(),
            }
        }
        Some("recall") => {
            let Some(references) = source.get("references").and_then(Value::as_array) else {
                return opaque();
            };
            let mut out = Vec::with_capacity(references.len());
            for reference in references {
                let label = reference.get("label").and_then(Value::as_str);
                let retained = reference.get("retainedMessages").and_then(Value::as_u64);
                let omitted = reference.get("omittedMessages").and_then(Value::as_u64);
                let truncated = reference.get("truncated").and_then(Value::as_bool);
                let (Some(label), Some(retained), Some(omitted), Some(truncated)) =
                    (label, retained, omitted, truncated)
                else {
                    return opaque();
                };
                if label.is_empty() {
                    return opaque();
                }
                out.push((label.to_owned(), retained, omitted, truncated));
            }
            if out.is_empty() {
                return opaque();
            }
            ContextBodyView::Recall {
                sessions: out,
                text,
            }
        }
        _ => opaque(),
    }
}

/// Body of one context-injection row.
///
/// The skill catalog carries structured `entries`; every other producer is a
/// plain snapshot whose text lives in `content` (an ACP update sends it as one
/// string, a durable event as content blocks). Both must stay readable after a
/// session is reopened.
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
    if lines.is_empty() {
        lines = content_snapshot_text(value);
    }
    lines.join("\n")
}

/// Text of a plain (non-catalog) context snapshot.
fn content_snapshot_text(value: &Value) -> Vec<String> {
    let content = value.get("content").unwrap_or(&Value::Null);
    if let Some(text) = content.as_str() {
        let trimmed = text.trim();
        return if trimmed.is_empty() {
            Vec::new()
        } else {
            vec![trimmed.to_owned()]
        };
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Merge one context-injection update into a single collapsible row.
///
/// A live ACP update and a replayed durable event describe the same message id,
/// so both paths share this so a session shows one row per injection.
/// Latest compaction summaries keyed by compactionId (live evidence cache).
fn compaction_summary_cache() -> &'static std::sync::Mutex<
    std::collections::HashMap<String, (String, Option<usize>, Option<u64>)>,
> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, (String, Option<usize>, Option<u64>)>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Remember one live `compaction/summary` so the marker that follows can cite it.
fn apply_compaction_summary(update: &Value) {
    let Some(compaction_id) = update.get("compactionId").and_then(Value::as_str) else {
        return;
    };
    let summary = update
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let items = update
        .get("items")
        .and_then(Value::as_u64)
        .map(|items| items as usize);
    let tokens = update.get("tokens").and_then(Value::as_u64);
    if let Ok(mut cache) = compaction_summary_cache().lock() {
        cache.insert(compaction_id.to_owned(), (summary, items, tokens));
    }
}

/// Insert (or refresh) one live compaction marker row at its stream position.
fn apply_compaction_entry(entries: &mut Vec<ChatEntry>, update: &Value) {
    let Some(compaction_id) = update.get("compactionId").and_then(Value::as_str) else {
        return;
    };
    let cached = compaction_summary_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(compaction_id).cloned());
    let summary = update
        .get("summary")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|text| !text.trim().is_empty())
        .or_else(|| cached.as_ref().map(|(summary, _, _)| summary.clone()))
        .filter(|text| !text.trim().is_empty());
    let items = update
        .get("items")
        .and_then(Value::as_u64)
        .map(|items| items as usize)
        .or_else(|| cached.as_ref().and_then(|(_, items, _)| *items));
    let tokens = update
        .get("tokens")
        .and_then(Value::as_u64)
        .or_else(|| cached.as_ref().and_then(|(_, _, tokens)| *tokens));
    let entry = compaction_entry(
        compaction_id,
        summary.as_deref(),
        items,
        tokens,
        update.get("time").and_then(Value::as_f64),
    );
    if let Some(existing) = entries
        .iter_mut()
        .find(|existing| existing.role == EntryRole::Compaction && existing.key == entry.key)
    {
        *existing = entry;
    } else {
        entries.push(entry);
    }
}

fn apply_context_injection(entries: &mut Vec<ChatEntry>, update: &Value) {
    let id = string_field(update, "messageId");
    if id.is_empty() {
        return;
    }
    let changes = tool_changes(&update["toolChanges"]);
    if !changes.is_empty() {
        let entry = tool_change_entry(&id, &changes, update.get("time").and_then(Value::as_f64));
        if let Some(existing) = entries.iter_mut().find(|entry| {
            entry.role == EntryRole::ToolChange && entry.external_id.as_deref() == Some(id.as_str())
        }) {
            *existing = entry;
        } else {
            entries.push(entry);
        }
        return;
    }
    let text = context_entries_text(update);
    let payload = update.to_string();
    let title = context_header(&payload).label;
    let status = if update["update"] == true {
        "update".to_owned()
    } else {
        String::new()
    };
    if let Some(entry) = entries.iter_mut().find(|entry| {
        entry.role == EntryRole::Context && entry.external_id.as_deref() == Some(id.as_str())
    }) {
        entry.title = title;
        entry.input = payload;
        if !text.is_empty() {
            entry.text = text;
        }
        entry.status = status;
        return;
    }
    entries.push(ChatEntry {
        key: format!("context-{id}"),
        external_id: Some(id),
        role: EntryRole::Context,
        title,
        text,
        input: payload,
        status,
        tool: None,
        at_ms: update.get("time").and_then(Value::as_f64),
    });
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
        EntryRole::SystemPrompt => "system-prompt",
        EntryRole::System => "system",
        EntryRole::Error => "error",
        EntryRole::Warning => "warning",
        EntryRole::ToolChange => "tool-change",
        EntryRole::Compaction => "compaction",
        EntryRole::Deliverable => "deliverable",
    }
}

/// One localized terminal turn failure (official TurnErrorItem).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnFailure {
    /// Official `message.turnError` / `message.taskStopped`.
    pub title: &'static str,
    /// Official `failureMessage()` text for the failure code.
    pub message: String,
    /// Machine-readable code chip; absent for an interrupted process.
    pub code: Option<String>,
}

/// Read the failure row a `turn/end` reason implies, if any.
///
/// Mirrors official `endFailure` + `failureMessage`: provider `AUTH` errors keep
/// their raw diagnostic out of the UI (it can echo the key), quota and sign-out
/// failures get their localized copy, and any other recorded error keeps its
/// message. User cancellations, completions, and max-token ends have no row.
pub fn turn_failure(reason: &Value) -> Option<TurnFailure> {
    let kind = reason
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if kind == "interrupted" {
        return Some(TurnFailure {
            title: "任务已停止",
            message: "DSH 进程在本轮完成前退出（例如被强制结束、崩溃或关闭超时），重新启动后本轮已中止。未完成的工具调用结果未知。".to_owned(),
            code: None,
        });
    }
    if kind == "aborted" {
        let cause = reason
            .pointer("/reason/kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let detail = reason
            .pointer("/reason/reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if cause != "hook" || detail != "deepseek-account/signed-out" {
            return None;
        }
        return Some(TurnFailure {
            title: "任务已停止",
            message: "任务已因退出 DeepSeek 登录而停止。".to_owned(),
            code: Some("ACCOUNT_SIGNED_OUT".to_owned()),
        });
    }
    if kind != "error" {
        return None;
    }
    let error = reason.get("error").cloned().unwrap_or(Value::Null);
    let raw_code = error
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let quota = raw_code == "QUOTA"
        || raw_code == "ACCOUNT_QUOTA"
        || message.to_ascii_lowercase().contains("quota");
    let (localized, code) = match raw_code {
        "ACCOUNT_SIGNED_OUT" => (
            "任务已因退出 DeepSeek 登录而停止。".to_owned(),
            Some("ACCOUNT_SIGNED_OUT".to_owned()),
        ),
        "ACCOUNT_SIGN_IN_REQUIRED" => (
            "请先登录 DeepSeek，并确认请求地址支持账号认证。".to_owned(),
            Some("ACCOUNT_SIGN_IN_REQUIRED".to_owned()),
        ),
        "AUTH" if quota => ("当前请求的额度已用尽".to_owned(), Some(raw_code.to_owned())),
        // The provider AUTH diagnostic can echo a masked credential, so only
        // the localized cause is shown.
        "AUTH" => ("API 密钥无效".to_owned(), Some("AUTH".to_owned())),
        _ if quota => ("当前请求的额度已用尽".to_owned(), Some("QUOTA".to_owned())),
        _ => {
            let message = if message.is_empty() {
                "本轮运行失败".to_owned()
            } else {
                message.to_owned()
            };
            (message, (!raw_code.is_empty()).then(|| raw_code.to_owned()))
        }
    };
    Some(TurnFailure {
        title: "本轮运行失败",
        message: localized,
        code,
    })
}

/// Derive the transcript row one `turn/end` reason writes.
///
/// Shared by the live stream and history replay so a failed turn looks the same
/// before and after a restart.
pub fn turn_failure_entry(turn: u64, reason: &Value, at_ms: Option<f64>) -> Option<ChatEntry> {
    let failure = turn_failure(reason)?;
    Some(ChatEntry {
        key: format!("turn-error-{turn}"),
        external_id: Some(turn.to_string()),
        role: EntryRole::Error,
        title: failure.title.to_owned(),
        text: failure.message,
        input: String::new(),
        status: failure.code.unwrap_or_default(),
        tool: None,
        at_ms,
    })
}

/// Official `TurnMaxTokensItem`: a persistent warning row where a turn hit the
/// output-token cap instead of failing. The transcript keeps the truncated
/// reply; the row tells the reader to send "continue".
pub fn turn_max_tokens_entry(turn: u64, at_ms: Option<f64>) -> ChatEntry {
    ChatEntry {
        key: format!("turn-max-tokens-{turn}"),
        external_id: Some(turn.to_string()),
        role: EntryRole::Warning,
        title: "已达到输出 token 上限".into(),
        text: "回答被截断，已有输出保留在对话中。发送“继续”可让模型接着输出。".into(),
        input: String::new(),
        status: String::new(),
        tool: None,
        at_ms,
    }
}

/// One tool-registry change from a `developer/message` tool notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolChange {
    pub added: bool,
    pub name: String,
}

/// Parse the `toolChanges` array shared by live updates and replayed events.
pub fn tool_changes(value: &Value) -> Vec<ToolChange> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|change| {
            let name = change
                .get("name")
                .and_then(Value::as_str)?
                .trim()
                .to_owned();
            if name.is_empty() {
                return None;
            }
            Some(ToolChange {
                added: change.get("added").and_then(Value::as_bool).unwrap_or(true),
                name,
            })
        })
        .collect()
}

/// ChatEntry for a tool-registry notice: the official ContextInjectionRow picks
/// 已添加工具：{name} / 已移除工具：{name} for a single change, and 工具已更新 with
/// a 新增 N 个 / 移除 N 个 summary plus 新增：names / 移除：names body otherwise.
pub fn tool_change_entry(id: &str, changes: &[ToolChange], at_ms: Option<f64>) -> ChatEntry {
    let added: Vec<&str> = changes
        .iter()
        .filter(|change| change.added)
        .map(|change| change.name.as_str())
        .collect();
    let removed: Vec<&str> = changes
        .iter()
        .filter(|change| !change.added)
        .map(|change| change.name.as_str())
        .collect();
    let (title, status, text) = if changes.len() == 1 {
        let change = &changes[0];
        let title = if change.added {
            format!("已添加工具：{}", change.name)
        } else {
            format!("已移除工具：{}", change.name)
        };
        (title, String::new(), String::new())
    } else {
        let mut lines = Vec::new();
        if !added.is_empty() {
            lines.push(format!("新增：{}", added.join("、")));
        }
        if !removed.is_empty() {
            lines.push(format!("移除：{}", removed.join("、")));
        }
        let summary = match (added.len(), removed.len()) {
            (count, 0) => format!("新增 {count} 个"),
            (0, count) => format!("移除 {count} 个"),
            (added, removed) => format!("新增 {added} 个，移除 {removed} 个"),
        };
        ("工具已更新".to_owned(), summary, lines.join("\n"))
    };
    ChatEntry {
        key: format!("tool-change-{id}"),
        external_id: Some(id.to_owned()),
        role: EntryRole::ToolChange,
        title,
        text,
        input: String::new(),
        status,
        tool: None,
        at_ms,
    }
}

/// One turn's token buckets (official TurnUsagePanel data).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TurnTokenUsage {
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    /// `provider/model` routes used by the turn.
    pub routes: Vec<String>,
}

impl TurnTokenUsage {
    /// Cache-hit share of the prompt side, as the official panel prints it.
    pub fn cache_hit_percent(&self) -> Option<u64> {
        let prompt = self.input_tokens + self.cache_read_tokens + self.cache_write_tokens;
        if prompt == 0 || self.cache_read_tokens == 0 {
            return None;
        }
        Some((self.cache_read_tokens.saturating_mul(100) + prompt / 2) / prompt)
    }
}

/// Parse the native per-turn usage reply into a turn-keyed map.
pub fn turn_usage_map(reply: &Value) -> std::collections::HashMap<u64, TurnTokenUsage> {
    reply
        .get("turns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|turn| {
            let turn_number = turn.get("turn").and_then(Value::as_u64)?;
            let read = |key: &str| turn.get(key).and_then(Value::as_u64).unwrap_or(0);
            let routes = turn
                .get("routes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            Some((
                turn_number,
                TurnTokenUsage {
                    total_tokens: read("totalTokens"),
                    input_tokens: read("inputTokens"),
                    cache_read_tokens: read("cacheReadTokens"),
                    cache_write_tokens: read("cacheWriteTokens"),
                    output_tokens: read("outputTokens"),
                    routes,
                },
            ))
        })
        .collect()
}

/// Official `CompactionItem`: the marker left where the model history was
/// compacted. `items`/`tokens` are the shadowed counts from `compaction/summary`;
/// the summary text is markdown and only present when the log still carries it.
pub fn compaction_entry(
    compaction_id: &str,
    summary: Option<&str>,
    items: Option<usize>,
    tokens: Option<u64>,
    at_ms: Option<f64>,
) -> ChatEntry {
    let summary = summary.map(str::trim).filter(|text| !text.is_empty());
    let expandable = summary.is_some();
    let status = match (items, tokens) {
        (Some(items), Some(tokens)) => format!("已压缩 {items} 条历史记录（约 {tokens} tokens）"),
        _ if expandable => "点击查看压缩摘要".to_owned(),
        _ => "压缩摘要不可用".to_owned(),
    };
    ChatEntry {
        key: format!("compaction-{compaction_id}"),
        external_id: Some(compaction_id.to_owned()),
        role: EntryRole::Compaction,
        title: "上下文已压缩".to_owned(),
        text: summary.unwrap_or_default().to_owned(),
        input: String::new(),
        status,
        tool: None,
        at_ms,
    }
}

/// One image attached to a user row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserImage {
    pub attachment_id: String,
    pub media_type: String,
    pub name: String,
    pub width: Option<u64>,
    pub height: Option<u64>,
    /// Data URL filled in by `read_attachment` once the host has read the object.
    pub data_url: Option<String>,
}

/// Parse the images a user row carries (stored as JSON in `input`).
pub fn user_images(entry: &ChatEntry) -> Vec<UserImage> {
    if entry.role != EntryRole::User {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<Value>(&entry.input) else {
        return Vec::new();
    };
    value
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|image| {
            let attachment_id = image.get("attachmentId")?.as_str()?.to_owned();
            Some(UserImage {
                attachment_id,
                media_type: image
                    .get("mediaType")
                    .and_then(Value::as_str)
                    .unwrap_or("image/png")
                    .to_owned(),
                name: image
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                width: image.get("width").and_then(Value::as_u64),
                height: image.get("height").and_then(Value::as_u64),
                data_url: image
                    .get("dataUrl")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

/// Serialize images into the user row's `input` JSON (the field is otherwise
/// unused for user rows; keeping it here avoids widening ChatEntry).
pub fn user_images_json(images: &[UserImage]) -> String {
    let value = Value::Array(
        images
            .iter()
            .map(|image| {
                let mut item = serde_json::json!({
                    "attachmentId": image.attachment_id,
                    "mediaType": image.media_type,
                    "name": image.name,
                });
                if let Some(width) = image.width {
                    item["width"] = Value::from(width);
                }
                if let Some(height) = image.height {
                    item["height"] = Value::from(height);
                }
                if let Some(data_url) = &image.data_url {
                    item["dataUrl"] = Value::from(data_url.clone());
                }
                item
            })
            .collect(),
    );
    serde_json::json!({ "images": value }).to_string()
}

/// Patch a loaded data URL into every user row that references the attachment.
pub fn set_user_image_data(entries: &mut [ChatEntry], attachment_id: &str, data_url: &str) {
    for entry in entries
        .iter_mut()
        .filter(|entry| entry.role == EntryRole::User)
    {
        if !entry.input.contains(attachment_id) {
            continue;
        }
        let Ok(mut value) = serde_json::from_str::<Value>(&entry.input) else {
            continue;
        };
        let mut changed = false;
        if let Some(images) = value.get_mut("images").and_then(Value::as_array_mut) {
            for image in images {
                if image.get("attachmentId").and_then(Value::as_str) == Some(attachment_id)
                    && image.get("dataUrl").is_none()
                {
                    image["dataUrl"] = Value::from(data_url.to_owned());
                    changed = true;
                }
            }
        }
        if changed {
            entry.input = value.to_string();
        }
    }
}

/// Official `DeliverablesTail`: the files a closing turn announced through
/// `deliverables/presented`. Cards are self-contained (path + optional
/// description), so the row keeps the validated file list as JSON in `input`.
pub fn deliverable_entry(files: &[Value], at_ms: Option<f64>) -> ChatEntry {
    let files: Vec<Value> = files
        .iter()
        .filter_map(|file| {
            let path = file.get("path").and_then(Value::as_str)?.trim();
            if path.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "path": path,
                "description": file.get("description").and_then(Value::as_str).unwrap_or_default(),
            }))
        })
        .collect();
    ChatEntry {
        key: format!("deliverable-{}", files.len()),
        external_id: None,
        role: EntryRole::Deliverable,
        title: "交付文件".to_owned(),
        text: String::new(),
        input: Value::Array(files).to_string(),
        status: String::new(),
        tool: None,
        at_ms,
    }
}

/// Localized title of one turn-trigger row (official `turnTriggerDetails`).
///
/// `title` is the durable trigger key the projection stores (`goal`,
/// `trigger:subagent-settled`, …) and `detail` is the extra recorded source fact
/// that picks the variant: a subagent's stop reason or a webhook's provider.
pub fn trigger_title(title: &str, detail: &str) -> &'static str {
    match title {
        "goal" | "trigger:goal" => "继续执行目标",
        "trigger:agent" | "trigger:agent-message" => "收到任务消息",
        "trigger:team" | "trigger:team-message" => "收到团队消息",
        "trigger:schedule" => "自动化任务",
        "trigger:webhook" | "trigger:github" => {
            if detail == "github" {
                "收到 GitHub 事件"
            } else {
                "收到外部事件"
            }
        }
        // Official subagentTitle(): an unfamiliar or missing reason is never
        // reported as success.
        "trigger:subagent" | "trigger:subagent-settled" => match detail {
            "" => "子任务状态更新",
            "completed" => "子智能体已完成任务",
            "aborted" => "子智能体任务已停止",
            "error" => "子智能体执行失败",
            "max-tokens" => "子智能体已达 token 上限",
            "refusal" => "子智能体拒绝执行",
            _ => "子智能体异常结束",
        },
        "trigger:job" | "trigger:tool-jobs" => "后台任务状态更新",
        "trigger:plugin" => "插件状态更新",
        // Official fallback for an unfamiliar waking producer.
        _ => "收到执行请求",
    }
}

/// Whether a non-human message source wakes a Turn (official TurnTriggerNodeView).
///
/// Everything else is injected model history: the ACP transport forwards both
/// kinds as `turn_trigger`, so the client must classify by source instead of
/// trusting the update name.
pub fn is_trigger_source(kind: &str) -> bool {
    matches!(
        kind,
        "goal"
            | "schedule"
            | "webhook"
            | "github"
            | "agent"
            | "agent-message"
            | "team"
            | "team-message"
            | "subagent"
            | "subagent-settled"
            | "job"
            | "tool-jobs"
            // Goal tool notices open a continuation turn as well.
            | "tool-goal"
            | "plugin"
    )
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
        ChatEntry, ContextBodyView, ContextRole, EntryRole, adopt_logged_user_message,
        apply_update, context_body_view, context_entries_text, context_header,
        context_source_label, is_auth_failure, job_detail_text, job_status_label,
        prompt_failure_message, trigger_title, turn_failure, turn_failure_entry, turn_usage_map,
        validate_api_key,
    };
    use serde_json::json;

    #[test]
    fn a_plain_context_snapshot_reads_its_body_from_the_content() {
        // Live ACP updates carry the body as one string; durable events carry
        // content blocks. Both must expand to the model-facing text.
        let live = serde_json::json!({
            "sessionUpdate": "context_injection",
            "messageId": "m-1",
            "sourceKind": "time-context",
            "content": "Time sampled while preparing turn 1, step 1",
        });
        assert_eq!(
            context_entries_text(&live),
            "Time sampled while preparing turn 1, step 1"
        );
        let replayed = serde_json::json!({
            "id": "m-2",
            "source": {"kind": "runtime-context"},
            "content": [{"type": "text", "text": "runtime facts"}],
        });
        assert_eq!(context_entries_text(&replayed), "runtime facts");
        // The skill catalog still expands to its structured entries.
        let catalog = serde_json::json!({
            "source": {"entries": [{"name": "review", "description": "代码评审"}]},
            "content": [{"type": "text", "text": "ignored"}],
        });
        assert_eq!(context_entries_text(&catalog), "review — 代码评审");
    }

    #[test]
    fn context_producers_get_readable_labels() {
        assert_eq!(context_source_label("skill-catalog"), "技能目录");
        assert_eq!(context_source_label("time-context"), "时间上下文");
        assert_eq!(context_source_label("runtime-context"), "运行时上下文");
        assert_eq!(context_source_label("future-source"), "future-source");
    }

    #[test]
    fn context_headers_use_the_official_producer_labels() {
        // Unknown producers keep their durable kind verbatim.
        let time = serde_json::json!({
            "source": {"kind": "time-context", "form": "snapshot", "sections": []},
            "content": [{"type": "text", "text": "now"}],
        });
        let header = context_header(&time.to_string());
        assert_eq!(header.role, ContextRole::Inject);
        assert_eq!(header.title, "上下文注入");
        assert_eq!(header.label, "time-context");
        assert_eq!(header.summary, None);

        // A notice puts its one-line account on the collapsed row.
        let notice = serde_json::json!({
            "source": {"kind": "tool-jobs", "form": "notice", "summary": "后台任务已完成"},
            "content": [{"type": "text", "text": "done"}],
        });
        let header = context_header(&notice.to_string());
        assert_eq!(header.label, "tool-jobs");
        assert_eq!(header.summary.as_deref(), Some("后台任务已完成"));

        // Instruction rows are labelled by the files they reconciled.
        let instructions = serde_json::json!({
            "source": {"kind": "agent-instructions", "form": "instructions", "changes": [
                {"action": "set", "path": "AGENTS.md"},
                {"action": "replace", "path": "KUI/AGENTS.md"},
            ]},
        });
        assert_eq!(
            context_header(&instructions.to_string()).label,
            "AGENTS.md, KUI/AGENTS.md"
        );

        // A loaded skill names itself; a recall names the referenced sessions.
        let skill = serde_json::json!({
            "source": {"kind": "skill-invocation", "form": "instructions", "name": "ppt-generation"},
        });
        assert_eq!(context_header(&skill.to_string()).label, "ppt-generation");
        let recall = serde_json::json!({
            "source": {"kind": "session-reference", "form": "recall", "references": [
                {"label": "修复登录问题", "retainedMessages": 3, "omittedMessages": 1, "truncated": false},
            ]},
        });
        let header = context_header(&recall.to_string());
        assert_eq!(header.role, ContextRole::Recall);
        assert_eq!(header.title, "跨会话召回");
        assert_eq!(header.label, "修复登录问题");

        // A live update carries the kind at the top level.
        let live = serde_json::json!({"sourceKind": "runtime-context", "form": "snapshot"});
        assert_eq!(context_header(&live.to_string()).label, "runtime-context");
        // No readable kind drops the label entirely.
        assert_eq!(context_header("{}").label, "");
    }

    #[test]
    fn context_bodies_follow_the_declared_form_with_opaque_fallback() {
        // snapshot: named sections replace the joined framing prose.
        let snapshot = serde_json::json!({
            "source": {"kind": "runtime-context", "form": "snapshot", "sections": [
                {"name": "working-directory:current", "text": "Current working directory: \"/\"."},
                {"name": "sandbox:policy", "text": "workspace-write"},
            ]},
            "content": [{"type": "text", "text": "Current runtime context. This snapshot supersedes earlier snapshots."}],
        });
        match context_body_view(&snapshot.to_string(), "") {
            ContextBodyView::Snapshot { sections } => {
                assert_eq!(sections.len(), 2);
                assert_eq!(sections[0].0, "working-directory:current");
                assert_eq!(sections[1].1, "workspace-write");
            }
            other => panic!("expected snapshot body, got {other:?}"),
        }

        // catalog: entries plus the replacement marker.
        let catalog = serde_json::json!({
            "source": {"kind": "skill-catalog", "form": "catalog", "update": true, "entries": [
                {"name": "ppt-generation", "description": "生成演示文稿"},
            ]},
        });
        match context_body_view(&catalog.to_string(), "") {
            ContextBodyView::Catalog {
                entries,
                replaced,
                truncated,
            } => {
                assert_eq!(
                    entries,
                    vec![("ppt-generation".to_owned(), "生成演示文稿".to_owned())]
                );
                assert!(replaced);
                assert_eq!(truncated, None);
            }
            other => panic!("expected catalog body, got {other:?}"),
        }
        // An empty catalog is a real replacement, not a fallback.
        let empty_catalog = serde_json::json!({
            "source": {"kind": "skill-catalog", "form": "catalog", "update": true, "entries": []},
        });
        assert!(matches!(
            context_body_view(&empty_catalog.to_string(), ""),
            ContextBodyView::Catalog { entries, .. } if entries.is_empty()
        ));

        // notice: the model-facing text.
        let notice = serde_json::json!({
            "source": {"kind": "tool-goal", "form": "notice", "summary": "complete: 算圆周率"},
            "content": [{"type": "text", "text": "<goal_complete>\nObjective: 算圆周率"}],
        });
        match context_body_view(&notice.to_string(), "") {
            ContextBodyView::Notice { text } => assert!(text.contains("<goal_complete>")),
            other => panic!("expected notice body, got {other:?}"),
        }

        // instructions: actions localize against the baseline flag.
        let instructions = serde_json::json!({
            "source": {"kind": "agent-instructions", "form": "instructions", "baseline": false, "changes": [
                {"action": "set", "path": "AGENTS.md"},
                {"action": "replace", "path": "KUI/AGENTS.md"},
                {"action": "remove", "path": "OLD.md"},
            ]},
            "content": [{"type": "text", "text": "<system-reminder>..."}],
        });
        match context_body_view(&instructions.to_string(), "") {
            ContextBodyView::Instructions { changes, text } => {
                assert_eq!(
                    changes,
                    vec![
                        ("AGENTS.md".to_owned(), "已新增"),
                        ("KUI/AGENTS.md".to_owned(), "已更新"),
                        ("OLD.md".to_owned(), "已移除"),
                    ]
                );
                assert!(text.contains("<system-reminder>"));
            }
            other => panic!("expected instructions body, got {other:?}"),
        }

        // relay and recall.
        let relay = serde_json::json!({
            "source": {"kind": "agent-message", "form": "relay", "senderSessionId": "7a3a"},
            "content": [{"type": "text", "text": "Agent 7a3a sent a message"}],
        });
        match context_body_view(&relay.to_string(), "") {
            ContextBodyView::Relay { sender, .. } => assert_eq!(sender, "7a3a"),
            other => panic!("expected relay body, got {other:?}"),
        }
        let recall = serde_json::json!({
            "source": {"kind": "session-reference", "form": "recall", "references": [
                {"label": "修复登录", "retainedMessages": 3, "omittedMessages": 1, "truncated": true},
            ]},
            "content": [{"type": "text", "text": "material"}],
        });
        match context_body_view(&recall.to_string(), "") {
            ContextBodyView::Recall { sessions, .. } => {
                assert_eq!(sessions, vec![("修复登录".to_owned(), 3, 1, true)]);
            }
            other => panic!("expected recall body, got {other:?}"),
        }

        // An unreadable declared form falls back to the opaque model text.
        let broken = serde_json::json!({
            "source": {"kind": "skill-catalog", "form": "catalog", "entries": [{"name": "x"}]},
            "content": [{"type": "text", "text": "catalog prose"}],
        });
        match context_body_view(&broken.to_string(), "fallback") {
            ContextBodyView::Opaque { text } => assert_eq!(text, "catalog prose"),
            other => panic!("expected opaque body, got {other:?}"),
        }
        // Without model-facing content the row's recorded text is the fallback.
        match context_body_view(
            &serde_json::json!({"sourceKind": "goal"}).to_string(),
            "记录文本",
        ) {
            ContextBodyView::Opaque { text } => assert_eq!(text, "记录文本"),
            other => panic!("expected opaque body, got {other:?}"),
        }
    }

    #[test]
    fn a_time_context_injection_is_a_context_row_not_a_trigger() {
        let mut entries = Vec::new();
        let effect = apply_update(
            &mut entries,
            &serde_json::json!({
                "sessionUpdate": "context_injection",
                "messageId": "ctx-1",
                "sourceKind": "time-context",
                "content": "Time sampled while preparing turn 2, step 1",
                "time": 10,
            }),
        );
        assert!(!effect.turn_triggered);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, EntryRole::Context);
        assert_eq!(entries[0].title, "time-context");
        assert_eq!(
            entries[0].text,
            "Time sampled while preparing turn 2, step 1"
        );
    }

    #[test]
    fn trigger_titles_follow_the_recorded_source_variant() {
        // Official subagentTitle(): outcomes are never collapsed into one label,
        // and an unknown reason never implies success.
        assert_eq!(
            trigger_title("trigger:subagent-settled", ""),
            "子任务状态更新"
        );
        assert_eq!(
            trigger_title("trigger:subagent-settled", "completed"),
            "子智能体已完成任务"
        );
        assert_eq!(
            trigger_title("trigger:subagent-settled", "aborted"),
            "子智能体任务已停止"
        );
        assert_eq!(
            trigger_title("trigger:subagent-settled", "error"),
            "子智能体执行失败"
        );
        assert_eq!(
            trigger_title("trigger:subagent-settled", "max-tokens"),
            "子智能体已达 token 上限"
        );
        assert_eq!(
            trigger_title("trigger:subagent-settled", "refusal"),
            "子智能体拒绝执行"
        );
        assert_eq!(
            trigger_title("trigger:subagent-settled", "mystery"),
            "子智能体异常结束"
        );
        // A GitHub webhook is told apart by its recorded provider.
        assert_eq!(
            trigger_title("trigger:webhook", "github"),
            "收到 GitHub 事件"
        );
        assert_eq!(trigger_title("trigger:webhook", ""), "收到外部事件");
        assert_eq!(trigger_title("goal", ""), "继续执行目标");
        assert_eq!(trigger_title("trigger:tool-jobs", ""), "后台任务状态更新");
        assert_eq!(trigger_title("trigger:unheard-of", ""), "收到执行请求");
    }

    #[test]
    fn a_subagent_notice_keeps_its_stop_reason_for_the_title() {
        let mut entries = Vec::new();
        let effect = apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "turn_trigger",
                "messageId": "settled-1",
                "sourceKind": "subagent-settled",
                "stopReason": "aborted",
                "content": "Background subagent agent-1 finished.",
                "time": 3,
            }),
        );
        assert!(effect.turn_triggered);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, "aborted");
        assert_eq!(
            trigger_title(&entries[0].title, &entries[0].status),
            "子智能体任务已停止"
        );
        // The bridge's own trigger update may land second and must not drop it.
        apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "turn_trigger",
                "messageId": "settled-1",
                "sourceKind": "subagent-settled",
                "content": "Background subagent agent-1 finished.",
            }),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, "aborted");
    }

    #[test]
    fn a_transport_trigger_for_injected_context_becomes_a_context_row() {
        // The ACP transport labels every non-human message `turn_trigger`. An
        // injection must not start a reply or become a user bubble; it is the
        // same collapsed row the durable projection builds.
        let mut entries = Vec::new();
        let effect = apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "turn_trigger",
                "messageId": "ctx-9",
                "sourceKind": "time-context",
                "form": "snapshot",
                "content": "Time sampled while preparing turn 3, step 1",
                "time": 5,
            }),
        );
        assert!(!effect.turn_triggered);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, EntryRole::Context);
        assert_eq!(entries[0].title, "time-context");
        // A waking producer keeps its trigger card and opens the turn.
        let effect = apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "turn_trigger",
                "messageId": "goal-2",
                "sourceKind": "goal",
                "content": "<goal_round>\nObjective: \"继续\"",
                "time": 6,
            }),
        );
        assert!(effect.turn_triggered);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].role, EntryRole::User);
        assert_eq!(entries[1].title, "goal");
    }

    #[test]
    fn turn_failures_localize_provider_errors() {
        // Payloads copied from real `turn/end` reasons in ~/.dsh logs.
        let auth = turn_failure(&json!({
            "kind": "error",
            "error": {
                "message": "Authentication Fails, Your api key: ****3E7l is invalid (request_id: b1806464)",
                "code": "AUTH", "status": 401,
            },
        }))
        .expect("auth failure");
        assert_eq!(auth.title, "本轮运行失败");
        assert_eq!(auth.message, "API 密钥无效");
        // The provider diagnostic can echo the credential, so it never renders.
        assert!(!auth.message.contains("3E7l"));
        assert_eq!(auth.code.as_deref(), Some("AUTH"));

        let quota = turn_failure(&json!({
            "kind": "error",
            "error": {
                "message": "token quota is not enough, token remain quota: $0.03, need quota: $0.03",
                "code": "AUTH", "status": 403,
            },
        }))
        .expect("quota failure");
        assert_eq!(quota.message, "当前请求的额度已用尽");

        let transport = turn_failure(&json!({
            "kind": "error",
            "error": {"message": "DeepSeek Messages transport failed", "code": "TRANSPORT"},
        }))
        .expect("transport failure");
        assert_eq!(transport.message, "DeepSeek Messages transport failed");
        assert_eq!(transport.code.as_deref(), Some("TRANSPORT"));
    }

    #[test]
    fn user_stops_and_completions_have_no_failure_row() {
        assert!(turn_failure(&json!({"kind": "completed"})).is_none());
        assert!(turn_failure(&json!({"kind": "aborted", "reason": {"kind": "user"}})).is_none());
        assert!(turn_failure(&json!({"kind": "max_tokens"})).is_none());
        // A sign-out cancellation is a stopped task with its own copy.
        let signed_out = turn_failure(&json!({
            "kind": "aborted",
            "reason": {"kind": "hook", "reason": "deepseek-account/signed-out"},
        }))
        .expect("signed-out stop");
        assert_eq!(signed_out.title, "任务已停止");
        // An interrupted process end keeps the recovery explanation.
        let interrupted = turn_failure(&json!({"kind": "interrupted"})).expect("interrupted");
        assert!(interrupted.message.contains("进程"));
        assert!(interrupted.code.is_none());
    }

    #[test]
    fn a_turn_failure_writes_one_transcript_row() {
        let entry = turn_failure_entry(
            3,
            &json!({
                "kind": "error",
                "error": {"message": "Authentication Fails", "code": "AUTH", "status": 401},
            }),
            Some(12.0),
        )
        .expect("failure row");
        assert_eq!(entry.role, EntryRole::Error);
        assert_eq!(entry.key, "turn-error-3");
        assert_eq!(entry.title, "本轮运行失败");
        assert_eq!(entry.text, "API 密钥无效");
        assert_eq!(entry.status, "AUTH");
    }

    #[test]
    fn a_live_system_prompt_is_a_collapsed_row() {
        let mut entries = Vec::new();
        let effect = apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "gpui_system_prompt",
                "seq": 8,
                "text": "You are an AI agent powered by DeepSeek Harness.",
                "time": 30,
            }),
        );
        // A prompt disclosure never starts or ends a turn on its own.
        assert!(!effect.turn_triggered);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, EntryRole::SystemPrompt);
        assert_eq!(entries[0].title, "系统提示词");
        // The same sequence updates in place (one row per prompt position).
        apply_update(
            &mut entries,
            &json!({
                "sessionUpdate": "gpui_system_prompt",
                "seq": 8,
                "text": "You are an AI agent powered by DeepSeek Harness. Updated.",
                "time": 31,
            }),
        );
        assert_eq!(entries.len(), 1);
        assert!(entries[0].text.ends_with("Updated."));
    }

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
    fn turn_usage_map_reads_buckets_and_cache_hit() {
        let reply = serde_json::json!({"turns": [
            {"turn": 1, "inputTokens": 100, "cacheReadTokens": 300, "cacheWriteTokens": 0,
             "outputTokens": 50, "totalTokens": 450,
             "routes": ["deepseek-official/deepseek-v4.1-flash"]},
            {"turn": 2, "inputTokens": 10, "cacheReadTokens": 0, "cacheWriteTokens": 0,
             "outputTokens": 5, "totalTokens": 15, "routes": []},
        ]});
        let map = turn_usage_map(&reply);
        assert_eq!(map.len(), 2);
        let first = &map[&1];
        assert_eq!(first.total_tokens, 450);
        assert_eq!(first.routes, vec!["deepseek-official/deepseek-v4.1-flash"]);
        // 300 of 400 prompt tokens came from cache.
        assert_eq!(first.cache_hit_percent(), Some(75));
        // No cached input means the row is omitted.
        assert_eq!(map[&2].cache_hit_percent(), None);
        assert_eq!(map[&2].total_tokens, 15);
        assert!(turn_usage_map(&serde_json::json!({})).is_empty());
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
        assert_eq!(entries[0].input, "1000");
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
        assert_eq!(entries[0].input, "3000");
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
