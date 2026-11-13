//! File mentions in assistant prose (official `MarkdownFileMentions`).
//!
//! Official ChatView links inline-code tokens that name a file produced in the
//! closing turn: the vocabulary comes from the turn's successful `write`, `edit`
//! and `str_replace_editor` calls plus its delivered files, so a reply that says
//! `` `src/main.rs` `` opens that file in the sidebar instead of staying inert code.

use std::collections::HashMap;

use crate::state::{ChatEntry, EntryRole};

/// One produced file with both spellings the model may use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProducedFile {
    /// Path exactly as the tool call authored it.
    pub(crate) authored: String,
    /// Absolute path (relative authored paths are joined to the workspace).
    pub(crate) resolved: String,
}

/// The files one turn produced, ready to resolve inline-code tokens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FileMentionVocabulary {
    pub(crate) files: Vec<ProducedFile>,
}

impl FileMentionVocabulary {
    /// Resolve one authored inline-code token to a unique produced file path.
    ///
    /// Matches the authored spelling, the absolute path, or the file name; a token
    /// matching more than one produced file stays inert, exactly like the official
    /// resolver.
    pub(crate) fn resolve(&self, token: &str) -> Option<&str> {
        let token = token.trim();
        if token.is_empty() {
            return None;
        }
        let mut matches = self.files.iter().filter(|file| {
            file.authored == token || file.resolved == token || basename(&file.resolved) == token
        });
        let first = matches.next()?;
        if matches.any(|other| other.resolved != first.resolved) {
            return None;
        }
        Some(first.resolved.as_str())
    }
}

/// Final path segment of a `/`-separated path.
fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}

/// Join a relative authored path to the workspace; absolute paths stay as written.
fn resolve_path(workspace: &str, authored: &str) -> String {
    let absolute = authored.starts_with('/')
        || (authored.len() > 2
            && authored.as_bytes()[1] == b':'
            && matches!(authored.as_bytes()[2], b'\\' | b'/'));
    if absolute || workspace.trim().is_empty() {
        return authored.to_owned();
    }
    let workspace = workspace.trim_end_matches('/');
    let rest = authored.trim_start_matches("./");
    format!("{workspace}/{rest}")
}

/// Extract the mutation path from a supported first-party call, mirroring the
/// official rules (`write`, `edit`, `str_replace_editor` only).
pub(crate) fn mutation_path(name: &str, arguments: &str) -> Option<String> {
    let arguments: serde_json::Value = serde_json::from_str(arguments).ok()?;
    let path = arguments
        .get("file_path")
        .or_else(|| arguments.get("path"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())?;
    let non_empty = |key: &str| {
        arguments
            .get(key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| !text.is_empty())
    };
    match name {
        "write" => arguments
            .get("content")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|content| !content.is_empty())
            .then(|| path.to_owned()),
        "edit" => {
            let distinct = non_empty("old_string")
                && arguments
                    .get("new_string")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|new| new != arguments["old_string"].as_str().unwrap_or_default());
            let replace_all_ok = arguments
                .get("replace_all")
                .map(|value| value.is_boolean())
                .unwrap_or(true);
            (distinct && replace_all_ok).then(|| path.to_owned())
        }
        "str_replace_editor" => match arguments
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
        {
            "create" => non_empty("file_text").then(|| path.to_owned()),
            "str_replace" => non_empty("old_str").then(|| path.to_owned()),
            "insert" => arguments
                .get("insert_line")
                .and_then(serde_json::Value::as_i64)
                .filter(|line| *line >= 0)
                .filter(|_| non_empty("new_str"))
                .map(|_| path.to_owned()),
            _ => None,
        },
        _ => None,
    }
}

/// Per-assistant-row vocabularies for the rendered transcript.
///
/// Produced files accumulate across one turn (between user rows), deliveries
/// included; every assistant row of that turn resolves against the same set.
pub(crate) fn vocabularies<'a>(
    entries: impl IntoIterator<Item = &'a ChatEntry>,
    workspace: &str,
) -> HashMap<String, FileMentionVocabulary> {
    let mut produced: Vec<String> = Vec::new();
    let mut map: HashMap<String, FileMentionVocabulary> = HashMap::new();
    let push = |produced: &mut Vec<String>, path: String| {
        let path = path.trim().to_owned();
        if !path.is_empty() && !produced.iter().any(|seen| *seen == path) {
            produced.push(path);
        }
    };
    for entry in entries {
        match entry.role {
            EntryRole::User => {
                produced.clear();
                continue;
            }
            EntryRole::Tool => {
                let Some(tool) = entry.tool.as_ref().filter(|tool| tool.parent.is_none()) else {
                    continue;
                };
                if let Some(path) = mutation_path(&tool.name, &entry.input) {
                    push(&mut produced, path);
                }
                continue;
            }
            EntryRole::Deliverable => {
                if let Ok(serde_json::Value::Array(files)) =
                    serde_json::from_str::<serde_json::Value>(&entry.input)
                {
                    for file in files {
                        if let Some(path) = file.get("path").and_then(serde_json::Value::as_str) {
                            push(&mut produced, path.to_owned());
                        }
                    }
                }
                continue;
            }
            EntryRole::Assistant => {}
            _ => continue,
        }
        let files = produced
            .iter()
            .map(|authored| ProducedFile {
                authored: authored.clone(),
                resolved: resolve_path(workspace, authored),
            })
            .collect::<Vec<_>>();
        if !files.is_empty() {
            map.insert(entry.key.clone(), FileMentionVocabulary { files });
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(role: EntryRole, name: &str, arguments: serde_json::Value) -> ChatEntry {
        let mut entry = ChatEntry::local(role, name, "");
        if role == EntryRole::Tool {
            entry.tool = Some(crate::tool_calls::ToolCall::root(name.to_string()));
            entry.input = arguments.to_string();
        }
        entry
    }

    #[test]
    fn mutation_rules_match_the_official_supported_calls() {
        assert_eq!(
            mutation_path("write", r#"{"file_path":"a.rs","content":"x"}"#).as_deref(),
            Some("a.rs")
        );
        // write without content is not a completed mutation.
        assert_eq!(mutation_path("write", r#"{"file_path":"a.rs"}"#), None);
        assert_eq!(
            mutation_path(
                "edit",
                r#"{"file_path":"a.rs","old_string":"a","new_string":"b"}"#
            )
            .as_deref(),
            Some("a.rs")
        );
        assert_eq!(
            mutation_path(
                "edit",
                r#"{"file_path":"a.rs","old_string":"a","new_string":"a"}"#
            ),
            None
        );
        assert_eq!(
            mutation_path(
                "str_replace_editor",
                r#"{"path":"b.rs","command":"create","file_text":"x"}"#
            )
            .as_deref(),
            Some("b.rs")
        );
        assert_eq!(
            mutation_path(
                "str_replace_editor",
                r#"{"path":"b.rs","command":"view","file_text":"x"}"#
            ),
            None
        );
        // Unsupported tools (apply_patch, bash) never name a file.
        assert_eq!(
            mutation_path("apply_patch", r#"{"file_path":"a.rs","content":"x"}"#),
            None
        );
        assert_eq!(mutation_path("bash", r#"{"file_path":"a.rs"}"#), None);
    }

    #[test]
    fn vocabulary_resolves_authored_names_basenames_and_rejects_ambiguity() {
        let vocabulary = FileMentionVocabulary {
            files: vec![
                ProducedFile {
                    authored: "src/main.rs".into(),
                    resolved: "/w/src/main.rs".into(),
                },
                ProducedFile {
                    authored: "../shared/util.rs".into(),
                    resolved: "/w/../shared/util.rs".into(),
                },
            ],
        };
        assert_eq!(vocabulary.resolve("src/main.rs"), Some("/w/src/main.rs"));
        assert_eq!(vocabulary.resolve("main.rs"), Some("/w/src/main.rs"));
        assert_eq!(vocabulary.resolve("/w/src/main.rs"), Some("/w/src/main.rs"));
        assert_eq!(vocabulary.resolve("main.rs "), Some("/w/src/main.rs"));
        assert_eq!(vocabulary.resolve("other.rs"), None);
        // Two different files with the same final name keep the token inert.
        let ambiguous = FileMentionVocabulary {
            files: vec![
                ProducedFile {
                    authored: "a/dup.rs".into(),
                    resolved: "/w/a/dup.rs".into(),
                },
                ProducedFile {
                    authored: "b/dup.rs".into(),
                    resolved: "/w/b/dup.rs".into(),
                },
            ],
        };
        assert_eq!(ambiguous.resolve("dup.rs"), None);
        assert_eq!(ambiguous.resolve("a/dup.rs"), Some("/w/a/dup.rs"));
    }

    #[test]
    fn each_turn_gets_its_own_vocabulary_on_every_assistant_row() {
        let mut entries = vec![
            tool(EntryRole::User, "", serde_json::json!({})),
            tool(
                EntryRole::Tool,
                "write",
                serde_json::json!({"file_path":"one.rs","content":"x"}),
            ),
            ChatEntry::local(EntryRole::Assistant, "a1", "wrote it"),
            ChatEntry::local(EntryRole::Assistant, "a2", "done"),
            tool(EntryRole::User, "", serde_json::json!({})),
            tool(EntryRole::Assistant, "plain", serde_json::json!({})),
        ];
        entries[0].title = "user".into();
        entries[4].title = "user".into();
        let map = vocabularies(&entries, "/w");
        assert_eq!(map["a1"].resolve("one.rs"), Some("/w/one.rs"));
        assert_eq!(map["a2"].resolve("one.rs"), Some("/w/one.rs"));
        assert!(!map.contains_key("plain"));
        // The next turn starts with an empty vocabulary.
        assert!(
            !map.values()
                .any(|vocabulary| vocabulary.resolve("one.rs").is_some()
                    && vocabulary.files.len() > 1)
        );
    }
}
