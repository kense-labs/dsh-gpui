//! Composer `@` token parsing and workspace-file presentation shared by the WASM UI and tests.

use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceFile {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) is_directory: bool,
    #[cfg(target_family = "wasm")]
    #[serde(default)]
    pub(crate) is_other: bool,
}

#[derive(Default)]
pub(crate) struct FileListing {
    pub(crate) directory: String,
    pub(crate) entries: Vec<WorkspaceFile>,
    pub(crate) error: Option<String>,
    pub(crate) truncated: bool,
    request: Option<u64>,
    loaded: bool,
}

impl FileListing {
    pub(crate) fn loading(&self) -> bool {
        self.request.is_some()
    }

    #[cfg(target_family = "wasm")]
    pub(crate) fn loaded(&self) -> bool {
        self.loaded
    }

    #[cfg(target_family = "wasm")]
    pub(crate) fn cancel(&mut self) {
        self.request = None;
    }

    pub(crate) fn start(&mut self, directory: String, request: u64) {
        if directory != self.directory {
            self.entries.clear();
            self.truncated = false;
            self.loaded = false;
        }
        self.directory = directory;
        self.request = Some(request);
        self.error = None;
    }

    pub(crate) fn finish(
        &mut self,
        request: u64,
        reply: Result<serde_json::Value, String>,
    ) -> bool {
        if self.request != Some(request) {
            return false;
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Reply {
            request_id: u64,
            directory: String,
            entries: Vec<WorkspaceFile>,
            truncated: bool,
        }

        self.request = None;
        match reply.and_then(|reply| {
            serde_json::from_value::<Reply>(reply).map_err(|error| error.to_string())
        }) {
            Ok(reply) if reply.request_id == request => {
                self.directory = reply.directory;
                self.entries = reply.entries;
                self.truncated = reply.truncated;
                self.loaded = true;
            }
            Ok(_) => self.error = Some("文件列表请求编号不匹配".into()),
            Err(error) => self.error = Some(error),
        }
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileToken {
    pub(crate) span: Range<usize>,
    pub(crate) directory: String,
    pub(crate) filter: String,
}

pub(crate) fn active_file_token(text: &str, caret_utf16: usize) -> Option<FileToken> {
    let caret = utf16_byte_index(text, caret_utf16);
    let prefix = text.get(..caret)?;
    let start = prefix.rfind('@')?;
    if start > 0 {
        let previous = prefix[..start].chars().next_back()?;
        if previous.is_alphanumeric() || previous == '_' {
            return None;
        }
    }
    let raw = &prefix[start + 1..];
    let query = if let Some(quoted) = raw.strip_prefix('"') {
        if quoted.contains('"') {
            return None;
        }
        quoted
    } else {
        if raw.chars().any(char::is_whitespace) {
            return None;
        }
        raw
    };
    let (directory, filter) = split_query(query);
    Some(FileToken {
        span: start..caret,
        directory,
        filter,
    })
}

pub(crate) fn utf16_byte_index(text: &str, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    let mut utf16 = 0;
    for (index, character) in text.char_indices() {
        if utf16 == offset {
            return index;
        }
        utf16 += character.len_utf16();
        if utf16 > offset {
            return index;
        }
    }
    text.len()
}

pub(crate) fn split_query(query: &str) -> (String, String) {
    if let Some(directory) = query.strip_suffix('/') {
        return (directory.trim_start_matches('/').to_string(), String::new());
    }
    query
        .rsplit_once('/')
        .map(|(directory, filter)| {
            (
                directory.trim_start_matches('/').to_string(),
                filter.to_string(),
            )
        })
        .unwrap_or_else(|| (String::new(), query.to_string()))
}

pub(crate) fn file_mention(path: &str, directory: bool) -> Option<String> {
    if path.is_empty() || path.contains(['\n', '\r', '"']) {
        return None;
    }
    let mut path = path.trim_matches('/').to_string();
    if directory && !path.ends_with('/') {
        path.push('/');
    }
    if path.chars().any(char::is_whitespace) {
        Some(format!("@\"{path}\""))
    } else {
        Some(format!("@{path}"))
    }
}

pub(crate) fn replace_token(text: &str, token: &FileToken, mention: &str) -> Option<String> {
    let before = text.get(..token.span.start)?;
    let after = text.get(token.span.end..)?;
    Some(format!("{before}{mention}{after}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_file_failures_and_successes_cannot_replace_a_newer_directory() {
        let mut listing = FileListing::default();
        listing.start("old".into(), 1);
        listing.start("new".into(), 2);
        assert!(!listing.finish(1, Err("old request failed".into())));
        assert!(listing.loading());
        assert!(listing.error.is_none());
        assert!(!listing.finish(
            1,
            Ok(serde_json::json!({"requestId": 1, "directory": "old"}))
        ));
        assert!(listing.finish(
            2,
            Ok(serde_json::json!({
                "requestId": 2, "directory": "new", "truncated": false,
                "entries": [{"name": "note.txt", "path": "new/note.txt", "isDirectory": false}],
            }))
        ));
        assert!(!listing.loading());
        assert_eq!(listing.directory, "new");
        assert_eq!(listing.entries[0].path, "new/note.txt");
    }

    #[test]
    fn detects_only_live_reference_tokens() {
        let token = active_file_token("查看 @apps/ast", 12).unwrap();
        assert_eq!(token.directory, "apps");
        assert_eq!(token.filter, "ast");
        assert!(active_file_token("mail@example.com", 15).is_none());
        assert!(active_file_token("@src closed ", 12).is_none());
    }

    #[test]
    fn drilled_and_unicode_tokens_replace_the_exact_span() {
        let text = "你好 @apps/aster/";
        let caret = text.chars().map(char::len_utf16).sum();
        let token = active_file_token(text, caret).unwrap();
        assert_eq!(token.directory, "apps/aster");
        assert_eq!(token.filter, "");
        assert_eq!(
            replace_token(text, &token, "@apps/aster/src/").unwrap(),
            "你好 @apps/aster/src/"
        );
    }

    #[test]
    fn mentions_quote_spaces_and_mark_directories() {
        assert_eq!(file_mention("src/main.rs", false).unwrap(), "@src/main.rs");
        assert_eq!(file_mention("my docs", true).unwrap(), "@\"my docs/\"");
        assert!(file_mention("bad\"name", false).is_none());
    }
}
