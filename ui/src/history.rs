//! Project DeepSeek Harness session.v4 JSONL events into chat rows.
//!
//! ACP `session/resume` intentionally does not replay history; the official
//! desktop loads Cordis projections from the persisted log. This module is the
//! GPUI equivalent of that projection for user-visible transcript rows.

use crate::state::{ChatEntry, EntryRole};
use serde_json::Value;

#[cfg(any(target_family = "wasm", test))]
pub(crate) mod fold_scroll;
#[cfg(any(target_family = "wasm", test))]
pub(crate) mod turn_process;
#[cfg(any(target_family = "wasm", test))]
pub(crate) mod turn_tails;

/// Visible transcript reconstructed from a session log.
#[derive(Clone, Debug, Default)]
pub struct HistoryProjection {
    pub entries: Vec<ChatEntry>,
    pub usage: Option<(u64, u64)>,
    /// Whole-log billed buckets (official StatsPills / tokenUsage).
    pub uncached_input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub decode_tokens: u64,
    pub decode_ms: f64,
    pub turns: u64,
    pub steps: u64,
    pub has_more: bool,
    pub oldest_seq: Option<u64>,
    pub has_active_goal: bool,
    /// Latest durable `session/title` from the log (official sidebar label).
    pub session_title: Option<(u64, String)>,
    /// Last committed human prompt, used by the workspace row's activity label.
    pub last_prompt_at_ms: Option<u64>,
}

/// Sources that should appear as ordinary user bubbles or turn triggers.
fn user_visible_source(kind: &str) -> Option<&'static str> {
    match kind {
        "user" => Some(""),
        "goal" => Some("goal"),
        "schedule" => Some("trigger:schedule"),
        "webhook" | "github" => Some("trigger:webhook"),
        "agent" | "agent-message" => Some("trigger:agent"),
        _ => None,
    }
}

fn content_texts(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block.get("type").and_then(Value::as_str) {
            Some("text") | Some("reasoning") => block.get("text").and_then(Value::as_str),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_title(name: &str, arguments: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(arguments) {
        if let Some(description) = value.get("description").and_then(Value::as_str) {
            if !description.is_empty() {
                return description.to_string();
            }
        }
        if let Some(command) = value.get("command").and_then(Value::as_str) {
            let short = command.lines().next().unwrap_or(command).trim();
            if !short.is_empty() {
                return format!("{name}: {short}");
            }
        }
        if let Some(path) = value
            .get("path")
            .or_else(|| value.get("filePath"))
            .or_else(|| value.get("file_path"))
            .and_then(Value::as_str)
        {
            return format!("{name}: {path}");
        }
    }
    name.to_string()
}

/// Fold session.v4 events into GPUI chat rows (newest-preserving order).
pub fn project_session_events(events: &[Value]) -> HistoryProjection {
    project_session_events_with_more(events, false)
}

/// Fold session.v4 events; `has_more` marks that older history still exists off-window.
pub fn project_session_events_with_more(events: &[Value], has_more: bool) -> HistoryProjection {
    let mut out = HistoryProjection {
        has_more,
        ..HistoryProjection::default()
    };
    let mut tool_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut seen_turns: std::collections::HashSet<u64> = std::collections::HashSet::new();

    for event in events {
        if let Some(seq) = event.get("seq").and_then(Value::as_u64) {
            out.oldest_seq = Some(match out.oldest_seq {
                Some(current) => current.min(seq),
                None => seq,
            });
        }
        let Some(kind) = event.get("type").and_then(Value::as_str) else {
            continue;
        };
        let at_ms = event.get("time").and_then(Value::as_f64);
        let data = event.get("data").cloned().unwrap_or(Value::Null);
        if matches!(kind, "tool/ptc-dispatch" | "tool/ptc-dispatch-start") {
            crate::tool_calls::apply_dispatch(&mut out.entries, event);
        }
        if kind != "tool/result"
            && let Some(reply) =
                crate::questions::reply_entry(event, event.get("questionArguments"))
        {
            out.entries.push(reply);
        }
        match kind {
            "user/message" => {
                if data.pointer("/source/kind").and_then(Value::as_str) == Some("user")
                    && let Some(time) = event["time"].as_u64()
                {
                    out.last_prompt_at_ms = out.last_prompt_at_ms.max(Some(time));
                }
                let source = data
                    .pointer("/source/kind")
                    .and_then(Value::as_str)
                    .unwrap_or("user");
                let Some(title) = user_visible_source(source) else {
                    continue;
                };
                let text = data
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|blocks| content_texts(blocks))
                    .unwrap_or_default();
                if text.trim().is_empty() {
                    continue;
                }
                // Goal rounds wrap the objective; surface as turn trigger, not a giant bubble.
                let (title, text) = if title == "goal" || text.contains("<goal_round>") {
                    let objective = text
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
                                .to_string()
                        })
                        .filter(|value| !value.is_empty())
                        .unwrap_or(text);
                    ("goal".to_string(), objective)
                } else {
                    (title.to_string(), text)
                };
                let id = data.get("id").and_then(Value::as_str).map(str::to_string);
                let key = id
                    .clone()
                    .unwrap_or_else(|| format!("hist-user-{}", out.entries.len()));
                out.entries.push(ChatEntry {
                    key,
                    external_id: id,
                    role: EntryRole::User,
                    title,
                    text,
                    input: String::new(),
                    status: String::new(),
                    tool: None,
                    at_ms,
                });
            }
            "assistant/message" => {
                let message = data.get("message").unwrap_or(&data);
                let id = message
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(turn) = data.get("turn").and_then(Value::as_u64) {
                    seen_turns.insert(turn);
                }
                out.steps = out.steps.saturating_add(1);
                if let Some(stream) = data.get("stream").and_then(Value::as_array) {
                    let times: Vec<f64> = stream
                        .iter()
                        .filter_map(|chunk| chunk.get("time").and_then(Value::as_f64))
                        .collect();
                    if let (Some(first), Some(last)) = (times.first(), times.last()) {
                        let elapsed = (last - first).max(0.);
                        out.decode_ms += elapsed;
                    }
                }
                if let Some(usage) = data.get("usage") {
                    let input = usage
                        .get("inputTokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let output = usage
                        .get("outputTokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let cache_read = usage
                        .get("cacheReadTokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let cache_write = usage
                        .get("cacheWriteTokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    out.uncached_input_tokens = out.uncached_input_tokens.saturating_add(input);
                    out.cache_read_tokens = out.cache_read_tokens.saturating_add(cache_read);
                    out.cache_write_tokens = out.cache_write_tokens.saturating_add(cache_write);
                    out.output_tokens = out.output_tokens.saturating_add(output);
                    out.decode_tokens = out.decode_tokens.saturating_add(output);
                    let billed = out.uncached_input_tokens
                        + out.cache_read_tokens
                        + out.cache_write_tokens
                        + out.output_tokens;
                    let window = out
                        .usage
                        .map(|(_, size)| size)
                        .filter(|size| *size > billed)
                        .unwrap_or(billed.max(1));
                    out.usage = Some((billed, window));
                }
                let blocks = message
                    .get("content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut thought = String::new();
                let mut answer = String::new();
                for block in &blocks {
                    match block.get("type").and_then(Value::as_str) {
                        Some("reasoning") => {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                if !text.is_empty() {
                                    if !thought.is_empty() {
                                        thought.push('\n');
                                    }
                                    thought.push_str(text);
                                }
                            }
                        }
                        Some("text") => {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                if !text.is_empty() {
                                    if !answer.is_empty() {
                                        answer.push('\n');
                                    }
                                    answer.push_str(text);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if !thought.is_empty() {
                    let key = format!(
                        "hist-thought-{}",
                        id.as_deref().unwrap_or(&out.entries.len().to_string())
                    );
                    out.entries.push(ChatEntry {
                        key,
                        external_id: id.clone(),
                        role: EntryRole::Thought,
                        title: String::new(),
                        text: thought,
                        input: String::new(),
                        status: String::new(),
                        tool: None,
                        at_ms,
                    });
                }
                if !answer.is_empty() {
                    let key = format!(
                        "hist-assistant-{}",
                        id.as_deref().unwrap_or(&out.entries.len().to_string())
                    );
                    out.entries.push(ChatEntry {
                        key,
                        external_id: id,
                        role: EntryRole::Assistant,
                        title: String::new(),
                        text: answer,
                        input: String::new(),
                        status: String::new(),
                        tool: None,
                        at_ms,
                    });
                }
            }
            "tool/call" => {
                let call_id = data
                    .get("callId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if call_id.is_empty() {
                    continue;
                }
                let name = data
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                let arguments = data
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let title = tool_title(&name, &arguments);
                if let Some(&index) = tool_index.get(&call_id) {
                    let entry = &mut out.entries[index];
                    entry.tool = Some(crate::tool_calls::ToolCall::root(name));
                    entry.title = title;
                    entry.input = arguments;
                    entry.at_ms = at_ms.or(entry.at_ms);
                    continue;
                }
                tool_index.insert(call_id.clone(), out.entries.len());
                out.entries.push(ChatEntry {
                    key: format!("tool-{call_id}"),
                    external_id: Some(call_id),
                    role: EntryRole::Tool,
                    title,
                    text: String::new(),
                    input: arguments,
                    status: "in_progress".to_string(),
                    tool: Some(crate::tool_calls::ToolCall::root(name)),
                    at_ms,
                });
            }
            "tool/result" => {
                let call_id = data
                    .pointer("/message/toolCallId")
                    .or_else(|| data.pointer("/message/source/callId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if call_id.is_empty() {
                    continue;
                }
                let output = crate::tool_calls::result_text(&data["message"]);
                let status = crate::tool_calls::result_status(&data, &data["message"]);
                if let Some(&index) = tool_index.get(&call_id) {
                    let entry = &mut out.entries[index];
                    entry.text = output;
                    entry.status = status.to_owned();
                } else {
                    let arguments = event.get("questionArguments");
                    let tool = arguments
                        .map(|_| crate::tool_calls::ToolCall::root("ask_user_question".into()));
                    let title = tool
                        .as_ref()
                        .map_or("tool", |tool| tool.name.as_str())
                        .to_owned();
                    let input = arguments
                        .map(|args| {
                            args.as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| args.to_string())
                        })
                        .unwrap_or_default();
                    tool_index.insert(call_id.clone(), out.entries.len());
                    out.entries.push(ChatEntry {
                        key: format!("tool-{call_id}"),
                        external_id: Some(call_id),
                        role: EntryRole::Tool,
                        title,
                        text: output,
                        input,
                        status: status.to_owned(),
                        tool,
                        at_ms,
                    });
                }
                if let Some(reply) =
                    crate::questions::reply_entry(event, event.get("questionArguments"))
                {
                    out.entries.push(reply);
                }
            }
            "llm/retry" | "llm/retry-started" => {
                let retry_id = data["retryId"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| event["seq"].as_u64().map(|seq| format!("retry-{seq}")))
                    .unwrap_or_else(|| format!("hist-retry-{}", out.entries.len()));
                let retry = data.get("retry").and_then(Value::as_u64).unwrap_or(1);
                let maximum = data
                    .get("maxRetries")
                    .map(|value| match value {
                        Value::String(text) => text.clone(),
                        Value::Number(number) => number.to_string(),
                        _ => "5".to_string(),
                    })
                    .unwrap_or_else(|| "5".to_string());
                out.entries.push(ChatEntry {
                    key: format!("hist-retry-{retry_id}"),
                    external_id: Some(retry_id),
                    role: EntryRole::Retry,
                    title: format!("{retry}/{maximum}"),
                    text: String::new(),
                    input: String::new(),
                    status: if kind == "llm/retry-started" {
                        "started".to_string()
                    } else {
                        "scheduled".to_string()
                    },
                    tool: None,
                    at_ms,
                });
            }
            "turn/end" => {
                let reason = data
                    .pointer("/reason/kind")
                    .or_else(|| data.pointer("/reason/reason/kind"))
                    .and_then(Value::as_str)
                    .unwrap_or("completed");
                let text = match reason {
                    "aborted" | "interrupted" | "cancelled" | "user" => "已停止",
                    "max_tokens" => "输出达到 token 上限",
                    "error" => "本轮出错",
                    _ => "",
                };
                if !text.is_empty() {
                    out.entries.push(ChatEntry {
                        key: format!("hist-turn-end-{}", out.entries.len()),
                        external_id: data["turn"].as_u64().map(|turn| turn.to_string()),
                        role: EntryRole::System,
                        title: "turn-end".into(),
                        text: text.into(),
                        input: String::new(),
                        status: reason.to_string(),
                        tool: None,
                        at_ms,
                    });
                }
                if let Some(plans) = event["gpuiSubmittedPlans"].as_array() {
                    out.entries.extend(plans.iter().filter_map(|plan| {
                        serde_json::from_value(plan.clone())
                            .ok()
                            .map(ChatEntry::submitted_plan)
                    }));
                }
            }
            "session/title" => {
                if let Some(seq) = event["seq"].as_u64()
                    && let Some(title) = data
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|title| !title.is_empty())
                {
                    out.session_title = Some((seq, title.to_string()));
                }
            }
            "goal/change" => {
                let operation = data
                    .get("operation")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                out.has_active_goal =
                    matches!(operation, "create" | "update" | "resume" | "continue");
                if operation == "complete" || operation == "cancel" || operation == "clear" {
                    out.has_active_goal = false;
                }
            }
            "request/context" => {
                if let Some(window) = data
                    .get("contextWindow")
                    .and_then(Value::as_u64)
                    .filter(|value| *value > 0)
                {
                    let used = out.usage.map(|(used, _)| used).unwrap_or(0);
                    out.usage = Some((used.min(window), window));
                }
            }
            _ => {}
        }
    }

    out.turns = if seen_turns.is_empty() {
        out.entries
            .iter()
            .filter(|entry| entry.role == EntryRole::User && entry.title.is_empty())
            .count() as u64
    } else {
        seen_turns.len() as u64
    };

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn historical_question_replies_follow_question_order_and_keep_skips() {
        let questions = json!([
            {"id":"one","header":"Options","question":"Where?"},
            {"id":"two","question":"What else?"}
        ]);
        let answers = json!([{"id":"two","selected":[]},{"id":"one","selected":["Local (Recommended)"],"custom":"extra"}]);
        let result = json!({
            "type":"tool/result","surfaceOp":"append","time":10,
            "questionArguments":{"questions":questions},
            "data":{"message":{"source":{"callId":"ask"},"content":[{"type":"text","text":json!({"answers":answers}).to_string()}]}}
        });
        let late = json!({
            "type":"user/message","surfaceOp":"append","time":20,
            "data":{"id":"late","source":{"kind":"user-question-reply","callId":"other","outcome":"answered"},
            "content":[{"type":"text","text":json!({"questions":questions,"answers":answers}).to_string()}]}
        });
        let projected = project_session_events(&[result, late]);
        assert_eq!(projected.entries[0].tool_name(), "ask_user_question");
        let replies = projected
            .entries
            .iter()
            .filter(|entry| entry.role == EntryRole::QuestionReply);
        assert_eq!(replies.clone().count(), 2);
        for (index, entry) in replies.enumerate() {
            assert_eq!(entry.at_ms, Some((index + 1) as f64 * 10.));
            assert_eq!(
                entry.text,
                "Options — Where?\n回答：Local (Recommended), extra\n\nWhat else?\n已跳过"
            );
        }
    }

    #[test]
    fn projects_user_assistant_thought_and_tools() {
        let events = vec![
            json!({
                "type": "user/message",
                "time": 1.0,
                "data": {
                    "id": "u1",
                    "source": {"kind": "user"},
                    "content": [{"type": "text", "text": "hello"}]
                }
            }),
            json!({
                "type": "assistant/message",
                "time": 2.0,
                "data": {
                    "message": {
                        "id": "a1",
                        "content": [
                            {"type": "reasoning", "text": "think"},
                            {"type": "text", "text": "world"}
                        ]
                    },
                    "usage": {
                        "inputTokens": 10,
                        "outputTokens": 5,
                        "cacheReadTokens": 100,
                        "totalTokens": 115
                    }
                }
            }),
            json!({
                "type": "tool/call",
                "time": 3.0,
                "data": {
                    "callId": "c1",
                    "name": "bash",
                    "arguments": "{\"command\":\"pwd\",\"description\":\"Check cwd\"}"
                }
            }),
            json!({
                "type": "tool/result",
                "time": 4.0,
                "data": {
                    "message": {
                        "toolCallId": "c1",
                        "content": [{"type": "text", "text": "/tmp"}]
                    }
                }
            }),
            json!({
                "type": "user/message",
                "time": 5.0,
                "data": {
                    "source": {"kind": "goal"},
                    "content": [{"type": "text", "text": "<goal_round> Objective: \"fix the bug\"\nmore"}]
                }
            }),
        ];
        let projected = project_session_events(&events);
        assert_eq!(projected.entries.len(), 5);
        assert_eq!(projected.entries[0].role, EntryRole::User);
        assert_eq!(projected.entries[0].text, "hello");
        assert_eq!(projected.entries[1].role, EntryRole::Thought);
        assert_eq!(projected.entries[2].role, EntryRole::Assistant);
        assert_eq!(projected.entries[3].role, EntryRole::Tool);
        assert_eq!(projected.entries[3].title, "Check cwd");
        assert_eq!(projected.entries[3].text, "/tmp");
        assert_eq!(projected.entries[4].title, "goal");
        assert!(projected.entries[4].text.contains("fix the bug"));
        assert_eq!(projected.uncached_input_tokens, 10);
        assert_eq!(projected.cache_read_tokens, 100);
        assert_eq!(projected.output_tokens, 5);
        assert_eq!(projected.decode_tokens, 5);
        assert_eq!(projected.steps, 1);
        assert_eq!(projected.usage, Some((10 + 100 + 5, 10 + 100 + 5)));
    }

    #[test]
    fn projects_latest_session_title() {
        let events = vec![
            json!({"type":"session/title","seq":1,"data":{"title":"临时标题"}}),
            json!({"type":"session/title","seq":2,"data":{"title":"会话进度确认"}}),
        ];
        let projected = project_session_events(&events);
        assert_eq!(projected.session_title, Some((2, "会话进度确认".into())));
    }

    #[test]
    fn skips_system_injected_user_messages() {
        let events = vec![json!({
            "type": "user/message",
            "data": {
                "source": {"kind": "time-context"},
                "content": [{"type": "text", "text": "now"}]
            }
        })];
        assert!(project_session_events(&events).entries.is_empty());
    }
}
