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

fn role_name(role: EntryRole) -> &'static str {
    match role {
        EntryRole::User => "user",
        EntryRole::Assistant => "assistant",
        EntryRole::Thought => "thought",
        EntryRole::Tool => "tool",
        EntryRole::QuestionReply => "question-reply",
        EntryRole::Plan => "plan",
        EntryRole::Retry => "retry",
        EntryRole::System => "system",
        EntryRole::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::{EntryRole, apply_update, validate_api_key};
    use serde_json::json;

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
}
