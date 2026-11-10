//! Composer file-reference state and keyboard shortcuts.

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
                || self.mention_files.directory != token.directory;
            self.file_filter = token.filter.clone();
            self.file_token = Some(token.clone());
            self.file_selected = 0;
            self.file_show_crumbs = false;
            self.composer_menu = Some(ComposerMenu::Files);
            self.show_workspace_menu = false;
            if reload {
                self.load_workspace_files(token.directory, cx);
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
        self.file_token = Some(token.clone());
        self.file_selected = 0;
        self.file_show_crumbs = false;
        self.composer_menu = Some(ComposerMenu::Files);
        self.show_workspace_menu = false;
        self.load_workspace_files(token.directory, cx);
        self.focus_composer(w, cx);
        cx.notify();
    }

    pub(super) fn visible_files(&self) -> Vec<WorkspaceFile> {
        let query = self.file_filter.to_lowercase();
        self.mention_files
            .entries
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
            self.mention_files.error = Some("该文件名无法作为引用插入".to_string());
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
        self.mention_files.error = None;
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
                self.mention_files.error = Some("该目录名无法作为引用使用".to_string());
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
        self.load_workspace_files(directory, cx);
        self.focus_composer(w, cx);
        cx.notify();
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

    fn load_workspace_files(&mut self, directory: String, cx: &mut Context<Self>) {
        self.next_file_request = self.next_file_request.wrapping_add(1);
        let request = self.next_file_request;
        self.mention_files.start(directory.clone(), request);
        let args =
            json!({"workspace": self.workspace, "directory": directory, "requestId": request});
        cx.spawn(async move |this, cx| {
            let reply = bridge::invoke("list_workspace_files", args).await;
            let _ = this.update(cx, |this, cx| {
                if this.mention_files.finish(request, reply) {
                    this.file_selected = this
                        .file_selected
                        .min(this.visible_files().len().saturating_sub(1));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn replace_composer_without_trigger(
        &mut self,
        text: &str,
        w: &Window,
        cx: &mut Context<Self>,
    ) {
        self.suppress_file_trigger = true;
        replace(&self.composer, text, w, cx);
        self.suppress_file_trigger = false;
    }

    pub(super) fn focus_composer(&self, w: &mut Window, cx: &mut Context<Self>) {
        super::document_html::restore_shell_focus();
        self.composer.update(cx, |editor, cx| editor.focus(w, cx));
    }

    // —— “/ 调用指令”：行首 "/" 打开指令面板，输入即过滤，空格结束 token ——
    // 官方行为：↑↓ 在结果中移动选择，Enter 执行选中指令并清掉 "/…" 前缀，Esc 关闭。

    pub(super) fn sync_command_trigger(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().to_string();
        let token = text
            .strip_prefix('/')
            .filter(|rest| !rest.chars().any(char::is_whitespace));
        if let Some(filter) = token {
            self.command_filter = filter.to_string();
            if self.composer_menu != Some(ComposerMenu::Commands) {
                self.composer_menu = Some(ComposerMenu::Commands);
                self.command_selected = 0;
                self.show_workspace_menu = false;
                self.show_workspace_view_options = false;
            }
            self.command_from_slash = true;
        } else if self.command_from_slash && self.composer_menu == Some(ComposerMenu::Commands) {
            self.composer_menu = None;
            self.command_from_slash = false;
            self.command_filter.clear();
            self.command_selected = 0;
        }
        cx.notify();
    }

    pub(super) fn run_application_shortcut(
        &mut self,
        command: crate::shortcuts::Command,
        embedded: Option<(DockPaneId, RightPanelTab)>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use crate::shortcuts::Command;
        if self.session_feedback.visible() {
            if command == Command::Close {
                self.dismiss_session_feedback(w, cx);
            }
            return true;
        }
        if self.session_downloads.visible() {
            if command == Command::Close {
                self.dismiss_session_download(w, cx);
            }
            return true;
        }

        if command == Command::Shortcuts {
            if self.shortcuts.open {
                self.dismiss_shortcuts(w, cx);
            } else if self.confirm_permission.is_none()
                && self.workspace_rename.is_none()
                && self.workspace_delete.is_none()
                && self.session_rename.is_none()
            {
                self.open_shortcuts(w, cx);
            }
            return true;
        }
        if self.shortcuts.open {
            if command == Command::Close {
                self.dismiss_shortcuts(w, cx);
            }
            return true;
        }
        if self.show_settings {
            if command == Command::Close || command == Command::Settings {
                self.close_settings(cx);
            }
            return true;
        }
        if self.confirm_permission.is_some()
            || self.workspace_rename.is_some()
            || self.workspace_delete.is_some()
            || self.session_rename.is_some()
        {
            if command == Command::Close {
                if self.confirm_permission.is_some() {
                    self.close_permission_confirmation(w, cx);
                } else {
                    self.close_workspace_dialog(w, cx);
                }
                cx.notify();
            }
            return true;
        }
        if command == Command::Settings {
            super::document_html::restore_shell_focus();
            self.show_settings = true;
            cx.notify();
            return true;
        }
        if self.connected() {
            match command {
                Command::NewSession => {
                    self.new_session(w, cx);
                    return true;
                }
                Command::ToggleSidebar => {
                    self.set_left_sidebar_collapsed(!self.rail_active(w), w, cx);
                    return true;
                }
                Command::SearchSessions => {
                    super::document_html::restore_shell_focus();
                    self.open_workspace_search(w, cx);
                    return true;
                }
                Command::RenameSession => {
                    if self.page == WorkspacePage::Conversation
                        && let Some(id) = self.active_session.clone()
                        && self
                            .sessions
                            .iter()
                            .any(|session| session.id == id && !session.blank)
                    {
                        self.begin_session_rename(id, w, cx);
                    } else {
                        self.push_toast("请先选择一个会话", cx);
                    }
                    return true;
                }
                Command::ArchiveSession => {
                    if let Some(id) = &self.active_session {
                        self.archived_sessions.insert(id.clone());
                        self.save_workspace_preferences(cx);
                        cx.notify();
                    }
                    return true;
                }
                Command::ForkSession => {
                    if let Some(session) = self
                        .sessions
                        .iter()
                        .find(|session| self.active_session.as_deref() == Some(session.id.as_str()))
                    {
                        if session.blank {
                            self.push_toast("当前会话没有已结束的轮次", cx);
                        } else {
                            let id = session.id.clone();
                            self.fork_session(&id, ForkOrigin::Shortcut, cx);
                        }
                    } else {
                        self.push_toast("请先选择一个会话", cx);
                    }
                    return true;
                }
                _ => {}
            }
        }
        let focused_pane = embedded
            .map(|(pane, _)| pane)
            .or_else(|| self.focused_right_pane(w, cx));
        if command == Command::Close && focused_pane.is_none() {
            invoke(
                "close_shortcut_window",
                json!({"revision": self.shortcuts.revision()}),
            );
            return true;
        }
        if self.page != WorkspacePage::Conversation || self.show_welcome {
            return false;
        }
        let open_pane = focused_pane
            .filter(|pane| !pane.is_floating())
            .unwrap_or_else(|| self.sidebar.surface.layout.active_dock_pane());
        match command {
            Command::Terminal => self.new_terminal(open_pane, None, w, cx),
            Command::NewBrowser => self.new_browser_tab(open_pane, None, w, cx),
            Command::Files => {
                if let Some(pane) = focused_pane.filter(|pane| !pane.is_floating())
                    && self
                        .sidebar
                        .surface
                        .layout
                        .pane(pane)
                        .is_some_and(|pane| matches!(pane.active(), RightPanelTab::Document(_)))
                {
                    self.show_document_navigator(pane, w, cx);
                } else {
                    self.open_right_tab(open_pane, RightPanelTab::Files, w, cx);
                }
            }
            Command::OpenWorkspace => self.open_workspace_in(cx),
            Command::ToggleDock => self.toggle_right_panel_from(focused_pane, w, cx),
            _ => {
                let Some(pane) = focused_pane else {
                    return false;
                };
                let tab = embedded
                    .map(|(_, tab)| tab)
                    .or_else(|| self.focused_dock_tab(pane, w, cx));
                match command {
                    Command::Split => self.split_right_panel(pane, w, cx),
                    Command::Fullscreen if !pane.is_floating() => {
                        self.toggle_right_fullscreen(w, cx)
                    }
                    Command::Close => {
                        if let Some(tab) = tab {
                            self.close_right_tab(pane, tab, w, cx);
                        }
                    }
                    Command::Refresh => match tab {
                        Some(RightPanelTab::Browser(key)) => self.browser_reload(key, w, cx),
                        Some(RightPanelTab::Document(key)) => self.reload_document(key, cx),
                        Some(RightPanelTab::Files) => self.refresh_dock_files(pane, cx),
                        _ => return false,
                    },
                    _ => return false,
                }
            }
        }
        true
    }

    pub(super) fn handle_fixed_key(
        &mut self,
        combo: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.shortcuts.open {
            if combo == "Escape" {
                self.dismiss_shortcuts(w, cx);
                return true;
            }
            return false;
        }
        if self.show_settings {
            if self.transcript_menu_key(combo, w, cx) {
                return true;
            }
            if combo == "Escape" {
                self.close_settings(cx);
                return true;
            }
            return false;
        }
        if self.confirm_permission.is_some() && combo == "Escape" {
            self.close_permission_confirmation(w, cx);
            return true;
        }
        if self.sidebar_drawer
            && self.narrow_sidebar_expanded
            && matches!(combo, "Tab" | "shift+Tab")
            && self.workspace_rename.is_none()
            && self.workspace_delete.is_none()
            && self.session_rename.is_none()
        {
            if combo == "Tab" {
                w.focus_next(cx);
            } else {
                w.focus_prev(cx);
            }
            if !self.sidebar_drawer_focus.scope.contains_focused(w, cx) {
                let edge = if combo == "Tab" {
                    &self.sidebar_drawer_focus.close
                } else {
                    &self.sidebar_drawer_focus.more
                };
                edge.focus(w, cx);
            }
            return true;
        }
        if self.handle_command_picker_key(combo, w, cx)
            || self.handle_model_menu_key(combo, w, cx)
            || self.handle_permission_menu_key(combo, w, cx)
        {
            return true;
        }
        if matches!(combo, "Escape" | "Tab" | "shift+Tab")
            && self.dock_controls.dismiss_pinned(None)
        {
            cx.notify();
            if combo == "Escape" {
                return true;
            }
        }
        if combo == "Escape" {
            if self.dismiss_document_viewer(cx)
                || self.dismiss_terminal_shell_menu(true, w, cx)
                || self.dismiss_dock_tab_menu(true, w, cx)
            {
                return true;
            }
            let dock = self.cancel_dock_tab_drag(cx);
            let float = self.cancel_float_drag(cx);
            if dock || float {
                return true;
            }
        }
        // 官方审批卡固定键：Enter=允许一次，Esc=拒绝（此时输入区被审批卡替换）。
        if let Some(permission) = self.conversation.permission.as_ref() {
            let pick = |predicate: fn(&str) -> bool| {
                permission
                    .options
                    .iter()
                    .find(|(id, _)| predicate(id))
                    .map(|(id, _)| id.clone())
            };
            let option = match combo {
                "Enter" => {
                    pick(|id| id.contains("allow-once")).or_else(|| pick(|id| id.contains("allow")))
                }
                "Escape" => pick(|id| id.contains("reject") || id.contains("deny")),
                _ => None,
            };
            if let Some(option) = option {
                self.respond_permission(option, cx);
                return true;
            }
        }
        if let Some(pane) = self.focused_right_pane(w, cx)
            && let Some(RightPanelTab::Document(key)) = self.focused_dock_tab(pane, w, cx)
            && let Some(tab) = self.sidebar.documents.get(&key)
            && tab.selection.shortcut(combo, w, cx)
        {
            return true;
        }
        if self.composer_menu == Some(ComposerMenu::Commands) && self.command_from_slash {
            let catalog = command_catalog();
            let visible = visible_commands(&catalog, &self.command_filter);
            match combo {
                "ArrowDown" if !visible.is_empty() => {
                    self.command_selected = (self.command_selected + 1).min(visible.len() - 1);
                }
                "ArrowUp" if !visible.is_empty() => {
                    self.command_selected = self.command_selected.saturating_sub(1);
                }
                "Enter" if !visible.is_empty() => {
                    let index = self.command_selected.min(visible.len() - 1);
                    let key = visible
                        .get(index)
                        .filter(|spec| spec.enabled)
                        .map(|spec| spec.key);
                    if let Some(key) = key {
                        self.activate_command(key, w, cx);
                    }
                }
                _ => return false,
            }
            cx.notify();
            return true;
        }
        // 官方 Esc,Esc（500ms 内双击）停止生成；单次 Esc 交给弹层/编辑器处理。
        if combo == "Escape" {
            let chord = self
                .last_escape
                .is_some_and(|at| bridge::monotonic_ms() - at < 500.);
            if chord && self.conversation.running {
                self.last_escape = None;
                self.cancel(cx);
                return true;
            }
            self.last_escape = Some(bridge::monotonic_ms());
        }
        false
    }

    pub(super) fn activate_command(
        &mut self,
        command: &'static str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let from_slash = self.command_from_slash;
        self.command_from_slash = false;
        self.command_filter.clear();
        self.command_selected = 0;
        if from_slash && !matches!(command, "permission" | "model") {
            self.replace_composer_without_trigger("", w, cx);
        }
        match command {
            "file" => self.open_file_picker(w, cx),
            "attach" => {
                self.composer_menu = None;
                self.pick_attachments(cx);
                self.focus_composer(w, cx);
            }
            "model" => {
                self.open_command_picker(ComposerMenu::CommandModels, from_slash, w, cx);
            }
            "feedback" => {
                self.composer_menu = None;
                self.open_session_feedback(w, cx);
            }
            "export" => {
                self.composer_menu = None;
                self.start_session_download(w, cx);
            }
            "permission" => {
                self.open_command_picker(ComposerMenu::PermissionChoices, from_slash, w, cx);
            }
            _ => {
                self.composer_menu = None;
                self.focus_composer(w, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn activate_first_command(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let catalog = command_catalog();
        let key = visible_commands(&catalog, &self.command_filter)
            .into_iter()
            .find(|spec| spec.enabled)
            .map(|spec| spec.key);
        if let Some(key) = key {
            self.activate_command(key, w, cx);
        } else {
            self.composer_menu = None;
            cx.notify();
        }
    }
}
