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

/// Only an authorized DeepSeek account may unlock the workspace.
pub fn account_signed_in(auth: Option<&Value>) -> bool {
    auth.is_some_and(|status| {
        status.pointer("/account/status").and_then(Value::as_str) == Some("credential-stored")
            || status
                .pointer("/account/attempt/phase")
                .and_then(Value::as_str)
                == Some("succeeded")
    })
}

/// Reject a delayed response that would move one sign-in attempt backwards.
pub fn is_stale_account(current: &Value, incoming: &Value) -> bool {
    let phase_rank = |value: &Value| match value.get("phase").and_then(Value::as_str) {
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
        && incoming_attempt
            .is_some_and(|attempt| phase_rank(attempt) < current_attempt.map_or(-1, phase_rank))
}

/// Visual role for one conversation row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryRole {
    User,
    Assistant,
    Thought,
    Tool,
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
        }
    }
}

/// State produced by an ACP update in addition to conversation rows.
#[derive(Default)]
pub struct UpdateEffect {
    pub usage: Option<(u64, u64)>,
    pub config_options: Option<Vec<Value>>,
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
        Some("tool_call") => {
            let id = string_field(update, "toolCallId");
            let title = string_field(update, "title");
            let input = update.get("rawInput").map(pretty_value).unwrap_or_default();
            let status = string_field(update, "status");
            if let Some(entry) = entries.iter_mut().find(|entry| {
                entry.role == EntryRole::Tool && entry.external_id.as_deref() == Some(id.as_str())
            }) {
                entry.title = title;
                entry.input = input;
                entry.status = status;
            } else {
                entries.push(ChatEntry {
                    key: format!("tool-{id}"),
                    external_id: Some(id),
                    role: EntryRole::Tool,
                    title,
                    text: String::new(),
                    input,
                    status,
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
                entry.role == EntryRole::Tool && entry.external_id.as_deref() == Some(id.as_str())
            }) {
                entry.text = output;
                entry.status = string_field(update, "status");
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
        Some("config_option_update") => {
            effect.config_options = update
                .get("configOptions")
                .and_then(Value::as_array)
                .cloned();
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
        EntryRole::System => "system",
        EntryRole::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::{EntryRole, account_signed_in, apply_update, is_stale_account, validate_api_key};
    use serde_json::json;

    #[test]
    fn api_key_alone_does_not_unlock_workspace() {
        let auth = json!({
            "account": {"status": "signed-out"},
            "apiKey": {"configured": true}
        });
        assert!(!account_signed_in(Some(&auth)));
    }

    #[test]
    fn stored_credential_unlocks_workspace() {
        let auth = json!({"account": {"status": "credential-stored"}});
        assert!(account_signed_in(Some(&auth)));
    }

    #[test]
    fn successful_attempt_unlocks_workspace() {
        let auth = json!({"account": {"attempt": {"phase": "succeeded"}}});
        assert!(account_signed_in(Some(&auth)));
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
    fn delayed_responses_cannot_reverse_one_sign_in_attempt() {
        let initializing = json!({"attempt": {"id": "attempt-1", "phase": "initializing"}});
        let waiting = json!({"attempt": {"id": "attempt-1", "phase": "waiting-browser"}});
        let succeeded = json!({"attempt": {"id": "attempt-1", "phase": "succeeded"}});
        assert!(!is_stale_account(&initializing, &waiting));
        assert!(!is_stale_account(&waiting, &succeeded));
        assert!(is_stale_account(&waiting, &initializing));
        assert!(is_stale_account(&succeeded, &initializing));
    }

    #[test]
    fn a_new_sign_in_attempt_is_not_stale() {
        let previous = json!({"attempt": {"id": "attempt-1", "phase": "succeeded"}});
        let next = json!({"attempt": {"id": "attempt-2", "phase": "initializing"}});
        assert!(!is_stale_account(&previous, &next));
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
}
