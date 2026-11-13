//! Official rc.3 `ApprovalCommand`: the command line an approval card shows.
//!
//! The official approval card renders the command of the correlated Chat Tool
//! node below its heading: a shell `command` argument, or the text
//! `terminal_send` types into a terminal. The permission request itself may
//! carry `rawInput`; otherwise the call is resolved through the transcript
//! entry keyed by its `toolCallId`.

use crate::state::{ChatEntry, EntryRole};
use serde_json::Value;

/// Extract the command line for one permission request.
///
/// Returns `None` when the request is unrelated to a shell/terminal call, when
/// the arguments are missing or malformed, or when the command is empty.
pub(crate) fn approval_command(tool_call: &Value, entries: &[ChatEntry]) -> Option<String> {
    let call_id = tool_call.get("toolCallId").and_then(Value::as_str);
    let (name, arguments) = match raw_arguments(tool_call.get("rawInput")) {
        Some(arguments) => (
            tool_call
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            arguments,
        ),
        None => {
            let entry = call_id.and_then(|id| {
                entries.iter().find(|entry| {
                    entry.role == EntryRole::Tool && entry.external_id.as_deref() == Some(id)
                })
            })?;
            let arguments = serde_json::from_str::<Value>(&entry.input).ok()?;
            (entry.tool_name().to_owned(), arguments)
        }
    };
    let command = if name == "terminal_send" {
        arguments.get("text")
    } else {
        arguments.get("command")
    }?;
    let command = command.as_str()?;
    (!command.is_empty()).then(|| command.to_owned())
}

/// Official `approval.escalation` subject: the display name of the tool that
/// asks for privileged execution.
///
/// ACP permission requests may carry only the `toolCallId`, so the name comes
/// from the correlated transcript tool row when the request has no `title`.
pub(crate) fn approval_tool_label(tool_call: &Value, entries: &[ChatEntry]) -> Option<String> {
    if let Some(title) = tool_call
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        return Some(title.to_owned());
    }
    let call_id = tool_call.get("toolCallId").and_then(Value::as_str)?;
    let entry = entries.iter().find(|entry| {
        entry.role == EntryRole::Tool && entry.external_id.as_deref() == Some(call_id)
    })?;
    Some(tool_label(entry.tool_name()))
}

/// Localized name of one tool, as the escalation line reads it.
fn tool_label(name: &str) -> String {
    match name {
        "bash" | "sh" | "shell" | "command" | "run" | "terminal_send" | "terminal" => {
            "运行命令".to_owned()
        }
        "read" | "cat" => "读取文件".to_owned(),
        "write" | "edit" | "apply_patch" | "multi_edit" | "str_replace" => "编辑文件".to_owned(),
        "delete" | "remove" | "rm" => "删除文件".to_owned(),
        "grep" | "glob" | "search" | "find" | "rg" => "搜索文件".to_owned(),
        "web_fetch" | "web_search" | "fetch" => "访问网络".to_owned(),
        "present" => "交付文件".to_owned(),
        "todo_write" => "更新待办".to_owned(),
        other => other.to_owned(),
    }
}

/// Official decision-button label for one ACP permission option.
///
/// The ACP names are English (`Allow once`, `Reject`); the card shows the
/// official `approval.*` copy instead, keeping an unknown option's own name.
pub(crate) fn approval_option_label(option_id: &str, name: &str) -> String {
    let id = option_id.to_lowercase();
    if id.contains("reject") || id.contains("deny") || id.contains("cancel") {
        "拒绝".to_owned()
    } else if id.contains("always") {
        "始终允许".to_owned()
    } else if id.contains("once") {
        "允许一次".to_owned()
    } else if id.contains("allow") {
        "允许".to_owned()
    } else {
        name.to_owned()
    }
}

/// Read `rawInput` as an object, or as a JSON string holding one.
fn raw_arguments(raw: Option<&Value>) -> Option<Value> {
    match raw? {
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_object),
        Value::Object(_) => raw.cloned(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_entry(call_id: &str, name: &str, input: Value) -> ChatEntry {
        let mut entry = ChatEntry::local(EntryRole::Tool, format!("tool-{call_id}"), "");
        entry.external_id = Some(call_id.to_owned());
        entry.title = name.to_owned();
        entry.tool = Some(crate::tool_calls::ToolCall::root(name.to_owned()));
        entry.input = serde_json::to_string_pretty(&input).expect("input");
        entry
    }

    #[test]
    fn shell_commands_come_from_the_request_or_the_correlated_entry() {
        let inline = json!({
            "toolCallId": "call-1",
            "rawInput": {"command": "cargo test --workspace"}
        });
        assert_eq!(
            approval_command(&inline, &[]).as_deref(),
            Some("cargo test --workspace")
        );
        // rawInput may arrive as a JSON string.
        let encoded = json!({
            "toolCallId": "call-1",
            "rawInput": "{\"command\":\"git status\"}"
        });
        assert_eq!(
            approval_command(&encoded, &[]).as_deref(),
            Some("git status")
        );
        // Otherwise the transcript entry behind the toolCallId resolves it.
        let entries = vec![tool_entry("call-2", "bash", json!({"command": "ls -la"}))];
        let request = json!({"toolCallId": "call-2"});
        assert_eq!(
            approval_command(&request, &entries).as_deref(),
            Some("ls -la")
        );
    }

    #[test]
    fn terminal_send_shows_the_typed_text() {
        let entries = vec![tool_entry(
            "call-3",
            "terminal_send",
            json!({"terminalId": "t1", "text": "npm run dev\n"}),
        )];
        let request = json!({"toolCallId": "call-3"});
        assert_eq!(
            approval_command(&request, &entries).as_deref(),
            Some("npm run dev\n")
        );
        // A shell command argument is not what terminal_send types.
        let entries = vec![tool_entry(
            "call-4",
            "terminal_send",
            json!({"command": "ignored"}),
        )];
        assert_eq!(
            approval_command(&json!({"toolCallId": "call-4"}), &entries),
            None
        );
    }

    #[test]
    fn unrelated_or_malformed_requests_have_no_command() {
        let missing = json!({"toolCallId": "call-9"});
        assert_eq!(approval_command(&missing, &[]), None);
        let entries = vec![tool_entry("call-5", "read", json!({"path": "src/main.rs"}))];
        assert_eq!(
            approval_command(&json!({"toolCallId": "call-5"}), &entries),
            None
        );
        let empty = json!({"toolCallId": "call-6", "rawInput": {"command": ""}});
        assert_eq!(approval_command(&empty, &[]), None);
        let broken = json!({"toolCallId": "call-7", "rawInput": "{not json"});
        assert_eq!(approval_command(&broken, &[]), None);
    }

    #[test]
    fn tool_subject_and_decision_labels_are_localized() {
        // The request's own title wins when the Host sends one.
        let request = json!({"toolCallId": "call-8", "title": "自定义工具"});
        assert_eq!(
            approval_tool_label(&request, &[]).as_deref(),
            Some("自定义工具")
        );
        // Otherwise the correlated tool row names the tool.
        let entries = vec![tool_entry("call-9", "bash", json!({"command": "ls"}))];
        assert_eq!(
            approval_tool_label(&json!({"toolCallId": "call-9"}), &entries).as_deref(),
            Some("运行命令")
        );
        let entries = vec![tool_entry("call-10", "apply_patch", json!({}))];
        assert_eq!(
            approval_tool_label(&json!({"toolCallId": "call-10"}), &entries).as_deref(),
            Some("编辑文件")
        );
        assert_eq!(
            approval_tool_label(&json!({"toolCallId": "missing"}), &[]),
            None
        );
        assert_eq!(
            approval_option_label("allow-once", "Allow once"),
            "允许一次"
        );
        assert_eq!(approval_option_label("allow", "Allow once"), "允许");
        assert_eq!(
            approval_option_label("always-allow", "Allow always"),
            "始终允许"
        );
        assert_eq!(approval_option_label("reject-once", "Reject"), "拒绝");
        assert_eq!(approval_option_label("custom-option", "Keep"), "Keep");
    }
}
