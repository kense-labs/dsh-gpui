//! Workspace group rows, hover actions and native-path context menus.

use super::workspace::{composer_bg, menu_bg, path_name};
use super::*;
use gpui::{Transformation, svg};

impl DshApp {
    pub(super) fn workspace_name(&self, cwd: &str) -> String {
        self.workspace_aliases.get(cwd).cloned().unwrap_or_else(|| {
            if cwd.is_empty() {
                "默认工作区".to_string()
            } else {
                path_name(cwd)
            }
        })
    }

    pub(super) fn apply_workspace_preferences(&mut self, reply: Value, cx: &mut Context<Self>) {
        self.workspace_open_with = reply["workspaceOpenWith"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        self.workspace_aliases = reply
            .get("aliases")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(cwd, name)| {
                let name = name.as_str()?.trim();
                (!name.is_empty()).then(|| (cwd.clone(), name.to_string()))
            })
            .collect();
        self.hidden_workspaces = string_set(reply.get("hidden"));
        self.added_workspaces = reply["addedWorkspaces"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect();
        self.collapsed_workspaces = string_set(reply.get("collapsed"));
        if let Some(collapsed) = reply.get("sidebarCollapsed").and_then(Value::as_bool) {
            self.sidebar_collapsed = collapsed;
            // 启动恢复时不播放折叠动画，直接落到目标态。
            self.sidebar_slide = None;
        }
        if let Some(width) = reply
            .get("sidebarWidth")
            .and_then(Value::as_f64)
            .map(|width| width as f32)
            .filter(|width| (264. ..=420.).contains(width))
        {
            self.sidebar_width = width;
        }
        if let Some(group) = reply.get("groupBy").and_then(Value::as_str) {
            self.workspace_group_by = match group {
                "workspaceTree" => WorkspaceGroupBy::WorkspaceTree,
                "flat" => WorkspaceGroupBy::Flat,
                "workspace" => WorkspaceGroupBy::Workspace,
                _ => self.workspace_group_by,
            };
        }
        if let Some(order) = reply.get("orderBy").and_then(Value::as_str) {
            self.workspace_order_by = match order {
                "manual" => WorkspaceOrderBy::Manual,
                "updated" => WorkspaceOrderBy::Updated,
                _ => self.workspace_order_by,
            };
        }
        if let Some(archived) = reply.get("archivedFilter").and_then(Value::as_str) {
            self.workspace_archived_filter = match archived {
                "show" => WorkspaceArchivedFilter::Show,
                "only" => WorkspaceArchivedFilter::Only,
                "hide" => WorkspaceArchivedFilter::Hide,
                _ => self.workspace_archived_filter,
            };
        }
        if let Some(pinned) = reply.get("pinned").and_then(Value::as_array) {
            self.pinned_sessions = pinned
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
        }
        if let Some(titles) = reply.get("sessionTitles").and_then(Value::as_object) {
            self.legacy_session_titles = titles
                .into_iter()
                .filter_map(|(id, name)| {
                    let name = name.as_str()?.trim();
                    (!name.is_empty()).then(|| (id.clone(), name.to_string()))
                })
                .collect();
        }
        if !self.session_archival.available
            && let Some(archived) = reply.get("archived").and_then(Value::as_array)
        {
            self.archived_sessions = archived
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
        }
        self.pinned_sessions
            .retain(|id| !self.archived_sessions.contains(id));
        if let Some(delta) = reply.get("contentFontDelta").and_then(Value::as_i64) {
            self.content_font_delta = (delta as i32).clamp(-2, 4);
        }
        if let Some(mode) = reply.get("workDetails").and_then(Value::as_str) {
            self.work_details = match mode {
                "compact" => WorkDetailsMode::Compact,
                "detailed" => WorkDetailsMode::Detailed,
                "verbose" => WorkDetailsMode::Verbose,
                "standard" => WorkDetailsMode::Standard,
                _ => self.work_details,
            };
        }
        if let Some(timing) = reply["collapseTiming"].as_str() {
            self.collapse_next_input = timing == "next-input";
        }
        if let Some(replies) = reply["foldedReplies"].as_str() {
            self.keep_folded_replies = replies != "final";
        }
        if let Some(mode) = reply.get("themeMode").and_then(Value::as_str) {
            let next = match mode {
                "light" => ThemeMode::Light,
                "dark" => ThemeMode::Dark,
                _ => Theme::global(cx).mode,
            };
            if Theme::global(cx).mode != next {
                Theme::global_mut(cx).set_mode(next);
                self.sync_shell_window(cx);
                cx.refresh_windows();
            }
        }
    }

    pub(super) fn save_workspace_preferences(&self, cx: &App) {
        let mut hidden = self.hidden_workspaces.iter().cloned().collect::<Vec<_>>();
        let mut collapsed = self
            .collapsed_workspaces
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut pinned = self.pinned_sessions.iter().cloned().collect::<Vec<_>>();
        let mut archived = self.archived_sessions.iter().cloned().collect::<Vec<_>>();
        hidden.sort();
        collapsed.sort();
        pinned.sort();
        archived.sort();
        invoke(
            "save_workspace_preferences",
            json!({
                "preferences": {
                    "aliases": self.workspace_aliases,
                    "hidden": hidden,
                    "addedWorkspaces": self.added_workspaces,
                    "collapsed": collapsed,
                    "sidebarCollapsed": self.sidebar_collapsed,
                    "sidebarWidth": self.sidebar_width,
                    "groupBy": match self.workspace_group_by {
                        WorkspaceGroupBy::Workspace => "workspace",
                        WorkspaceGroupBy::WorkspaceTree => "workspaceTree",
                        WorkspaceGroupBy::Flat => "flat",
                    },
                    "orderBy": match self.workspace_order_by {
                        WorkspaceOrderBy::Manual => "manual",
                        WorkspaceOrderBy::Updated => "updated",
                    },
                    "archivedFilter": match self.workspace_archived_filter {
                        WorkspaceArchivedFilter::Hide => "hide",
                        WorkspaceArchivedFilter::Show => "show",
                        WorkspaceArchivedFilter::Only => "only",
                    },
                    "pinned": pinned,
                    "sessionTitles": self.legacy_session_titles,
                    "archived": archived,
                    "themeMode": match Theme::global(cx).mode {
                        ThemeMode::Dark => "dark",
                        ThemeMode::Light => "light",
                    },
                    "contentFontDelta": self.content_font_delta,
                    "collapseTiming": if self.collapse_next_input { "next-input" } else { "completion" },
                    "foldedReplies": if self.keep_folded_replies { "all" } else { "final" },
                    "workspaceOpenWith": self.workspace_open_with,
                    "workDetails": match self.work_details {
                        WorkDetailsMode::Compact => "compact",
                        WorkDetailsMode::Standard => "standard",
                        WorkDetailsMode::Detailed => "detailed",
                        WorkDetailsMode::Verbose => "verbose",
                    },
                }
            }),
        );
    }

    pub(super) fn render_workspace_group(
        &mut self,
        cwd: String,
        sessions: Vec<SessionSummary>,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        let expanded = !self.collapsed_workspaces.contains(&cwd);
        let hovered = self.hovered_workspace.as_deref() == Some(cwd.as_str());
        let menu_open = self
            .workspace_context_menu
            .as_ref()
            .is_some_and(|(path, _)| path == &cwd);
        // 官方：工作区操作只在展开时显示；无 hover 的设备上展开行常显。
        let revealed = expanded && (hovered || menu_open || !bridge::has_pointer_hover());
        let active = self
            .sessions
            .iter()
            .find(|session| Some(session.id.as_str()) == self.active_session.as_deref())
            .is_some_and(|session| session.cwd == cwd);
        let label = self.workspace_name(&cwd);
        let for_hover = cwd.clone();
        let for_toggle = cwd.clone();
        let for_menu = cwd.clone();
        let for_create = cwd.clone();
        let enabled = self.connected() && !self.creating_session;
        let folder_color = if active {
            p.state_business_primary
        } else {
            p.faint
        };
        let mut group = column().w_full().gap(px(2.)).child(
            row()
                .id(gpui::ElementId::Name(
                    format!("workspace-group-{cwd}").into(),
                ))
                .role(gpui::Role::Button)
                .aria_label(label.clone())
                .aria_expanded(expanded)
                .tab_index(0)
                .w_full()
                .h(px(32.))
                .flex_none()
                .px(px(8.))
                .gap(px(6.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(revealed, |element| element.bg(p.hover))
                .hover(|style| style.bg(p.hover))
                .on_hover(cx.listener(move |this, inside: &bool, _, cx| {
                    let next = inside.then_some(for_hover.clone());
                    if this.hovered_workspace != next {
                        this.hovered_workspace = next;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.workspace_context_menu = None;
                    if !this.collapsed_workspaces.remove(&for_toggle) {
                        this.collapsed_workspaces.insert(for_toggle.clone());
                        // 官方：折叠 Workspace 会重置它的水位线。
                        this.workspace_watermarks.remove(&for_toggle);
                    }
                    this.save_workspace_preferences(cx);
                    cx.notify();
                }))
                .on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        open_workspace_menu(this, &for_menu, event.position.y.into(), cx);
                    }),
                )
                .child(
                    div()
                        .size(px(16.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if revealed {
                            svg()
                                .data(include_bytes!("../../assets/workspace-triangle.svg"))
                                .size(px(14.))
                                .text_color(p.secondary)
                                .with_transformation(Transformation::default().with_rotation(
                                    gpui::radians(if expanded {
                                        std::f32::consts::FRAC_PI_2
                                    } else {
                                        0.
                                    }),
                                ))
                        } else {
                            svg()
                                .data(if expanded {
                                    include_bytes!("../../assets/workspace-folder-open.svg")
                                        .as_slice()
                                } else {
                                    include_bytes!("../../assets/workspace-folder.svg").as_slice()
                                })
                                .size(px(16.))
                                .text_color(folder_color)
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(14.))
                        .line_height(px(20.))
                        .text_color(p.ink)
                        .child(label),
                )
                .when(revealed, |element| {
                    let for_more = cwd.clone();
                    element
                        .child(
                            div()
                                .id(gpui::ElementId::Name(
                                    format!("workspace-more-{cwd}").into(),
                                ))
                                .role(gpui::Role::Button)
                                .aria_label("工作区更多操作")
                                .tab_index(0)
                                .size(px(24.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|style| style.text_color(p.ink))
                                .child(
                                    svg()
                                        .data(include_bytes!("../../assets/workspace-ellipsis.svg"))
                                        .size(px(16.))
                                        .text_color(p.secondary),
                                )
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(
                                        move |this, event: &gpui::MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            open_workspace_menu(
                                                this,
                                                &for_more,
                                                event.position.y.into(),
                                                cx,
                                            );
                                        },
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .id(gpui::ElementId::Name(
                                    format!("workspace-new-session-{cwd}").into(),
                                ))
                                .role(gpui::Role::Button)
                                .aria_label("在此工作区新建会话")
                                .tab_index(0)
                                .tab_stop(enabled)
                                .size(px(24.))
                                .ml(px(-4.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .when(enabled, |element| {
                                    element
                                        .cursor_pointer()
                                        .hover(|style| style.text_color(p.ink))
                                })
                                .when(!enabled, |element| element.opacity(0.4))
                                .child(
                                    svg()
                                        .data(include_bytes!("../../assets/new-chat-outline.svg"))
                                        .size(px(14.))
                                        .text_color(p.secondary),
                                )
                                .on_click(cx.listener(move |this, _, w, cx| {
                                    cx.stop_propagation();
                                    this.workspace_context_menu = None;
                                    this.collapsed_workspaces.remove(&for_create);
                                    this.new_session_at(for_create.clone(), None, w, cx);
                                })),
                        )
                }),
        );
        if expanded {
            // 官方水位线：初始 5 行；前 50 条里最后一条正在运行、等待用户、
            // 未读或在主视图打开的会话把水位线抬到它，之后不自行回落。
            const WATERMARK_BASE: usize = 5;
            let total = sessions.len();
            let active = self.active_session.clone();
            let mut limit = self
                .workspace_watermarks
                .get(&cwd)
                .copied()
                .unwrap_or(WATERMARK_BASE)
                .max(WATERMARK_BASE);
            for (index, session) in sessions.iter().take(50).enumerate() {
                let notable = self.session_indicator(&session.id).0
                    != crate::sessions::SessionIndicator::Idle
                    || active.as_deref() == Some(session.id.as_str());
                if notable {
                    limit = limit.max(index + 1);
                }
            }
            self.workspace_watermarks.insert(cwd.clone(), limit);
            // 当前选中的空白「新会话」是水位线之外的额外行。
            let extra_blank = active.as_deref().and_then(|id| {
                sessions
                    .iter()
                    .enumerate()
                    .find(|(index, session)| session.id == id && session.blank && *index >= limit)
                    .map(|(index, _)| index)
            });
            for (index, session) in sessions.into_iter().enumerate() {
                if index < limit || Some(index) == extra_blank {
                    group = group.child(self.render_session_row(session, w, cx));
                }
            }
            let remaining = total.saturating_sub(limit);
            if remaining > 0 || limit > WATERMARK_BASE {
                // 剩余五条或更少时一次显示全部；最后一批之后提供「收起」。
                let expand = remaining > 0;
                let target = if remaining <= 5 { total } else { limit + 5 };
                let for_watermark = cwd.clone();
                group = group.child(
                    div()
                        .id(gpui::ElementId::Name(
                            format!("workspace-watermark-{cwd}").into(),
                        ))
                        .role(gpui::Role::Button)
                        .tab_index(0)
                        .w_full()
                        .h(px(28.))
                        .pl(px(32.))
                        .flex()
                        .items_center()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(p.label_tertiary)
                        .hover(|style| style.text_color(p.label_primary))
                        .child(if expand {
                            format!("展开其余（{remaining}）")
                        } else {
                            "收起".to_owned()
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if expand {
                                this.workspace_watermarks
                                    .insert(for_watermark.clone(), target);
                            } else {
                                this.workspace_watermarks.remove(&for_watermark);
                            }
                            cx.notify();
                        })),
                );
            }
        }
        group
    }

    pub(super) fn render_workspace_context_menu(
        &self,
        cwd: String,
        pointer_y: f32,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let for_rename = cwd.clone();
        let for_reveal = cwd.clone();
        let height = f32::from(w.viewport_size().height);
        let top = (pointer_y - 12.).clamp(52., (height - 142.).max(52.));
        column()
            .id("workspace-context-menu")
            .absolute()
            .left(px(self.sidebar_width - 10.))
            .top(px(top))
            .w(px(216.))
            .gap_0()
            .p(px(4.))
            .rounded(px(12.))
            .bg(menu_bg(cx))
            .shadow(elevation_prominent())
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.workspace_context_menu = None;
                cx.notify();
            }))
            .child(workspace_menu_row(
                "rename",
                "重命名",
                include_bytes!("../../assets/workspace-edit.svg"),
                false,
                cx,
                move |this, w, cx| {
                    let name = this.workspace_name(&for_rename);
                    replace(&this.workspace_rename_editor, name, w, cx);
                    this.workspace_rename = Some(for_rename.clone());
                    this.workspace_context_menu = None;
                    this.workspace_rename_editor.update(cx, |editor, cx| {
                        editor.focus(w, cx);
                        editor.select_all(w, cx);
                    });
                    cx.notify();
                },
            ))
            .child(workspace_menu_row(
                "reveal",
                // The frontend is compiled to wasm32, so compile-time target
                // checks are always false here — use the runtime host platform.
                if bridge::is_mac() {
                    "在 Finder 中显示"
                } else if bridge::is_windows() {
                    "在资源管理器中显示"
                } else {
                    "在文件夹中显示"
                },
                include_bytes!("../../assets/workspace-reveal.svg"),
                false,
                cx,
                move |this, _, cx| {
                    invoke(
                        "open_start_tool",
                        json!({"workspace": for_reveal, "tool": "files"}),
                    );
                    this.workspace_context_menu = None;
                    cx.notify();
                },
            ))
            .child(div().h(px(0.5)).mx(px(8.)).my(px(3.)).bg(p.line))
            .child(workspace_menu_row(
                "delete",
                "移除工作区",
                include_bytes!("../../assets/workspace-trash.svg"),
                true,
                cx,
                move |this, _, cx| {
                    this.workspace_delete = Some(cwd.clone());
                    this.workspace_context_menu = None;
                    cx.notify();
                },
            ))
    }

    pub(super) fn close_workspace_dialog(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self
            .session_rename
            .as_ref()
            .is_some_and(|rename| rename.pending)
        {
            return;
        }
        self.workspace_rename = None;
        self.workspace_delete = None;
        if let Some(rename) = self.session_rename.take()
            && let Some(focus) = rename.restore_focus
        {
            focus.focus(w, cx);
        }
    }

    pub(super) fn begin_session_rename(
        &mut self,
        id: String,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.connected() || self.session_rename.is_some() {
            return;
        }
        let Some(session) = self
            .sessions
            .iter()
            .find(|session| session.id == id && !session.blank)
        else {
            return;
        };
        let name = self
            .session_title_name(session)
            .unwrap_or_default()
            .to_owned();
        self.dismiss_header_menu(true, w, cx);
        let restore_focus = w.focused(cx);
        super::document_html::restore_shell_focus();
        replace(&self.session_rename_editor, name, w, cx);
        let mut rename = SessionRename {
            id,
            restore_focus,
            close: cx.focus_handle().tab_stop(true),
            cancel: cx.focus_handle().tab_stop(true),
            confirm: cx.focus_handle().tab_stop(true),
            pending: false,
            error: None,
        };
        rename.set_pending(&self.session_rename_editor, false, cx);
        self.session_rename = Some(rename);
        self.session_rename_editor.update(cx, |editor, cx| {
            editor.focus(w, cx);
            editor.select_all(w, cx);
        });
        cx.notify();
    }

    pub(super) fn confirm_session_rename(&mut self, cx: &mut Context<Self>) {
        let Some(rename) = self.session_rename.as_mut() else {
            return;
        };
        let editor = self.session_rename_editor.read(cx);
        let title = editor.value().trim();
        if rename.pending || title.is_empty() || editor.is_composing() {
            return;
        }
        invoke(
            "rename_session",
            json!({"sessionId": rename.id, "title": title}),
        );
        rename.error = None;
        rename.set_pending(&self.session_rename_editor, true, cx);
    }

    pub(super) fn finish_session_rename(
        &mut self,
        result: Result<Value, String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rename) = self.session_rename.as_mut().filter(|rename| rename.pending) else {
            return;
        };
        let id = rename.id.clone();
        rename.set_pending(&self.session_rename_editor, false, cx);
        match result.and_then(|reply| {
            if let (Some(title), Some(seq)) = (reply["title"].as_str(), reply["seq"].as_u64()) {
                Ok((title.to_owned(), seq))
            } else {
                Err("服务未返回重命名结果。".into())
            }
        }) {
            Ok((title, seq)) => {
                self.apply_durable_session_title(&id, &title, seq);
                if self.legacy_session_titles.remove(&id).is_some() {
                    self.save_workspace_preferences(cx);
                }
                self.close_workspace_dialog(w, cx);
            }
            Err(error) => {
                rename.error = Some(error);
                self.session_rename_editor
                    .update(cx, |editor, cx| editor.focus(w, cx));
            }
        }
    }

    fn workspace_rename_duplicate(&self, path: &str, name: &str) -> bool {
        self.sessions
            .iter()
            .any(|session| session.cwd != path && self.workspace_name(&session.cwd) == name)
            || (self.workspace != path && self.workspace_name(&self.workspace) == name)
            || self
                .workspace_aliases
                .keys()
                .any(|cwd| cwd != path && self.workspace_name(cwd) == name)
    }

    pub(super) fn confirm_workspace_rename(&mut self, cx: &App) {
        let Some(path) = self.workspace_rename.clone() else {
            return;
        };
        let name = self
            .workspace_rename_editor
            .read(cx)
            .value()
            .trim()
            .to_string();
        if name.is_empty()
            || self.workspace_aliases.get(&path) == Some(&name)
            || self.workspace_rename_duplicate(&path, &name)
        {
            return;
        }
        self.workspace_aliases.insert(path, name);
        self.workspace_rename = None;
        self.save_workspace_preferences(cx);
    }

    pub(super) fn render_workspace_dialog(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let session_rename = self.session_rename.is_some();
        let pending = self
            .session_rename
            .as_ref()
            .is_some_and(|rename| rename.pending);
        let rename_error = self
            .session_rename
            .as_ref()
            .and_then(|rename| rename.error.clone());
        let rename = self.workspace_rename.is_some() || session_rename;
        let title = if session_rename {
            "重命名会话"
        } else if rename {
            "重命名工作区"
        } else {
            "移除工作区"
        };
        let path = self
            .workspace_rename
            .as_ref()
            .or(self.workspace_delete.as_ref())
            .cloned()
            .unwrap_or_default();
        let name = self.workspace_name(&path);
        let editor = if session_rename {
            self.session_rename_editor.clone()
        } else {
            self.workspace_rename_editor.clone()
        };
        let draft = editor.read(cx).value().trim().to_owned();
        // 重名检查只适用于工作区；会话标题允许与工作区无关。
        let duplicate = self.workspace_rename.is_some()
            && !draft.is_empty()
            && self.workspace_rename_duplicate(&path, &draft);
        let blocked = pending
            || draft.is_empty()
            || (!session_rename && self.workspace_aliases.get(&path) == Some(&draft))
            || duplicate;
        let width = (f32::from(w.viewport_size().width) - 48.).clamp(0., 380.);
        let caption = if bridge::is_mac() && !self.shell_fullscreen(w) {
            40.
        } else {
            0.
        };
        let rename_focus = self.session_rename.as_ref();
        div()
            .id("workspace-dialog")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, w, cx| {
                if event.keystroke.key == "escape" {
                    this.close_workspace_dialog(w, cx);
                    w.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(div().absolute().inset_0().top(px(caption)).bg(p.bg_mask_1))
            .child(
                column()
                    .id("workspace-dialog-panel")
                    .role(gpui::Role::Dialog)
                    .aria_label(title)
                    .w(px(width))
                    .pb(px(24.))
                    .rounded(px(28.))
                    .bg(composer_bg(cx))
                    .shadow(elevation_prominent())
                    .occlude()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                    .when(session_rename, |panel| {
                        panel.capture_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, w, cx| {
                            let stroke = &event.keystroke;
                            if stroke.key != "tab" || stroke.modifiers.control || stroke.modifiers.alt
                                || stroke.modifiers.platform {
                                return;
                            }
                            let Some(rename) = &this.session_rename else { return };
                            let focus = &this.session_rename_editor.read(cx).focus;
                            let stops = [&rename.close, focus, &rename.cancel, &rename.confirm];
                            let stops = &stops[..if rename.pending { 1 } else if blocked { 3 } else { 4 }];
                            let index = stops.iter().position(|stop| stop.is_focused(w));
                            let next = if stroke.modifiers.shift {
                                index.unwrap_or(0).checked_sub(1).unwrap_or(stops.len() - 1)
                            } else {
                                index.map_or(0, |index| (index + 1) % stops.len())
                            };
                            let target = stops[next].clone();
                            target.focus(w, cx);
                            w.prevent_default();
                            cx.stop_propagation();
                        }))
                    })
                    .child(super::permission_dialog::dialog_header(
                        title,
                        "close-workspace-dialog",
                        rename_focus.map(|rename| &rename.close),
                        cx,
                        |this, w, cx| {
                            this.close_workspace_dialog(w, cx);
                            cx.notify();
                        },
                    ))
                    .child(
                        column()
                            .w_full()
                            .mt(px(20.))
                            .px(px(24.))
                            .when(rename, |element| {
                                element
                                    .child(
                                        div()
                                            .w_full()
                                            .h(px(44.))
                                            .px(px(14.))
                                            .flex()
                                            .items_center()
                                            .rounded(px(16.))
                                            .border(px(0.5))
                                            .border_color(p.border_l4)
                                            .text_size(px(14.))
                                            .line_height(px(22.))
                                            .child(editor),
                                    )
                                    .when(duplicate, |element| {
                                        element.child(
                                            div()
                                                .mt(px(8.))
                                                .text_size(px(12.))
                                                .line_height(px(18.))
                                                .text_color(p.danger)
                                                .child(format!("已存在名为“{draft}”的工作区。")),
                                        )
                                    })
                            })
                            .when_some(rename_error, |element, error| {
                                element.child(
                                    div()
                                        .id("session-rename-error")
                                        .role(gpui::Role::Alert)
                                        .mt(px(8.))
                                        .text_size(px(12.))
                                        .line_height(px(18.))
                                        .text_color(p.danger)
                                        .child(error),
                                )
                            })
                            .when(!rename, |element| {
                                element.child(
                                    div()
                                        .text_size(px(14.))
                                        .line_height(px(22.))
                                        .text_color(p.secondary)
                                        .child(format!(
                                            "仅从侧栏移除“{name}”分组，不会删除磁盘文件夹或会话数据。"
                                        )),
                                )
                            }),
                    )
                    .child(
                        row()
                            .w_full()
                            .mt(px(20.))
                            .px(px(24.))
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                super::permission_dialog::dialog_action(
                                    "workspace-dialog-cancel",
                                    "取消",
                                    false,
                                    pending,
                                    0.,
                                    cx,
                                    |this, w, cx| {
                                        this.close_workspace_dialog(w, cx);
                                        cx.notify();
                                    },
                                )
                                .when_some(rename_focus, |button, rename| button.track_focus(&rename.cancel)),
                            )
                            .child(
                                super::permission_dialog::dialog_action(
                                    "workspace-dialog-confirm",
                                    if rename { "重命名" } else { "移除" },
                                    rename,
                                    rename && blocked,
                                    0.,
                                    cx,
                                    move |this, _, cx| {
                                        if session_rename {
                                            this.confirm_session_rename(cx);
                                        } else if rename {
                                            this.confirm_workspace_rename(cx);
                                        } else {
                                            this.hidden_workspaces.insert(path.clone());
                                            this.workspace_delete = None;
                                            this.save_workspace_preferences(cx);
                                            this.push_toast(
                                                "已从侧栏移除工作区分组；可在「设置 → 通用」中恢复。",
                                                cx,
                                            );
                                        }
                                        cx.notify();
                                    },
                                )
                                .when_some(rename_focus, |button, rename| button.track_focus(&rename.confirm))
                                .when(!rename, |element| element.text_color(p.danger)),
                            ),
                    ),
            )
            .on_click(cx.listener(|this, _, w, cx| {
                this.close_workspace_dialog(w, cx);
                cx.notify();
            }))
    }
}

impl SessionRename {
    fn set_pending(&mut self, editor: &gpui::Entity<Editor>, pending: bool, cx: &mut App) {
        self.pending = pending;
        let p = dsh(cx);
        let color = if pending {
            p.label_dimmed
        } else {
            p.label_primary
        };
        editor.update(cx, |editor, cx| {
            editor.disabled = pending;
            editor.set_run_styler(std::rc::Rc::new(move |_, run| {
                vec![gpui::TextRun {
                    color,
                    ..run.clone()
                }]
            }));
            cx.notify();
        });
    }
}

fn open_workspace_menu(this: &mut DshApp, cwd: &str, pointer_y: f32, cx: &mut Context<DshApp>) {
    this.workspace_context_menu = Some((cwd.to_string(), pointer_y));
    this.replace_header_menu(cx);
    this.show_workspace_view_options = false;
    this.show_workspace_menu = false;
    this.show_more_menu = false;
    this.composer_menu = None;
    cx.stop_propagation();
    cx.notify();
}

impl DshApp {
    pub(super) fn toggle_session_pin(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.pinned_sessions.remove(id) {
            self.pinned_sessions.insert(id.to_owned());
        }
        self.save_workspace_preferences(cx);
        cx.notify();
    }
    /// Preserve pre-upgrade aliases until the user saves a durable title.
    pub(super) fn session_title_name<'a>(&'a self, session: &'a SessionSummary) -> Option<&'a str> {
        self.legacy_session_titles
            .get(&session.id)
            .map(String::as_str)
            .or(session.title.as_deref())
    }

    pub(super) fn session_display_title(&self, session: &SessionSummary) -> String {
        self.session_title_name(session)
            .unwrap_or_else(|| session.title())
            .to_owned()
    }
}

fn string_set(value: Option<&Value>) -> std::collections::HashSet<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn workspace_menu_row(
    id: &'static str,
    label: &'static str,
    artwork: &'static [u8],
    danger: bool,
    cx: &mut Context<DshApp>,
    action: impl Fn(&mut DshApp, &mut Window, &mut Context<DshApp>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let p = dsh(cx);
    let color = if danger { p.danger } else { p.ink };
    row()
        .id(gpui::ElementId::Name(format!("workspace-menu-{id}").into()))
        .role(gpui::Role::Button)
        .aria_label(label)
        .tab_index(0)
        .w_full()
        .h(px(32.))
        .flex_none()
        .px(px(8.))
        .gap(px(8.))
        .rounded(px(8.))
        .cursor_pointer()
        .text_size(px(14.))
        .line_height(px(20.))
        .text_color(color)
        .hover(|style| style.bg(p.hover))
        .child(
            svg()
                .data(artwork)
                .size(px(16.))
                .flex_none()
                .text_color(color),
        )
        .child(div().flex_1().min_w_0().truncate().child(label))
        .on_click(cx.listener(move |this, _, w, cx| action(this, w, cx)))
}
