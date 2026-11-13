//! Load persisted DeepSeek Harness session.v4 JSONL logs for transcript hydrate.

pub(crate) mod export;
pub(crate) mod plan;
#[path = "../../shared/submitted_plan.rs"]
mod submitted_plan;

use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_EVENTS: usize = 2_500;

fn dsh_home() -> PathBuf {
    if let Ok(value) = std::env::var("DSH_HOME") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    dirs_home().join(".dsh")
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn encode_segment(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    if raw == "." {
        return "~002E".to_string();
    }
    if raw == ".." {
        return "~002E~002E".to_string();
    }
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let code = ch as u32;
        if ch != '~' && matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '-') {
            out.push(ch);
        } else {
            out.push('~');
            out.push_str(&format!("{code:04X}"));
        }
    }
    out
}

fn project_key(cwd: &str) -> String {
    let mut readable = String::new();
    let mut separator_run = false;
    for ch in cwd.chars() {
        let code = ch as u32;
        if ch == '/' || ch == '\\' || ch == ':' {
            if !separator_run {
                readable.push('-');
            }
            separator_run = true;
        } else if ch != '~' && matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '-') {
            readable.push(ch);
            separator_run = false;
        } else {
            readable.push('~');
            readable.push_str(&format!("{code:04X}"));
            separator_run = false;
        }
    }
    let slug = readable.trim_start_matches('-');
    let slug = if slug.is_empty() { "root" } else { slug };
    let bounded: String = slug.chars().take(251).collect();
    format!("--{bounded}--")
}

fn candidate_dirs(root: &Path, cwd: &str, session_id: &str) -> Vec<PathBuf> {
    let encoded_id = encode_segment(session_id);
    let literal_id = Path::new(session_id)
        .file_name()
        .filter(|name| *name == session_id);
    let mut dirs = Vec::new();
    if !cwd.is_empty() {
        dirs.push(root.join(project_key(cwd)).join(&encoded_id));
        // Session ids are often already filesystem-safe; try literal too.
        if encoded_id != session_id
            && let Some(literal) = literal_id
        {
            dirs.push(root.join(project_key(cwd)).join(literal));
        }
    }
    // Fallback: scan every project for this session id (cwd encoding can drift).
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let direct = path.join(&encoded_id);
            if direct.is_dir() {
                dirs.push(direct);
            }
            if let Some(literal) = literal_id.map(|name| path.join(name))
                && literal.is_dir()
            {
                dirs.push(literal);
            }
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs
}

fn find_log_path(dir: &Path) -> Option<PathBuf> {
    let preferred = [
        "session.v4.jsonl.zstd",
        "session.v4.jsonl",
        "session.jsonl.zstd",
        "session.jsonl",
    ];
    for name in preferred {
        let path = dir.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("session") && name.contains("jsonl"))
        })
        .collect();
    found.sort();
    found.pop()
}

fn decode_log_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = fs::read(path).map_err(|error| format!("读取会话日志失败：{error}"))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if name.ends_with(".zstd") || looks_like_zstd(&bytes) {
        let mut decoder = zstd::stream::read::Decoder::new(bytes.as_slice())
            .map_err(|error| format!("解压会话日志失败：{error}"))?;
        let mut plain = Vec::new();
        decoder
            .read_to_end(&mut plain)
            .map_err(|error| format!("解压会话日志失败：{error}"))?;
        Ok(plain)
    } else {
        Ok(bytes)
    }
}

fn looks_like_zstd(bytes: &[u8]) -> bool {
    // Zstandard magic number: 0xFD2FB528
    bytes.len() >= 4 && bytes[0] == 0x28 && bytes[1] == 0xB5 && bytes[2] == 0x2F && bytes[3] == 0xFD
}

fn parse_events(
    plain: &[u8],
    before_seq: Option<u64>,
    limit: usize,
) -> Result<(Vec<Value>, bool), String> {
    let text = String::from_utf8_lossy(plain);
    let mut events = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(value) => events.push(value),
            Err(_) => continue,
        }
    }
    plan::index_turn_plans(&mut events);
    index_question_results(&mut events);
    if let Some(before) = before_seq {
        events.retain(|event| {
            event
                .get("seq")
                .and_then(Value::as_u64)
                .is_none_or(|seq| seq < before)
        });
    }
    let has_more = events.len() > limit;
    if has_more {
        let skip = events.len() - limit;
        events = events.split_off(skip);
    }
    Ok((events, has_more))
}

/// Pair question answers with the call before cutting the log into UI pages.
fn index_question_results(events: &mut [Value]) {
    let mut calls = std::collections::HashMap::new();
    for event in events {
        let data = &event["data"];
        match event["type"].as_str() {
            Some("tool/call") if data["name"] == "ask_user_question" => {
                if let (Some(id), Some(arguments)) =
                    (data["callId"].as_str(), data.get("arguments"))
                {
                    calls.insert(id.to_owned(), arguments.clone());
                }
            }
            Some("tool/result") if event["surfaceOp"] == "append" => {
                if let Some(arguments) = data["message"]
                    .pointer("/source/callId")
                    .or_else(|| data["message"].get("toolCallId"))
                    .and_then(Value::as_str)
                    .and_then(|id| calls.remove(id))
                {
                    event["questionArguments"] = arguments;
                }
            }
            _ => {}
        }
    }
}

/// Resolve and decode the persisted session log into JSONL events.
pub fn load_events(
    session_id: &str,
    cwd: &str,
    before_seq: Option<u64>,
    limit: usize,
) -> Result<(Vec<Value>, bool), String> {
    if session_id.trim().is_empty() {
        return Err("缺少 sessionId".to_string());
    }
    let root = dsh_home().join("sessions");
    if !root.is_dir() {
        return Err(format!("未找到会话目录：{}", root.display()));
    }
    let mut last_error = format!("未找到会话日志：{session_id}");
    for dir in candidate_dirs(&root, cwd, session_id) {
        let Some(path) = find_log_path(&dir) else {
            continue;
        };
        match decode_log_bytes(&path).and_then(|bytes| parse_events(&bytes, before_seq, limit)) {
            Ok(result) => return Ok(result),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

#[tauri::command]
pub async fn load_session_history(
    session_id: String,
    cwd: String,
    before_seq: Option<u64>,
    limit: Option<u64>,
) -> Result<Value, String> {
    let window = limit
        .unwrap_or(MAX_EVENTS as u64)
        .clamp(50, MAX_EVENTS as u64) as usize;
    let stats_id = session_id.clone();
    let cwd_for_stats = cwd.clone();
    let (events, has_more) =
        tokio::task::spawn_blocking(move || load_events(&session_id, &cwd, before_seq, window))
            .await
            .map_err(|error| format!("读取历史失败：{error}"))??;
    let token_id = stats_id.clone();
    let session_stats = tokio::task::spawn_blocking(move || {
        peek_session_stats(&stats_id)
            .or_else(|| derive_session_stats_from_log(&stats_id, &cwd_for_stats))
    })
    .await
    .unwrap_or(None);
    let token_usage = tokio::task::spawn_blocking(move || peek_token_usage(&token_id))
        .await
        .unwrap_or(None);
    Ok(json!({
        "events": events,
        "hasMore": has_more,
        "sessionStats": session_stats,
        "tokenUsage": token_usage,
    }))
}

/// Official `sessionStats` projection rows used by ActivityPill dialogs.
fn peek_session_stats(session_id: &str) -> Option<Value> {
    peek_projcache_row(session_id, "sessionStats")
}

/// Official `tokenUsage` projection totals used by UsagePill dialogs.
fn peek_token_usage(session_id: &str) -> Option<Value> {
    let value = peek_projcache_row(session_id, "tokenUsage")?;
    // Prefer the durable whole-log `totals` object when present.
    if let Some(totals) = value.get("totals").filter(|value| value.is_object()) {
        return Some(totals.clone());
    }
    Some(value)
}

fn peek_projcache_row(session_id: &str, key: &str) -> Option<Value> {
    let path = dsh_home()
        .join("storages/session_projcache/sessions")
        .join(format!("{session_id}.json"));
    let bytes = fs::read(&path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value
        .pointer(&format!("/record/rows/{key}/val"))
        .cloned()
        .filter(|value| value.is_object())
}

/// Fallback fold mirroring `sessionStatsProjectionDefinition` when projcache is empty.
fn derive_session_stats_from_log(session_id: &str, cwd: &str) -> Option<Value> {
    let root = dsh_home().join("sessions");
    let mut plain = None;
    for dir in candidate_dirs(&root, cwd, session_id) {
        let Some(path) = find_log_path(&dir) else {
            continue;
        };
        if let Ok(bytes) = decode_log_bytes(&path) {
            plain = Some(bytes);
            break;
        }
    }
    let plain = plain?;
    let text = String::from_utf8_lossy(&plain);
    let mut turns: u64 = 0;
    let mut steps: u64 = 0;
    let mut llm_ms: f64 = 0.;
    let mut tool_ms: f64 = 0.;
    let mut ttft_ms: f64 = 0.;
    let mut ttft_steps: u64 = 0;
    let mut decode_ms: f64 = 0.;
    let mut decode_tokens: u64 = 0;
    let mut last_turn: Option<u64> = None;
    let mut open_step: Option<(u64, u64, f64, Option<f64>)> = None;
    let mut pending_calls: std::collections::HashMap<String, f64> =
        std::collections::HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(kind) = event.get("type").and_then(Value::as_str) else {
            continue;
        };
        let time = event.get("time").and_then(Value::as_f64).unwrap_or(0.);
        match kind {
            "step/start" => {
                let turn = event
                    .pointer("/data/turn")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let step = event
                    .pointer("/data/step")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                open_step = Some((turn, step, time, None));
            }
            "assistant/attempt" => {
                let Some((turn, step, start, first)) = open_step.as_mut() else {
                    continue;
                };
                let event_turn = event
                    .pointer("/data/turn")
                    .and_then(Value::as_u64)
                    .unwrap_or(*turn);
                let event_step = event
                    .pointer("/data/step")
                    .and_then(Value::as_u64)
                    .unwrap_or(*step);
                if event_turn != *turn || event_step != *step || first.is_some() {
                    continue;
                }
                if let Some(first_token) = first_token_time(event.pointer("/data/stream")) {
                    *first = Some(first_token);
                    let _ = start;
                }
            }
            "assistant/message" => {
                let Some((turn, step, start, first)) = open_step.take() else {
                    continue;
                };
                let event_turn = event
                    .pointer("/data/turn")
                    .and_then(Value::as_u64)
                    .unwrap_or(turn);
                let event_step = event
                    .pointer("/data/step")
                    .and_then(Value::as_u64)
                    .unwrap_or(step);
                if event_turn != turn || event_step != step {
                    open_step = Some((turn, step, start, first));
                    continue;
                }
                llm_ms += (time - start).max(0.);
                let first_token = first.or_else(|| first_token_time(event.pointer("/data/stream")));
                if let Some(first_token) = first_token {
                    ttft_ms += (first_token - start).max(0.);
                    ttft_steps += 1;
                    if let Some(output) = event
                        .pointer("/data/usage/outputTokens")
                        .and_then(Value::as_u64)
                        .or_else(|| {
                            event
                                .pointer("/data/usage/outputTokens")
                                .and_then(Value::as_f64)
                                .map(|v| v as u64)
                        })
                    {
                        decode_ms += (time - first_token).max(0.);
                        decode_tokens += output;
                    }
                }
            }
            "tool/call" => {
                if let Some(call_id) = event.pointer("/data/callId").and_then(Value::as_str) {
                    pending_calls.insert(call_id.to_string(), time);
                }
            }
            "tool/result" => {
                let call_id = event
                    .pointer("/data/message/source/callId")
                    .and_then(Value::as_str)
                    .or_else(|| event.pointer("/data/callId").and_then(Value::as_str));
                if let Some(call_id) = call_id {
                    if let Some(dispatched) = pending_calls.remove(call_id) {
                        tool_ms += (time - dispatched).max(0.);
                    }
                }
            }
            "step/end" => {
                let turn = event
                    .pointer("/data/turn")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if last_turn != Some(turn) {
                    turns += 1;
                    last_turn = Some(turn);
                }
                steps += 1;
                open_step = None;
            }
            "turn/end" => {
                pending_calls.clear();
            }
            _ => {}
        }
    }

    if turns == 0 && steps == 0 && llm_ms == 0. && tool_ms == 0. {
        return None;
    }
    Some(json!({
        "turns": turns,
        "steps": steps,
        "llmMs": llm_ms,
        "toolMs": tool_ms,
        "ttftMs": ttft_ms,
        "ttftSteps": ttft_steps,
        "decodeMs": decode_ms,
        "decodeTokens": decode_tokens,
    }))
}

/// Best-effort first visible token time from an assistant stream record array.
fn first_token_time(stream: Option<&Value>) -> Option<f64> {
    let records = stream?.as_array()?;
    for record in records {
        let kind = record
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("chunk");
        let time = record.get("time").and_then(Value::as_f64);
        match kind {
            "text-chunks" => {
                let texts = record.get("texts").and_then(Value::as_array)?;
                if texts.iter().any(|text| {
                    text.as_str()
                        .is_some_and(|value| value.chars().any(|ch| !ch.is_whitespace()))
                }) {
                    return time;
                }
            }
            _ => {
                let has_text = record
                    .pointer("/chunk/text")
                    .and_then(Value::as_str)
                    .or_else(|| record.get("text").and_then(Value::as_str))
                    .is_some_and(|value| value.chars().any(|ch| !ch.is_whitespace()));
                if has_text {
                    return time;
                }
            }
        }
    }
    None
}

/// Prefer the official projection cache title, then fall back to session.v4.
pub fn peek_title(session_id: &str, cwd: &str) -> Option<String> {
    if let Some(title) = peek_title_from_projcache(session_id) {
        return Some(title);
    }
    peek_title_from_log(session_id, cwd)
}

fn peek_title_from_projcache(session_id: &str) -> Option<String> {
    let path = dsh_home()
        .join("storages/session_projcache/sessions")
        .join(format!("{session_id}.json"));
    let bytes = fs::read(&path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value
        .pointer("/record/rows/title/val")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
}

/// Scan a session log for the latest `session/title` (official sidebar label),
/// then fall back to the first eligible user prompt (official session-title package).
fn peek_title_from_log(session_id: &str, cwd: &str) -> Option<String> {
    let root = dsh_home().join("sessions");
    for dir in candidate_dirs(&root, cwd, session_id) {
        let Some(path) = find_log_path(&dir) else {
            continue;
        };
        let Ok(bytes) = decode_log_bytes(&path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let mut title = None;
        let mut fallback = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(kind) = event.get("type").and_then(Value::as_str) else {
                continue;
            };
            if kind == "session/title" {
                if let Some(value) = event
                    .pointer("/data/title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    title = Some(value.to_string());
                }
                continue;
            }
            if fallback.is_some() || kind != "user/message" {
                continue;
            }
            if let Some(prompt) = first_user_prompt_text(&event) {
                fallback = Some(fallback_session_title(&prompt));
            }
        }
        if title.is_some() {
            return title;
        }
        if fallback.is_some() {
            return fallback;
        }
    }
    None
}

fn first_user_prompt_text(event: &Value) -> Option<String> {
    let blocks = event
        .pointer("/data/message/content")
        .or_else(|| event.pointer("/data/content"))
        .and_then(Value::as_array)?;
    let mut text = String::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let Some(chunk) = block.get("text").and_then(Value::as_str) else {
            continue;
        };
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(chunk);
    }
    (!text.is_empty()).then_some(text)
}

/// Official `fallbackSessionTitle`: leading words within a UTF-8 byte budget.
fn fallback_session_title(input: &str) -> String {
    const MAX_WORDS: usize = 8;
    const MAX_BYTES: usize = 96;
    let cleaned = input.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    let mut words = 0usize;
    for word in cleaned.split(' ').filter(|word| !word.is_empty()) {
        let candidate = if out.is_empty() {
            word.to_string()
        } else {
            format!("{out} {word}")
        };
        if candidate.len() > MAX_BYTES || words >= MAX_WORDS {
            break;
        }
        out = candidate;
        words += 1;
    }
    out
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTitleQuery {
    pub session_id: String,
    pub cwd: String,
}

/// Batch-enrich ACP session/list rows with durable titles from session.v4 logs.
#[tauri::command]
pub async fn peek_session_titles(sessions: Vec<SessionTitleQuery>) -> Result<Value, String> {
    let titles = tokio::task::spawn_blocking(move || {
        let mut out = serde_json::Map::new();
        for session in sessions {
            if let Some(title) = peek_title(&session.session_id, &session.cwd) {
                out.insert(session.session_id, Value::String(title));
            }
        }
        Value::Object(out)
    })
    .await
    .map_err(|error| format!("读取会话标题失败：{error}"))?;
    Ok(titles)
}

/// Merge durable local sessions (projcache) that ACP `session/list` may omit.
/// Per-turn token usage folded from the session log (official TurnUsagePanel).
#[derive(Default, Clone)]
struct TurnUsage {
    input_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    /// `provider/model` routes used by the turn, in first-seen order.
    routes: Vec<String>,
}

/// Fold `assistant/message` usage buckets and `request/context` routes per turn.
fn fold_turn_usage(events: &[serde_json::Value]) -> serde_json::Value {
    use std::collections::BTreeMap;
    let mut turns: BTreeMap<u64, TurnUsage> = BTreeMap::new();
    let mut current_turn: Option<u64> = None;
    for event in events {
        let kind = event
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let data = event
            .get("data")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        match kind {
            "turn/start" => {
                current_turn = data.get("turn").and_then(|value| value.as_u64());
            }
            "request/context" => {
                let Some(turn) = current_turn else { continue };
                let (Some(provider), Some(model)) = (
                    data.get("provider").and_then(|value| value.as_str()),
                    data.get("model").and_then(|value| value.as_str()),
                ) else {
                    continue;
                };
                let route = format!("{provider}/{model}");
                let usage = turns.entry(turn).or_default();
                if !usage.routes.iter().any(|seen| *seen == route) {
                    usage.routes.push(route);
                }
            }
            "assistant/message" => {
                let turn = data
                    .get("turn")
                    .and_then(|value| value.as_u64())
                    .or(current_turn);
                let Some(turn) = turn else { continue };
                let Some(usage) = data.get("usage") else {
                    continue;
                };
                let read = |key: &str| {
                    usage
                        .get(key)
                        .and_then(|value| {
                            value
                                .as_u64()
                                .or_else(|| value.as_f64().map(|value| value as u64))
                        })
                        .unwrap_or(0)
                };
                let input = read("inputTokens");
                let cache_read = read("cacheReadTokens");
                let cache_write = read("cacheWriteTokens");
                let output = read("outputTokens");
                let accumulated = turns.entry(turn).or_default();
                accumulated.input_tokens = accumulated.input_tokens.saturating_add(input);
                accumulated.cache_read_tokens =
                    accumulated.cache_read_tokens.saturating_add(cache_read);
                accumulated.cache_write_tokens =
                    accumulated.cache_write_tokens.saturating_add(cache_write);
                accumulated.output_tokens = accumulated.output_tokens.saturating_add(output);
                let total = usage
                    .get("totalTokens")
                    .and_then(|value| {
                        value
                            .as_u64()
                            .or_else(|| value.as_f64().map(|value| value as u64))
                    })
                    .unwrap_or(input + cache_read + cache_write + output);
                accumulated.total_tokens = accumulated.total_tokens.saturating_add(total);
            }
            _ => {}
        }
    }
    let turns: Vec<serde_json::Value> = turns
        .into_iter()
        .map(|(turn, usage)| {
            serde_json::json!({
                "turn": turn,
                "inputTokens": usage.input_tokens,
                "cacheReadTokens": usage.cache_read_tokens,
                "cacheWriteTokens": usage.cache_write_tokens,
                "outputTokens": usage.output_tokens,
                "totalTokens": usage.total_tokens,
                "routes": usage.routes,
            })
        })
        .collect();
    serde_json::json!({ "turns": turns })
}

/// Read one stored attachment as a data: URL for transcript images.
///
/// The Harness dedupes user images into `~/.dsh/attachments/v1/objects/<aa>/<sha>`
/// and logs only the reference, so the UI needs this to show the picture again
/// instead of the `[图片]` placeholder.
#[tauri::command]
pub async fn read_attachment(attachment_id: String, media_type: String) -> Result<Value, String> {
    let hash = attachment_id
        .strip_prefix("sha256:")
        .unwrap_or(&attachment_id)
        .trim()
        .to_owned();
    if hash.len() < 3 || !hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!("无效的附件 id：{attachment_id}"));
    }
    let prefix = &hash[..2];
    let path = dsh_home()
        .join("attachments/v1/objects")
        .join(prefix)
        .join(&hash);
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(&path))
        .await
        .map_err(|error| format!("读取附件失败：{error}"))?
        .map_err(|error| format!("读取附件失败：{error}"))?;
    let media_type = if media_type.trim().is_empty() {
        "image/png".to_string()
    } else {
        media_type
    };
    let encoded = base64_encode(&bytes);
    Ok(json!({
        "attachmentId": attachment_id,
        "dataUrl": format!("data:{media_type};base64,{encoded}"),
    }))
}

/// Read an absolute workspace image path for a Markdown picture.
///
/// Official `MarkdownPathImages` rewrites only absolute POSIX / drive paths to
/// the Host file API; this is the native equivalent, capped so a stray huge file
/// cannot be pulled into the transcript.
#[tauri::command]
pub async fn read_image_file(path: String) -> Result<Value, String> {
    let trimmed = path.trim().to_owned();
    if trimmed.is_empty() {
        return Err("缺少图片路径".to_string());
    }
    let file = PathBuf::from(&trimmed);
    if !file.is_absolute() {
        return Err(format!("图片路径必须是绝对路径：{trimmed}"));
    }
    let media_type = image_media_type(&file);
    if media_type.is_none() {
        return Err(format!("不是支持的图片格式：{trimmed}"));
    }
    let read_path = file.clone();
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(&read_path))
        .await
        .map_err(|error| format!("读取图片失败：{error}"))?
        .map_err(|error| format!("读取图片失败：{error}"))?;
    const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!("图片过大（{} 字节）", bytes.len()));
    }
    let media_type = media_type.unwrap_or("image/png");
    Ok(json!({
        "path": trimmed,
        "mediaType": media_type,
        "dataUrl": format!("data:{media_type};base64,{}", base64_encode(&bytes)),
    }))
}

fn image_media_type(path: &std::path::Path) -> Option<&'static str> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("webp") => Some("image/webp"),
        Some("gif") => Some("image/gif"),
        _ => None,
    }
}

/// Per-turn token usage for the chat's 本轮用量 panels.
#[tauri::command]
pub async fn turn_token_usage(session_id: String, cwd: String) -> Result<Value, String> {
    if session_id.trim().is_empty() {
        return Err("缺少 sessionId".to_string());
    }
    tokio::task::spawn_blocking(move || {
        let root = dsh_home().join("sessions");
        let mut last_error = format!("未找到会话日志：{session_id}");
        for dir in candidate_dirs(&root, &cwd, &session_id) {
            let Some(path) = find_log_path(&dir) else {
                continue;
            };
            let plain = match decode_log_bytes(&path) {
                Ok(plain) => plain,
                Err(error) => {
                    last_error = error;
                    continue;
                }
            };
            let events: Vec<Value> = plain
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
                .collect();
            return Ok(fold_turn_usage(&events));
        }
        Err(last_error)
    })
    .await
    .map_err(|error| format!("读取会话用量失败：{error}"))?
}

/// Minimal standard base64 encoder (no padding-free variant needed here).
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(b2 & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[tauri::command]
pub async fn list_local_sessions() -> Result<Value, String> {
    let sessions = tokio::task::spawn_blocking(scan_local_sessions)
        .await
        .map_err(|error| format!("扫描本地会话失败：{error}"))?;
    Ok(json!({ "sessions": sessions }))
}

fn scan_local_sessions() -> Vec<Value> {
    let root = dsh_home().join("storages/session_projcache/sessions");
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
            continue;
        };
        // Official ids are `session-<uuid>`; some local caches also keep bare uuids.
        let looks_like_session = stem.starts_with("session-")
            || (stem.len() == 36 && stem.chars().filter(|ch| *ch == '-').count() == 4);
        if !looks_like_session {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let cwd = value
            .pointer("/record/rows/workingDirectory/val")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|cwd| !cwd.is_empty())
            .unwrap_or("");
        // Skip empty / filesystem-root placeholders; they are not real workspaces.
        if cwd.is_empty() || cwd == "/" {
            continue;
        }
        let title = value
            .pointer("/record/rows/title/val")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string);
        let created_at = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| {
                time.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_millis() as u64)
            });
        let mut row = json!({
            "sessionId": stem,
            "cwd": cwd,
        });
        if let Some(title) = title {
            row["title"] = Value::String(title);
        }
        if let Some(created_at) = created_at {
            row["createdAt"] = json!(created_at);
        }
        sessions.push(row);
    }
    sessions.sort_by(|left, right| {
        let left_ms = left.get("createdAt").and_then(Value::as_u64).unwrap_or(0);
        let right_ms = right.get("createdAt").and_then(Value::as_u64).unwrap_or(0);
        right_ms.cmp(&left_ms)
    });
    sessions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_result_retains_call_arguments_across_history_pages() {
        let arguments = r#"{"questions":[{"id":"one","question":"Where?"}]}"#;
        let rows = [
            json!({"seq":1,"type":"tool/call","data":{"name":"ask_user_question","callId":"ask","arguments":arguments}}),
            json!({"seq":2,"type":"tool/result","surfaceOp":"append","data":{"message":{"source":{"callId":"ask"}}}}),
        ];
        let log = rows.map(|event| event.to_string()).join("\n");
        let (page, has_more) = parse_events(log.as_bytes(), None, 1).unwrap();
        assert!(has_more);
        assert_eq!(page[0]["seq"], 2);
        assert_eq!(page[0]["questionArguments"], arguments);
    }

    #[test]
    fn project_key_matches_known_workspace() {
        assert_eq!(
            project_key("/Users/leon/Documents/闲聊/dsh-gpui"),
            "--Users-leon-Documents-~95F2~804A-dsh-gpui--"
        );
        assert_eq!(
            project_key("/Users/leon/Documents/Kense ui"),
            "--Users-leon-Documents-Kense~0020ui--"
        );
    }

    #[test]
    fn encode_segment_escapes_unsafe() {
        assert_eq!(encode_segment("a b"), "a~0020b");
        assert_eq!(encode_segment("session-1"), "session-1");
    }
}

#[cfg(test)]
mod peek_tests {
    use super::*;

    #[test]
    fn peeks_kense_session_title() {
        let title = peek_title(
            "session-8e9a33c9-c664-46db-90d7-45b46bb09c2a",
            "/Users/leon/Documents/Kense ui",
        );
        assert_eq!(title.as_deref(), Some("会话进度确认"), "got {title:?}");
    }
}

#[cfg(test)]
mod local_scan_tests {
    use super::*;
    #[test]
    fn scan_includes_kense_titled_session() {
        let sessions = scan_local_sessions();
        let hit = sessions.iter().any(|session| {
            session.get("sessionId").and_then(Value::as_str)
                == Some("session-8e9a33c9-c664-46db-90d7-45b46bb09c2a")
                && session.get("title").and_then(Value::as_str) == Some("会话进度确认")
        });
        assert!(
            hit,
            "expected Kense titled session in local scan: {sessions:?}"
        );
    }
}

#[cfg(test)]
mod turn_usage_tests {
    use super::fold_turn_usage;

    #[test]
    fn folds_per_turn_buckets_and_routes() {
        let events: Vec<serde_json::Value> = vec![
            serde_json::json!({"type":"turn/start","seq":1,"data":{"turn":1}}),
            serde_json::json!({"type":"request/context","seq":2,"data":{"provider":"deepseek-official","model":"deepseek-v4.1-flash"}}),
            serde_json::json!({"type":"assistant/message","seq":3,"data":{"turn":1,"usage":{
                "inputTokens":100,"outputTokens":50,"cacheReadTokens":20,"cacheWriteTokens":5,"totalTokens":175}}}),
            serde_json::json!({"type":"assistant/message","seq":4,"data":{"turn":1,"usage":{
                "inputTokens":10,"outputTokens":5,"cacheReadTokens":2,"cacheWriteTokens":0,"totalTokens":17}}}),
            serde_json::json!({"type":"turn/start","seq":5,"data":{"turn":2}}),
            serde_json::json!({"type":"request/context","seq":6,"data":{"provider":"other","model":"m2"}}),
            serde_json::json!({"type":"assistant/message","seq":7,"data":{"turn":2,"usage":{
                "inputTokens":1,"outputTokens":2,"cacheReadTokens":3,"cacheWriteTokens":4}}}),
        ];
        let folded = fold_turn_usage(&events);
        let turns = folded["turns"].as_array().expect("turns array");
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0]["turn"], 1);
        assert_eq!(turns[0]["inputTokens"], 110);
        assert_eq!(turns[0]["cacheReadTokens"], 22);
        assert_eq!(turns[0]["cacheWriteTokens"], 5);
        assert_eq!(turns[0]["outputTokens"], 55);
        assert_eq!(turns[0]["totalTokens"], 192);
        assert_eq!(
            turns[0]["routes"],
            serde_json::json!(["deepseek-official/deepseek-v4.1-flash"])
        );
        // A message without totalTokens falls back to the bucket sum.
        assert_eq!(turns[1]["totalTokens"], 10);
        assert_eq!(turns[1]["routes"], serde_json::json!(["other/m2"]));
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::{base64_encode, image_media_type};
    use std::path::Path;

    #[test]
    fn image_media_types_follow_the_extension() {
        assert_eq!(image_media_type(Path::new("/tmp/a.png")), Some("image/png"));
        assert_eq!(
            image_media_type(Path::new("/tmp/a.JPG")),
            Some("image/jpeg")
        );
        assert_eq!(
            image_media_type(Path::new("/tmp/a.webp")),
            Some("image/webp")
        );
        assert_eq!(image_media_type(Path::new("/tmp/a.gif")), Some("image/gif"));
        assert_eq!(image_media_type(Path::new("/tmp/a.txt")), None);
        assert_eq!(image_media_type(Path::new("/tmp/a")), None);
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(&[0xff, 0x00, 0x10]), "/wAQ");
    }
}
