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
    /// Direct children this session spawned, in discovery order.
    pub subagents: Vec<crate::subagents::SubagentRow>,
}

/// Sources that should appear as ordinary user bubbles or turn triggers.
fn user_visible_source(kind: &str) -> Option<&'static str> {
    match kind {
        "user" => Some(""),
        "goal" => Some("goal"),
        "schedule" => Some("trigger:schedule"),
        "webhook" => Some("trigger:webhook"),
        "github" => Some("trigger:github"),
        "agent" | "agent-message" => Some("trigger:agent"),
        "team" | "team-message" => Some("trigger:team"),
        "subagent" | "subagent-settled" => Some("trigger:subagent-settled"),
        "job" | "tool-jobs" => Some("trigger:tool-jobs"),
        "tool-goal" => Some("trigger:tool-goal"),
        "plugin" => Some("trigger:plugin"),
        _ => None,
    }
}

fn content_texts(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block.get("type").and_then(Value::as_str) {
            Some("text") | Some("reasoning") => block.get("text").and_then(Value::as_str),
            // Media blocks carry no text, but dropping them silently erased a
            // prompt that was only an image when the session was reopened.
            Some("image") => Some("[图片]"),
            Some("file") => Some("[文件]"),
            Some("audio") => Some("[音频]"),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_title(name: &str, arguments: &str) -> String {
    // A replayed todo row must read the same as a live one, or reopening a
    // session silently downgrades it to the bare tool name.
    if name == "todo_write"
        && let Some(summary) = crate::process::todo_summary(arguments)
    {
        return crate::process::todo_summary_text(&summary);
    }
    // Official PresentRow: the delivery call is titled 交付文件, never the raw
    // tool name, so the process tree reads like the official one.
    if name == "present" {
        return "交付文件".to_owned();
    }
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
    // Files announced through `present` during the open turn; they become the
    // official turn-tail delivery cards when that turn closes.
    let mut pending_deliverables: Vec<Value> = Vec::new();
    // Compaction summaries arrive just before the checkpoint message that
    // replaces the shadowed history; keep them keyed so the marker can cite the
    // official counts without rendering an extra row.
    let mut compaction_summaries: std::collections::HashMap<
        String,
        (Option<String>, Option<usize>, Option<u64>),
    > = std::collections::HashMap::new();

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
            // Files a closing turn delivered through `present`. The official
            // DeliverablesTail renders them as file cards at the turn tail, so the
            // durable projection keeps the validated list instead of dropping it.
            "deliverables/presented" => {
                if let Some(files) = data.get("files").and_then(Value::as_array) {
                    pending_deliverables.extend(files.iter().cloned());
                }
            }
            // A compaction summary is evidence for the marker, not a row of its
            // own: the official client folds it into the checkpoint marker.
            "compaction/summary" => {
                if let Some(compaction_id) = data.get("compactionId").and_then(Value::as_str) {
                    let text = data
                        .get("summary")
                        .and_then(Value::as_array)
                        .map(|blocks| content_texts(blocks))
                        .filter(|text| !text.trim().is_empty());
                    let items = data
                        .get("shadowedSeqs")
                        .and_then(Value::as_array)
                        .map(Vec::len);
                    let tokens = data.get("shadowedTokenCount").and_then(Value::as_u64);
                    compaction_summaries.insert(compaction_id.to_owned(), (text, items, tokens));
                }
            }
            // Tool-registry notices: a developer message whose content blocks add
            // or remove tools. The official transcript shows the
            // ContextInjectionRow tool variant, so a reopened session must too.
            "developer/message" => {
                let source = data
                    .pointer("/message/source/kind")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if source == "tool-registry" {
                    let cells: Vec<Value> = data
                        .pointer("/message/content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|block| {
                            let kind = block.get("type").and_then(Value::as_str)?;
                            let added = match kind {
                                "tool-addition" => true,
                                "tool-removal" => false,
                                _ => return None,
                            };
                            Some(serde_json::json!({
                                "added": added,
                                "name": block.get("toolName").cloned().unwrap_or(Value::Null),
                            }))
                        })
                        .collect();
                    let changes = crate::state::tool_changes(&Value::Array(cells));
                    if !changes.is_empty() {
                        let id = data
                            .pointer("/message/id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("hist-tool-change-{}", out.entries.len()));
                        out.entries
                            .push(crate::state::tool_change_entry(&id, &changes, at_ms));
                    }
                }
            }
            "user/message" => {
                // The compaction checkpoint message is hidden model context; the
                // official transcript replaces it with an expandable marker that
                // cites the shadowed counts from compaction/summary.
                if data.pointer("/source/kind").and_then(Value::as_str)
                    == Some("compact-checkpoint")
                    && let Some(compaction_id) =
                        data.pointer("/source/compactionId").and_then(Value::as_str)
                {
                    let (summary, items, tokens) = compaction_summaries
                        .get(compaction_id)
                        .cloned()
                        .unwrap_or((None, None, None));
                    out.entries.push(crate::state::compaction_entry(
                        compaction_id,
                        summary.as_deref(),
                        items,
                        tokens,
                        at_ms,
                    ));
                    continue;
                }
                if data.pointer("/source/kind").and_then(Value::as_str) == Some("user")
                    && let Some(time) = event["time"].as_u64()
                {
                    out.last_prompt_at_ms = out.last_prompt_at_ms.max(Some(time));
                }
                let source = data
                    .pointer("/source/kind")
                    .and_then(Value::as_str)
                    .unwrap_or("user");
                // Context injections (official ContextInjectionRow) are logged
                // A non-human message either wakes a turn (official
                // TurnTriggerNodeView) or is an injection into model history
                // (official ContextInjectionRow). Both are durable, so a reopened
                // session must rebuild the same rows the live stream showed.
                let blocks_text = data
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|blocks| content_texts(blocks))
                    .unwrap_or_default();
                if source != "user" && source != "user-question-reply" {
                    if user_visible_source(source).is_some() {
                        // Fall through to the trigger row below.
                    } else {
                        let id = data.get("id").and_then(Value::as_str).map(str::to_string);
                        let body = {
                            let catalog = crate::state::context_entries_text(&data);
                            if catalog.is_empty() {
                                blocks_text.clone()
                            } else {
                                catalog
                            }
                        };
                        if !body.trim().is_empty() {
                            out.entries.push(ChatEntry {
                                key: id.clone().unwrap_or_else(|| {
                                    format!("hist-context-{}", out.entries.len())
                                }),
                                external_id: id,
                                role: EntryRole::Context,
                                title: crate::state::context_source_label(source).to_owned(),
                                text: body,
                                input: String::new(),
                                status: if data.pointer("/source/update")
                                    == Some(&Value::Bool(true))
                                {
                                    "update".to_owned()
                                } else {
                                    String::new()
                                },
                                tool: None,
                                at_ms,
                            });
                        }
                        continue;
                    }
                }
                let Some(title) = user_visible_source(source) else {
                    continue;
                };
                // Attached images are rendered as thumbnails in the user bubble
                // (official UserContent), so they travel in the row's `input`
                // instead of the `[图片]` placeholder text.
                let images: Vec<crate::state::UserImage> = data
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|block| {
                        if block.get("type").and_then(Value::as_str) != Some("image") {
                            return None;
                        }
                        let attachment = block.get("attachment")?;
                        Some(crate::state::UserImage {
                            attachment_id: attachment.get("attachmentId")?.as_str()?.to_owned(),
                            media_type: attachment
                                .get("mediaType")
                                .and_then(Value::as_str)
                                .unwrap_or("image/png")
                                .to_owned(),
                            name: attachment
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            width: attachment.get("width").and_then(Value::as_u64),
                            height: attachment.get("height").and_then(Value::as_u64),
                            data_url: None,
                        })
                    })
                    .collect();
                let text = if images.is_empty() {
                    blocks_text
                } else {
                    let text_blocks: Vec<Value> = data
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                        .cloned()
                        .collect();
                    content_texts(&text_blocks)
                };
                if text.trim().is_empty() && images.is_empty() {
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
                // Variant-selecting facts for a trigger card, exactly like the
                // live projection reads them (official turnTriggerDetails).
                let detail = match source {
                    "subagent" | "subagent-settled" => data
                        .pointer("/source/stopReason")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    "webhook" => data
                        .pointer("/source/provider")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    _ => "",
                };
                out.entries.push(ChatEntry {
                    key,
                    external_id: id,
                    role: EntryRole::User,
                    title,
                    text,
                    input: if images.is_empty() {
                        String::new()
                    } else {
                        crate::state::user_images_json(&images)
                    },
                    status: detail.to_owned(),
                    tool: None,
                    at_ms,
                });
            }
            "system/message" => {
                // Official SystemPromptRow: the complete model-visible prompt as
                // one collapsed disclosure at its position in the turn.
                let text = data
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .map(|blocks| content_texts(blocks))
                    .unwrap_or_default();
                if text.trim().is_empty() {
                    continue;
                }
                let seq = event.get("seq").and_then(Value::as_u64);
                out.entries.push(ChatEntry {
                    key: seq
                        .map(|seq| format!("system-prompt-{seq}"))
                        .unwrap_or_else(|| format!("hist-system-prompt-{}", out.entries.len())),
                    external_id: seq.map(|seq| seq.to_string()),
                    role: EntryRole::SystemPrompt,
                    title: if event.get("surfaceOp") == Some(&Value::from("append"))
                        || event.get("surfaceOp").is_none()
                    {
                        "系统提示词".to_owned()
                    } else {
                        "系统提示词更新".to_owned()
                    },
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
            // Background jobs settle in the same durable log: a reopened session
            // rebuilds the row rather than losing the record of what ran.
            "job/registered" | "job/settled" => {
                let settled = kind == "job/settled";
                let Some(id) = data.get("jobId").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(entry) = out.entries.iter_mut().find(|entry| {
                    entry.role == EntryRole::Job && entry.external_id.as_deref() == Some(id)
                }) {
                    if settled {
                        entry.status = crate::state::job_status_label(&data).to_owned();
                        entry.text = crate::state::job_detail_text(&data);
                        entry.input = crate::state::job_finished_at(&data);
                    }
                } else {
                    out.entries.push(ChatEntry {
                        key: format!("hist-job-{id}"),
                        external_id: Some(id.to_owned()),
                        role: EntryRole::Job,
                        title: data
                            .get("label")
                            .and_then(Value::as_str)
                            .filter(|label| !label.is_empty())
                            .or_else(|| data.get("kind").and_then(Value::as_str))
                            .unwrap_or("")
                            .to_owned(),
                        text: crate::state::job_detail_text(&data),
                        input: if settled {
                            crate::state::job_finished_at(&data)
                        } else {
                            String::new()
                        },
                        status: if settled {
                            crate::state::job_status_label(&data).to_owned()
                        } else {
                            "running".to_owned()
                        },
                        tool: None,
                        at_ms: data
                            .get("startedAt")
                            .and_then(Value::as_f64)
                            .or_else(|| event.get("time").and_then(Value::as_f64)),
                    });
                }
            }
            // Slash-command results are log-only events; replay rebuilds the
            // command card so a reopened session keeps showing it.
            "command/run" | "command/done" => {
                let run = kind == "command/run";
                let Some(id) = data.get("commandId").and_then(Value::as_str) else {
                    continue;
                };
                let name = data
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if let Some(entry) = out.entries.iter_mut().find(|entry| {
                    entry.role == EntryRole::Command && entry.external_id.as_deref() == Some(id)
                }) {
                    if run {
                        if !name.is_empty() {
                            entry.title = name;
                        }
                        if let Some(args) = data.get("args").and_then(Value::as_str) {
                            entry.input = args.to_owned();
                        }
                    } else {
                        entry.status = data
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or("success")
                            .to_owned();
                        if let Some(text) = data.get("text").and_then(Value::as_str) {
                            entry.text = text.to_owned();
                        }
                    }
                } else if run || !name.is_empty() {
                    out.entries.push(ChatEntry {
                        key: format!("hist-command-{id}"),
                        external_id: Some(id.to_owned()),
                        role: EntryRole::Command,
                        title: name,
                        text: data
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        input: data
                            .get("args")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        status: if run {
                            String::new()
                        } else {
                            data.get("kind")
                                .and_then(Value::as_str)
                                .unwrap_or("success")
                                .to_owned()
                        },
                        tool: None,
                        at_ms,
                    });
                }
            }
            "turn/end" => {
                if !pending_deliverables.is_empty() {
                    let files = std::mem::take(&mut pending_deliverables);
                    out.entries
                        .push(crate::state::deliverable_entry(&files, at_ms));
                }
                let reason = data
                    .pointer("/reason/kind")
                    .or_else(|| data.pointer("/reason/reason/kind"))
                    .and_then(Value::as_str)
                    .unwrap_or("completed");
                // A recorded failure becomes the official TurnErrorItem row; the
                // other endings keep the compact turn-end chip.
                if let Some(turn) = data.get("turn").and_then(Value::as_u64) {
                    if let Some(entry) =
                        crate::state::turn_failure_entry(turn, &data["reason"], at_ms)
                    {
                        out.entries.push(entry);
                    } else if reason == "max_tokens" {
                        out.entries
                            .push(crate::state::turn_max_tokens_entry(turn, at_ms));
                    }
                }
                let text = match reason {
                    "aborted" | "interrupted" | "cancelled" | "user" => "已停止",
                    "error" => "",
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
            "subagent/catalog" => {
                if let Some(row) = crate::subagents::SubagentRow::from_event_data(&data) {
                    crate::subagents::upsert_subagent(&mut out.subagents, row);
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

    if !pending_deliverables.is_empty() {
        let files = std::mem::take(&mut pending_deliverables);
        out.entries
            .push(crate::state::deliverable_entry(&files, None));
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
    #[test]
    fn a_replayed_system_prompt_is_one_collapsed_row() {
        // Payload shape copied from a real ACP session log: the complete
        // model-visible prompt is recorded once at the turn's first step and the
        // official transcript shows it as a disclosure row.
        let events = vec![json!({
            "type": "system/message", "seq": 8, "time": 1_794_544_667_013u64,
            "data": {
                "turn": 1, "step": 1,
                "message": {"role": "system", "content": [
                    {"type": "text", "text": "You are an AI agent powered by DeepSeek Harness.\nBe careful."}
                ]}
            }
        })];
        let projected = project_session_events(&events);
        assert_eq!(projected.entries.len(), 1);
        assert_eq!(projected.entries[0].role, EntryRole::SystemPrompt);
        assert_eq!(projected.entries[0].title, "系统提示词");
        assert!(projected.entries[0].text.contains("DeepSeek Harness"));
        assert_eq!(projected.entries[0].key, "system-prompt-8");
    }

    #[test]
    fn presented_files_replay_as_the_official_delivery_cards() {
        let events = vec![
            json!({
                "type": "deliverables/presented", "seq": 1857, "time": 1_794_476_022_609u64,
                "data": {
                    "turn": 3, "callId": "call_00_present",
                    "files": [
                        {"description": "商品页设计规格", "path": "docs/popmart-pdp-design-spec.md"},
                        {"path": "src/core/product-detail.ts", "description": "商品页视图模型"},
                        {"path": ""},
                    ],
                },
            }),
            // Delivery cards belong to the closing turn tail, so the row is
            // stamped when the turn ends rather than at the tool result.
            json!({
                "type": "turn/end", "seq": 1864, "time": 1_794_476_091_002u64,
                "data": {"turn": 3, "reason": {"kind": "completed"}},
            }),
        ];
        let projected = project_session_events(&events);
        assert_eq!(projected.entries.len(), 1);
        let row = &projected.entries[0];
        assert_eq!(row.role, EntryRole::Deliverable);
        assert_eq!(row.title, "交付文件");
        let files: Vec<serde_json::Value> = serde_json::from_str(&row.input).unwrap();
        assert_eq!(files.len(), 2, "blank paths are dropped");
        assert_eq!(files[0]["path"], "docs/popmart-pdp-design-spec.md");
        assert_eq!(files[0]["description"], "商品页设计规格");
        assert_eq!(files[1]["path"], "src/core/product-detail.ts");
        assert_eq!(row.at_ms, Some(1_794_476_091_002f64));
    }

    #[test]
    fn a_compaction_checkpoint_becomes_the_official_marker() {
        // Shapes copied from a real log: the summary event carries the model
        // summary blocks plus the shadowed counts, the checkpoint message is the
        // replacement surface that must not render as a context row.
        let events = vec![
            json!({
                "type": "compaction/start", "seq": 7673, "time": 1_794_566_867_387u64,
                "data": {"compactionId": "aa96b315", "turn": 15},
            }),
            json!({
                "type": "compaction/summary", "seq": 7675, "time": 1_794_566_897_388u64,
                "data": {
                    "compactionId": "aa96b315",
                    "summary": [{"type": "text", "text": "## Primary Request\n- keep going"}],
                    "shadowedSeqs": [148, 401, 500],
                    "shadowedTokenCount": 12_491,
                },
            }),
            json!({
                "type": "user/message", "seq": 7676, "time": 1_794_566_897_389u64,
                "surfaceOp": {"op": "replace", "startSeq": 8, "endSeq": 7654},
                "data": {
                    "content": [{"type": "text", "text": "This is an automatically generated checkpoint"}],
                    "source": {"kind": "compact-checkpoint", "compactionId": "aa96b315"},
                },
            }),
            json!({
                "type": "compaction/end", "seq": 7677, "time": 1_794_566_897_390u64,
                "data": {"compactionId": "aa96b315", "turn": 15},
            }),
        ];
        let projected = project_session_events(&events);
        assert_eq!(projected.entries.len(), 1);
        let marker = &projected.entries[0];
        assert_eq!(marker.role, EntryRole::Compaction);
        assert_eq!(marker.title, "上下文已压缩");
        assert_eq!(marker.status, "已压缩 3 条历史记录（约 12491 tokens）");
        assert!(marker.text.contains("Primary Request"));
        assert_eq!(marker.at_ms, Some(1_794_566_897_389f64));
    }

    #[test]
    fn a_checkpoint_without_a_loaded_summary_still_marks_the_compaction() {
        let events = vec![json!({
            "type": "user/message", "seq": 9, "time": 1_700_000_000_000u64,
            "surfaceOp": {"op": "replace", "startSeq": 2, "endSeq": 8},
            "data": {
                "content": [{"type": "text", "text": "checkpoint"}],
                "source": {"kind": "compact-checkpoint", "compactionId": "missing-summary"},
            },
        })];
        let projected = project_session_events(&events);
        assert_eq!(projected.entries.len(), 1);
        let marker = &projected.entries[0];
        assert_eq!(marker.role, EntryRole::Compaction);
        assert_eq!(marker.status, "压缩摘要不可用");
        assert!(marker.text.is_empty());
    }

    #[test]
    fn a_tool_registry_notice_replays_as_the_official_tool_change_row() {
        let events = vec![json!({
            "type": "developer/message", "seq": 4351, "time": 1_794_486_299_033u64,
            "data": {
                "turn": 8, "step": 1,
                "message": {
                    "source": {"kind": "tool-registry"},
                    "content": [
                        {"type": "tool-addition", "toolName": "wait_agent"},
                        {"type": "tool-removal", "toolName": "interrupt_agent"},
                    ],
                    "role": "developer",
                    "id": "fcc67030-3a46-4e1d-9053-8ff17cb800b8",
                },
                "headerSeq": 4350,
            },
            "surfaceOp": "append",
        })];
        let projected = project_session_events(&events);
        let change = projected
            .entries
            .iter()
            .find(|entry| entry.role == EntryRole::ToolChange)
            .expect("the tool notice is a row");
        assert_eq!(change.title, "工具已更新");
        assert_eq!(change.status, "新增 1 个，移除 1 个");
        assert!(change.text.contains("新增：wait_agent"));
        assert!(change.text.contains("移除：interrupt_agent"));
        assert_eq!(
            change.external_id.as_deref(),
            Some("fcc67030-3a46-4e1d-9053-8ff17cb800b8")
        );
    }

    #[test]
    fn a_single_tool_change_names_the_tool_in_the_title() {
        let events = vec![json!({
            "type": "developer/message", "seq": 7, "time": 1_794_486_300_000u64,
            "data": {
                "turn": 1, "step": 1,
                "message": {
                    "source": {"kind": "tool-registry"},
                    "content": [{"type": "tool-addition", "toolName": "bash"}],
                    "role": "developer",
                    "id": "single-change",
                },
            },
        })];
        let projected = project_session_events(&events);
        let change = &projected.entries[0];
        assert_eq!(change.role, EntryRole::ToolChange);
        assert_eq!(change.title, "已添加工具：bash");
        assert!(change.status.is_empty());
        assert!(change.text.is_empty());
    }

    #[test]
    fn a_capped_turn_replays_the_official_max_tokens_notice() {
        // Official TurnMaxTokensItem: warning row with the cap title and the
        // "send continue" hint, not the compact stopped-style chip.
        let events = vec![json!({
            "type": "turn/end", "seq": 9, "time": 1_700_000_000_000u64,
            "data": {"turn": 7, "reason": {"kind": "max_tokens"}}
        })];
        let projected = project_session_events(&events);
        let notice = projected
            .entries
            .iter()
            .find(|entry| entry.role == EntryRole::Warning)
            .expect("a capped turn keeps a warning row");
        assert_eq!(notice.title, "已达到输出 token 上限");
        assert!(notice.text.contains("回答被截断"));
        assert!(notice.text.contains("继续"));
        assert_eq!(notice.at_ms, Some(1_700_000_000_000f64));
        // The old compact chip would have said 输出达到 token 上限; the notice
        // replaces it rather than doubling up.
        assert!(
            projected
                .entries
                .iter()
                .all(|entry| entry.role != EntryRole::System)
        );
    }

    #[test]
    fn a_replayed_subagent_notice_keeps_its_outcome() {
        // Payload copied from a real parent log: the durable source records the
        // child's stop reason, which selects the trigger card's title.
        let events = vec![json!({
            "type": "user/message", "seq": 20, "time": 30,
            "data": {
                "id": "settled-1",
                "source": {
                    "kind": "subagent-settled", "form": "notice",
                    "summary": "Background subagent agent-1 finished.",
                    "stopReason": "completed",
                    "senderSessionId": "73652a28-dab6-4fd7-8aa5-a6a6d99132a8",
                },
                "content": [{"type": "text", "text": "Background subagent agent-1 finished."}],
            }
        })];
        let projected = project_session_events(&events);
        assert_eq!(projected.entries.len(), 1);
        let entry = &projected.entries[0];
        assert_eq!(entry.role, EntryRole::User);
        assert_eq!(entry.title, "trigger:subagent-settled");
        assert_eq!(entry.status, "completed");
        assert_eq!(
            crate::state::trigger_title(&entry.title, &entry.status),
            "子智能体已完成任务"
        );
    }

    #[test]
    fn a_replayed_subagent_catalog_lists_the_children() {
        // Payload copied from a real parent session log: the parent records one
        // durable discovery fact per direct child, and reopening it must still
        // list them so the child transcript stays reachable.
        let events = vec![
            json!({"type":"subagent/catalog","seq":11,"time":1_794_454_553_991u64,
                "data":{"version":0,"childId":"3ee01e83-40f7-4abf-b1e4-03c4bc0d85ef",
                "childCreatedAt":1_794_454_553_990u64,"mode":"continuable","label":"研究 goal 官方 UI 呈现"}}),
            json!({"type":"subagent/catalog","seq":12,"time":1_794_454_553_992u64,
                "data":{"version":0,"childId":"670c17fd-6c94-4ac5-8e69-a0e6a62b0845",
                "childCreatedAt":1_794_454_553_992u64,"mode":"one-shot","label":"研究思考动画实现"}}),
        ];
        let projected = project_session_events(&events);
        assert_eq!(projected.subagents.len(), 2);
        assert_eq!(projected.subagents[0].label, "研究 goal 官方 UI 呈现");
        assert_eq!(
            projected.subagents[0].mode,
            crate::subagents::SubagentMode::Continuable
        );
        assert_eq!(
            projected.subagents[1].child_id,
            "670c17fd-6c94-4ac5-8e69-a0e6a62b0845"
        );
    }

    #[test]
    fn a_prompt_that_was_only_an_image_keeps_its_attachment() {
        // The durable image block carries no text; the bubble renders the picture
        // from its attachment reference, so the row must survive projection.
        let events = vec![json!({
            "type":"user/message","seq":3,"time":20,
            "data":{"id":"img-1","source":{"kind":"user"},
                "content":[{"type":"image","attachment":{"attachmentId":"a-1","mediaType":"image/png"}}]}
        })];
        let projected = project_session_events(&events);
        assert_eq!(projected.entries.len(), 1);
        let entry = &projected.entries[0];
        assert_eq!(entry.role, EntryRole::User);
        assert!(entry.text.is_empty());
        let images = crate::state::user_images(entry);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].attachment_id, "a-1");
        assert_eq!(images[0].media_type, "image/png");
        assert!(images[0].data_url.is_none());
        // A mixed prompt keeps its text and the picture.
        let events = vec![json!({
            "type":"user/message","seq":4,"time":21,
            "data":{"id":"img-2","source":{"kind":"user"},
                "content":[{"type":"text","text":"看看这个"},
                    {"type":"image","attachment":{"attachmentId":"a-2","mediaType":"image/webp"}}]}
        })];
        let projected = project_session_events(&events);
        assert_eq!(projected.entries[0].text, "看看这个");
        assert_eq!(crate::state::user_images(&projected.entries[0]).len(), 1);
        assert!(!projected.entries[0].input.contains("[图片]"));
    }

    fn a_replayed_todo_row_keeps_the_live_summary() {
        // A reopened session must show the same todo summary a live turn showed,
        // not the bare tool name.
        assert_eq!(
            tool_title(
                "todo_write",
                r#"{"todos": [
                    {"content": "扫描磁盘", "status": "completed"},
                    {"content": "检查缓存", "status": "in_progress"}
                ]}"#
            ),
            "1/2 已完成 · 检查缓存"
        );
        // Unusable args fall back to the tool name, as before.
        assert_eq!(tool_title("todo_write", "{\"todos\": ["), "todo_write");
        // Official PresentRow titles the delivery call 交付文件, not `present`.
        assert_eq!(
            tool_title("present", "{\"files\": [{\"path\": \"README.md\"}]}"),
            "交付文件"
        );
    }

    #[test]
    fn a_replayed_job_pair_rebuilds_one_row() {
        let projection = project_session_events(&[
            json!({
                "type": "job/registered", "seq": 10, "time": 1794453670976f64,
                "data": {
                    "jobId": "bash-09d5", "owner": "s1", "kind": "bash",
                    "label": "uname -a", "startedAt": 1794453670976f64
                }
            }),
            json!({
                "type": "job/settled", "seq": 12, "time": 1794453671009f64,
                "data": {
                    "jobId": "bash-09d5", "owner": "s1", "kind": "bash",
                    "label": "uname -a", "startedAt": 1794453670976f64,
                    "finishedAt": 1794453671009f64, "status": "completed",
                    "detail": "exit code: 127"
                }
            }),
        ]);
        let jobs: Vec<_> = projection
            .entries
            .iter()
            .filter(|entry| entry.role == EntryRole::Job)
            .collect();
        assert_eq!(jobs.len(), 1, "the pair must rebuild exactly one row");
        assert_eq!(jobs[0].title, "uname -a");
        assert_eq!(jobs[0].status, "completed");
        assert_eq!(jobs[0].text, "exit code: 127");
        // The duration needs the numeric finish time preserved.
        assert_eq!(jobs[0].input, "1794453671009");
    }

    #[test]
    fn replays_command_cards_from_log_only_events() {
        // Copied from a real `/goal create` session log so the projection stays
        // pinned to the payload the Host actually writes.
        let result = "Goal created\nStatus: active\nObjective: 帮我算出圆周率第100位\n\
                      Rounds: 0/256\nActivation: armed\n\n\
                      Commands: /goal edit <objective>, /goal pause, /goal clear";
        let projection = project_session_events(&[
            json!({
                "type": "command/run", "seq": 3, "time": 1794454152068.0,
                "data": {
                    "commandId": "cmd-3192330d-1", "name": "goal",
                    "args": " 帮我算出圆周率第100位", "source": {"kind": "user"}
                }
            }),
            json!({
                "type": "command/done", "seq": 5, "time": 1794454152070.0,
                "data": {
                    "commandId": "cmd-3192330d-1", "kind": "success",
                    "text": result
                }
            }),
        ]);
        assert_eq!(projection.entries.len(), 1);
        let entry = &projection.entries[0];
        assert_eq!(entry.role, EntryRole::Command);
        assert_eq!(entry.title, "goal");
        assert_eq!(entry.input, " 帮我算出圆周率第100位");
        assert_eq!(entry.status, "success");
        assert_eq!(entry.text, result);
        // The card collapses to the first line and expands because it is multiline.
        assert_eq!(entry.text.lines().next(), Some("Goal created"));
        assert!(entry.text.contains('\n'));
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
    fn replayed_context_injections_become_collapsible_rows() {
        // Live and replayed sessions must agree: an injection is a collapsed
        // context row, never a user bubble and never a turn trigger.
        let events = vec![
            json!({
                "type": "user/message", "seq": 4, "time": 10,
                "data": {
                    "id": "ctx-time",
                    "source": {"kind": "time-context", "form": "snapshot"},
                    "content": [{"type": "text", "text": "Time sampled while preparing turn 1, step 1"}]
                }
            }),
            json!({
                "type": "user/message", "seq": 5, "time": 11,
                "data": {
                    "id": "ctx-runtime",
                    "source": {"kind": "runtime-context", "form": "snapshot"},
                    "content": [{"type": "text", "text": "runtime facts"}]
                }
            }),
            json!({
                "type": "user/message", "seq": 6, "time": 12,
                "data": {
                    "id": "goal-round",
                    "source": {"kind": "goal"},
                    "content": [{"type": "text", "text": "<goal_round>\nObjective: \"把目标做完\"\nRound: 2/8"}]
                }
            }),
        ];
        let projected = project_session_events(&events);
        let contexts: Vec<_> = projected
            .entries
            .iter()
            .filter(|entry| entry.role == EntryRole::Context)
            .collect();
        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[0].title, "时间上下文");
        assert!(contexts[0].text.contains("Time sampled"));
        assert_eq!(contexts[1].title, "运行时上下文");
        // A goal round still wakes the turn and stays a trigger row.
        let trigger = projected
            .entries
            .iter()
            .find(|entry| entry.role == EntryRole::User)
            .expect("goal trigger row");
        assert_eq!(trigger.title, "goal");
        assert_eq!(trigger.text, "把目标做完");
    }
}
