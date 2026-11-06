//! Composer file-reference state and workspace-directory navigation.

use super::*;

impl DshApp {
    pub(super) fn sync_file_trigger(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.suppress_file_trigger {
            return;
        }
        let (text, caret) = {
            let editor = self.composer.read(cx);
            (editor.value().to_string(), editor.selection().end)
        };
        if let Some(token) = active_file_token(&text, caret) {
            let reload = self.composer_menu != Some(ComposerMenu::Files)
                || self.file_directory != token.directory;
            self.file_filter = token.filter.clone();
            self.file_token = Some(token.clone());
            self.file_selected = 0;
            self.file_show_crumbs = false;
            self.composer_menu = Some(ComposerMenu::Files);
            self.show_workspace_menu = false;
            if reload {
                self.load_workspace_files(token.directory);
            }
        } else if self.composer_menu == Some(ComposerMenu::Files) {
            self.composer_menu = None;
            self.file_token = None;
        }
        cx.notify();
        let _ = w;
    }

    pub(super) fn open_file_picker(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let (text, selection) = {
            let editor = self.composer.read(cx);
            (editor.value().to_string(), editor.selection())
        };
        let token = active_file_token(&text, selection.end).unwrap_or_else(|| {
            let start = utf16_byte_index(&text, selection.start);
            let end = utf16_byte_index(&text, selection.end);
            let before = &text[..start];
            let after = &text[end..];
            let separator = before
                .chars()
                .next_back()
                .is_some_and(|character| character.is_alphanumeric() || character == '_');
            let prefix = if separator { " @" } else { "@" };
            let token_start = before.len() + usize::from(separator);
            let next = format!("{before}{prefix}{after}");
            let token = FileToken {
                span: token_start..token_start + 1,
                directory: String::new(),
                filter: String::new(),
            };
            self.replace_composer_without_trigger(&next, w, cx);
            token
        });
        self.file_filter = token.filter.clone();
        self.file_directory = token.directory.clone();
        self.file_token = Some(token.clone());
        self.file_selected = 0;
        self.file_show_crumbs = false;
        self.composer_menu = Some(ComposerMenu::Files);
        self.show_workspace_menu = false;
        self.load_workspace_files(token.directory);
        self.focus_composer(w, cx);
        cx.notify();
    }

    pub(super) fn visible_files(&self) -> Vec<WorkspaceFile> {
        let query = self.file_filter.to_lowercase();
        self.file_entries
            .iter()
            .filter(|entry| query.is_empty() || entry.name.to_lowercase().contains(&query))
            .cloned()
            .collect()
    }

    pub(super) fn insert_file_reference(
        &mut self,
        path: String,
        directory: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mention) = file_mention(&path, directory) else {
            self.file_error = Some("该文件名无法作为引用插入".to_string());
            cx.notify();
            return;
        };
        if let Some(token) = self.file_token.clone()
            && let Some(next) = replace_token(
                self.composer.read(cx).value(),
                &token,
                &format!("{mention} "),
            )
        {
            self.replace_composer_without_trigger(&next, w, cx);
        }
        self.composer_menu = None;
        self.file_token = None;
        self.file_error = None;
        self.focus_composer(w, cx);
        cx.notify();
    }

    pub(super) fn drill_file_directory(
        &mut self,
        directory: String,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mention = if directory.is_empty() {
            "@".to_string()
        } else {
            let Some(mention) = file_mention(&directory, true) else {
                self.file_error = Some("该目录名无法作为引用使用".to_string());
                cx.notify();
                return;
            };
            mention
        };
        if let Some(token) = self.file_token.clone() {
            let text = self.composer.read(cx).value();
            let caret = text[..token.span.start]
                .chars()
                .map(char::len_utf16)
                .sum::<usize>()
                + mention.chars().map(char::len_utf16).sum::<usize>();
            if let Some(next) = replace_token(text, &token, &mention) {
                self.replace_composer_without_trigger(&next, w, cx);
                self.file_token = active_file_token(&next, caret);
            }
        }
        self.file_filter.clear();
        self.file_selected = 0;
        self.file_show_crumbs = !directory.is_empty();
        self.composer_menu = Some(ComposerMenu::Files);
        self.load_workspace_files(directory);
        self.focus_composer(w, cx);
        cx.notify();
    }

    pub(super) fn apply_workspace_files(&mut self, reply: &Value) {
        if reply.get("requestId").and_then(Value::as_u64) != Some(self.file_request_id) {
            return;
        }
        self.file_loading = false;
        self.file_error = None;
        self.file_directory = reply
            .get("directory")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.file_entries = reply
            .get("entries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                Some(WorkspaceFile {
                    name: entry.get("name")?.as_str()?.to_string(),
                    path: entry.get("path")?.as_str()?.to_string(),
                    is_directory: entry.get("isDirectory")?.as_bool()?,
                })
            })
            .collect();
        self.file_truncated = reply
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.file_selected = self
            .file_selected
            .min(self.visible_files().len().saturating_sub(1));
    }

    pub(super) fn fail_workspace_files(&mut self, error: String) {
        self.file_loading = false;
        self.file_error = Some(error);
    }

    pub(super) fn handle_file_key(
        &mut self,
        key: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.composer_menu != Some(ComposerMenu::Files) {
            return false;
        }
        let files = self.visible_files();
        match key {
            "Escape" => {
                self.composer_menu = None;
                self.file_token = None;
            }
            "ArrowDown" if !files.is_empty() => {
                self.file_selected = (self.file_selected + 1).min(files.len() - 1);
            }
            "ArrowUp" if !files.is_empty() => {
                self.file_selected = self.file_selected.saturating_sub(1);
            }
            "Tab" => {
                let Some(file) = files
                    .get(self.file_selected)
                    .filter(|file| file.is_directory)
                else {
                    return true;
                };
                self.drill_file_directory(file.path.clone(), w, cx);
            }
            "Enter" => {
                let Some(file) = files.get(self.file_selected) else {
                    return true;
                };
                self.insert_file_reference(file.path.clone(), file.is_directory, w, cx);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    fn load_workspace_files(&mut self, directory: String) {
        self.file_request_id = self.file_request_id.wrapping_add(1);
        self.file_directory = directory.clone();
        self.file_loading = true;
        self.file_error = None;
        invoke(
            "list_workspace_files",
            json!({
                "workspace": self.workspace,
                "directory": directory,
                "requestId": self.file_request_id,
            }),
        );
    }

    fn replace_composer_without_trigger(&mut self, text: &str, w: &Window, cx: &mut Context<Self>) {
        self.suppress_file_trigger = true;
        replace(&self.composer, text, w, cx);
        self.suppress_file_trigger = false;
    }

    fn focus_composer(&self, w: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |editor, cx| editor.focus(w, cx));
    }
}
