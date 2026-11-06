//! Harness-shaped workspace chrome over the ACP-backed application state.

use super::*;
use gpui::{Transformation, svg};
use kui::primitives::ops_surface::icon_btn;

impl DshApp {
    pub(super) fn render_workspace(&mut self, w: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = Paint::get(cx);
        if self.show_welcome || !self.welcome.read(cx).signed_in() {
            return div()
                .relative()
                .size_full()
                .bg(conversation_bg(cx))
                .child(self.welcome.clone())
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(52.))
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, _| begin_window_drag()),
                );
        }
        let mut main = column()
            .flex_1()
            .h_full()
            .min_w_0()
            .gap_0()
            .bg(conversation_bg(cx));
        match self.page {
            WorkspacePage::Conversation => {
                main = main.child(self.render_header(w, cx));
                if self.entries.is_empty() {
                    main = main.child(self.render_hero(w, cx));
                } else {
                    main = main
                        .child(self.render_conversation(w, cx))
                        .child(self.render_composer(w, cx, false));
                }
            }
            WorkspacePage::Plugins => {
                let panel = self.render_plugins_panel(w, cx);
                main = main.child(self.render_global_panel(panel, w, cx));
            }
            WorkspacePage::Automations => {
                let panel = self.render_automations_panel(w, cx);
                main = main.child(self.render_global_panel(panel, w, cx));
            }
            WorkspacePage::More => {
                let panel = self.render_more_panel(cx);
                main = main.child(self.render_global_panel(panel, w, cx));
            }
        }
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(conversation_bg(cx))
            .text_color(p.ink)
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.sidebar_dragging {
                    this.sidebar_width = f32::from(event.position.x).clamp(264., 420.);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.sidebar_dragging {
                        this.sidebar_dragging = false;
                        cx.notify();
                    }
                }),
            )
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key != "escape" {
                    return;
                }
                if this.confirm_full_access {
                    this.close_full_access_confirmation();
                    cx.notify();
                } else if this.workspace_context_menu.is_some() {
                    this.workspace_context_menu = None;
                    cx.notify();
                } else if this.composer_menu.is_some() {
                    this.composer_menu = match this.composer_menu {
                        Some(ComposerMenu::ModelChoices | ComposerMenu::ReasoningChoices) => {
                            Some(ComposerMenu::Model)
                        }
                        _ => None,
                    };
                    cx.notify();
                } else if this.show_workspace_menu {
                    this.show_workspace_menu = false;
                    cx.notify();
                } else if this.show_workspace_view_options {
                    this.show_workspace_view_options = false;
                    cx.notify();
                }
            }))
            .child(
                row()
                    .size_full()
                    .gap_0()
                    .overflow_hidden()
                    .child(self.render_sidebar(w, cx))
                    .when(!self.sidebar_collapsed, |element| {
                        element.child(
                            div()
                                .id("sidebar-resize-handle")
                                .w(px(8.))
                                .h_full()
                                .ml(px(-4.))
                                .mr(px(-4.))
                                .flex_none()
                                .cursor(gpui::CursorStyle::ResizeLeftRight)
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.sidebar_dragging = true;
                                        cx.notify();
                                    }),
                                ),
                        )
                    })
                    .when(
                        !(self.page == WorkspacePage::Conversation
                            && self.entries.is_empty()
                            && self.show_start_panel
                            && self.start_panel_expanded),
                        |element| element.child(main),
                    )
                    .when(
                        self.page == WorkspacePage::Conversation
                            && self.entries.is_empty()
                            && self.show_start_panel,
                        |element| element.child(self.render_start_panel(w, cx)),
                    ),
            )
            .when(self.show_account_menu, |element| {
                element.child(self.render_account_menu(cx))
            })
            .when_some(
                self.workspace_context_menu.clone(),
                |element, (cwd, pointer_y)| {
                    element.child(self.render_workspace_context_menu(cwd, pointer_y, w, cx))
                },
            )
            .when(
                self.workspace_rename.is_some() || self.workspace_delete.is_some(),
                |element| element.child(self.render_workspace_dialog(w, cx)),
            )
            .when(self.confirm_full_access, |element| {
                element.child(self.render_full_access_dialog(w, cx))
            })
            .when(self.show_settings, |element| {
                element.child(self.render_settings(w, cx))
            })
    }

    fn render_global_panel(
        &mut self,
        panel: impl IntoElement,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(panel)
            .when(self.sidebar_collapsed, |element| {
                element.child(self.render_leading_controls(w, cx))
            })
    }

    fn render_sidebar(&mut self, w: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = Paint::get(cx);
        self.sidebar_slide
            .duration(if self.sidebar_collapsed { 0. } else { 1. }, 0.3, cx);
        let progress = self.sidebar_slide.get(w, cx).clamp(0., 1.);
        let width = self.sidebar_width * progress;
        let content_width = self.sidebar_width - 16.;
        if self.sidebar_collapsed && progress < 0.08 {
            return div().w_0().h_full().flex_none().overflow_hidden();
        }

        let query = self.search_editor.read(cx).value().trim().to_lowercase();
        let filtered = self
            .sessions
            .iter()
            .filter(|session| {
                !self.hidden_workspaces.contains(&session.cwd)
                    && (query.is_empty()
                        || session.title().to_lowercase().contains(&query)
                        || session.cwd.to_lowercase().contains(&query)
                        || self
                            .workspace_name(&session.cwd)
                            .to_lowercase()
                            .contains(&query))
            })
            .cloned()
            .collect::<Vec<_>>();
        let filtered_count = filtered.len();
        let mut session_groups = column().w_full().gap(px(match self.workspace_group_by {
            WorkspaceGroupBy::Workspace | WorkspaceGroupBy::WorkspaceTree => 4.,
            WorkspaceGroupBy::Flat => 2.,
        }));
        match self.workspace_group_by {
            WorkspaceGroupBy::Flat => {
                for session in filtered {
                    session_groups = session_groups.child(self.render_session_row(session, cx));
                }
            }
            WorkspaceGroupBy::Workspace | WorkspaceGroupBy::WorkspaceTree => {
                let mut groups: Vec<(String, Vec<SessionSummary>)> = Vec::new();
                for session in filtered {
                    if let Some((_, sessions)) =
                        groups.iter_mut().find(|(cwd, _)| cwd == &session.cwd)
                    {
                        sessions.push(session);
                    } else {
                        groups.push((session.cwd.clone(), vec![session]));
                    }
                }
                for (cwd, sessions) in groups {
                    session_groups =
                        session_groups.child(self.render_workspace_group(cwd, sessions, cx));
                }
            }
        }
        let account_name = {
            let welcome = self.welcome.read(cx);
            if welcome.signed_in() {
                welcome
                    .profile
                    .as_ref()
                    .and_then(|profile| {
                        profile
                            .get("name")
                            .or_else(|| profile.get("contact"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or("已登录")
                    .to_string()
            } else {
                "更多".to_string()
            }
        };

        column()
            .relative()
            .w(px(width))
            .h_full()
            .flex_none()
            .overflow_hidden()
            .gap(px(4.))
            .px(px(8.))
            .py(px(6.))
            .bg(sidebar_bg(cx))
            .border_r_1()
            .border_color(if dark(cx) {
                gpui::rgb(0x232324).into()
            } else {
                p.line
            })
            .opacity((progress * 2.).min(1.))
            .child(
                row()
                    .w(px(content_width))
                    .h(px(52.))
                    .flex_none()
                    .mt(px(-6.))
                    .justify_end()
                    .pr(px(4.))
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, _| begin_window_drag()),
                    )
                    .child(self.sidebar_toggle_button(true, cx)),
            )
            .child(
                row()
                    .w(px(content_width))
                    .h(px(60.))
                    .flex_none()
                    .mt(px(-12.))
                    .px(px(8.))
                    .gap(px(5.))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, _| begin_window_drag())
                    .child(static_brand_mark(24., p.ink))
                    .child(
                        div()
                            .text_size(px(18.))
                            .line_height(px(24.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("deepseek"),
                    )
                    .child(
                        div()
                            .h(px(13.))
                            .px(px(3.))
                            .rounded(px(3.))
                            .border(px(0.5))
                            .border_color(p.line)
                            .text_size(px(8.))
                            .line_height(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.secondary)
                            .child("HARNESS"),
                    ),
            )
            .child(
                div()
                    .w(px(content_width))
                    .px(px(4.))
                    .child(self.new_session_button(cx)),
            )
            .child(
                div()
                    .w(px(content_width))
                    .mt(px(8.))
                    .px(px(3.))
                    .child(self.render_workspace_navigation(cx)),
            )
            .child(self.render_workspace_search(content_width, filtered_count, w, cx))
            .child(
                div()
                    .id("session-list")
                    .flex_1()
                    .min_h_0()
                    .w(px(content_width))
                    .overflow_y_scroll()
                    .px(px(3.))
                    .child(session_groups)
                    .when(filtered_count == 0, |element| {
                        element.child(
                            div()
                                .px(px(9.))
                                .py(px(12.))
                                .text_size(px(12.))
                                .text_color(p.faint)
                                .child(if query.is_empty() {
                                    "暂无会话"
                                } else {
                                    "没有匹配的会话"
                                }),
                        )
                    }),
            )
            .child(
                row()
                    .id("sidebar-account")
                    .role(gpui::Role::Button)
                    .aria_label("账户与设置")
                    .tab_index(0)
                    .w(px(content_width))
                    .h(px(44.))
                    .flex_none()
                    .px(px(9.))
                    .gap(px(8.))
                    .rounded(px(12.))
                    .cursor_pointer()
                    .hover(|style| style.bg(p.hover))
                    .child(account_avatar(
                        self.welcome.read(cx).avatar.as_deref(),
                        24.,
                        cx,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(14.))
                            .child(account_name),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_account_menu = !this.show_account_menu;
                        this.show_workspace_menu = false;
                        this.show_workspace_view_options = false;
                        this.composer_menu = None;
                        cx.notify();
                    })),
            )
            .when(self.show_workspace_view_options, |element| {
                element.child(self.render_workspace_view_options(cx))
            })
    }

    fn render_workspace_view_options(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        column()
            .id("workspace-view-options-menu")
            .absolute()
            .top(px(312.))
            .right(px(8.))
            .min_w(px(200.))
            .max_w(px(360.))
            .gap_0()
            .p(px(4.))
            .rounded(px(12.))
            .bg(menu_bg(cx))
            .shadow_lg()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.show_workspace_view_options = false;
                cx.notify();
            }))
            .child(view_option_label("分组方式", cx))
            .child(self.workspace_view_option(
                "group-workspace",
                "按工作区",
                include_bytes!("../../assets/workspace-folder.svg"),
                self.workspace_group_by == WorkspaceGroupBy::Workspace,
                |this, _| this.workspace_group_by = WorkspaceGroupBy::Workspace,
                cx,
            ))
            .child(self.workspace_view_option(
                "group-workspace-tree",
                "按工作区树",
                include_bytes!("../../assets/workspace-tree.svg"),
                self.workspace_group_by == WorkspaceGroupBy::WorkspaceTree,
                |this, _| this.workspace_group_by = WorkspaceGroupBy::WorkspaceTree,
                cx,
            ))
            .child(self.workspace_view_option(
                "group-flat",
                "单列表",
                include_bytes!("../../assets/workspace-flat.svg"),
                self.workspace_group_by == WorkspaceGroupBy::Flat,
                |this, _| this.workspace_group_by = WorkspaceGroupBy::Flat,
                cx,
            ))
            .child(div().h(px(0.5)).mx(px(8.)).my(px(3.)).bg(p.line))
            .child(view_option_label("排序方式", cx))
            .child(self.workspace_view_option(
                "order-manual",
                "手动排序",
                include_bytes!("../../assets/workspace-manual.svg"),
                self.workspace_order_by == WorkspaceOrderBy::Manual,
                |this, _| this.workspace_order_by = WorkspaceOrderBy::Manual,
                cx,
            ))
            .child(self.workspace_view_option(
                "order-updated",
                "最近更新",
                include_bytes!("../../assets/workspace-updated.svg"),
                self.workspace_order_by == WorkspaceOrderBy::Updated,
                |this, _| this.workspace_order_by = WorkspaceOrderBy::Updated,
                cx,
            ))
            .child(div().h(px(0.5)).mx(px(8.)).my(px(3.)).bg(p.line))
            .child(view_option_label("筛选会话", cx))
            .child(self.workspace_view_option(
                "filter-hide-archived",
                "隐藏已归档",
                include_bytes!("../../assets/workspace-hide-archived.svg"),
                self.workspace_archived_filter == WorkspaceArchivedFilter::Hide,
                |this, _| {
                    this.workspace_archived_filter = WorkspaceArchivedFilter::Hide;
                },
                cx,
            ))
            .child(self.workspace_view_option(
                "filter-show-archived",
                "全部对话（显示已归档）",
                include_bytes!("../../assets/workspace-show-archived.svg"),
                self.workspace_archived_filter == WorkspaceArchivedFilter::Show,
                |this, _| {
                    this.workspace_archived_filter = WorkspaceArchivedFilter::Show;
                },
                cx,
            ))
            .child(self.workspace_view_option(
                "filter-only-archived",
                "仅显示已归档",
                include_bytes!("../../assets/workspace-only-archived.svg"),
                self.workspace_archived_filter == WorkspaceArchivedFilter::Only,
                |this, _| {
                    this.workspace_archived_filter = WorkspaceArchivedFilter::Only;
                },
                cx,
            ))
    }

    fn workspace_view_option(
        &self,
        id: &'static str,
        label: &'static str,
        artwork: &'static [u8],
        selected: bool,
        on_select: impl Fn(&mut DshApp, &mut Context<DshApp>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        row()
            .id(gpui::ElementId::Name(
                format!("workspace-option-{id}").into(),
            ))
            .role(gpui::Role::Button)
            .aria_label(label)
            .aria_selected(selected)
            .tab_index(0)
            .w_full()
            .h(px(30.))
            .flex_none()
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(13.))
            .line_height(px(20.))
            .hover(|style| style.bg(p.hover))
            .child(
                svg()
                    .data(artwork)
                    .size(px(14.))
                    .flex_none()
                    .text_color(p.secondary),
            )
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when(selected, |element| {
                element.child(
                    svg()
                        .data(include_bytes!("../../assets/menu-check.svg"))
                        .size(px(14.))
                        .flex_none()
                        .text_color(p.ink),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                on_select(this, cx);
                this.show_workspace_view_options = false;
                cx.notify();
            }))
    }

    fn render_workspace_navigation(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut navigation = column().w_full().gap(px(2.));
        for (page, label, glyph) in [
            (WorkspacePage::Plugins, "插件", Glyph::Bulb),
            (WorkspacePage::Automations, "自动化任务", Glyph::Clock),
            (WorkspacePage::More, "更多", Glyph::More),
        ] {
            let active = self.page == page;
            let p = Paint::get(cx);
            navigation = navigation.child(
                row()
                    .id(gpui::ElementId::Name(
                        format!("workspace-nav-{label}").into(),
                    ))
                    .role(gpui::Role::Button)
                    .aria_label(label)
                    .aria_selected(active)
                    .tab_index(0)
                    .w_full()
                    .h(px(32.))
                    .flex_none()
                    .px(px(9.))
                    .gap(px(8.))
                    .rounded(px(12.))
                    .cursor_pointer()
                    .when(active, |element| element.bg(p.hover))
                    .hover(|style| style.bg(p.hover))
                    .child(icon(glyph, 16., if active { p.ink } else { p.secondary }))
                    .child(
                        div()
                            .text_size(px(14.))
                            .line_height(px(22.))
                            .text_color(if active { p.ink } else { p.secondary })
                            .child(label),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.open_page(page, cx))),
            );
        }
        navigation
    }

    fn render_leading_controls(&self, w: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let enabled = self.connected() && !self.running && !self.creating_session;
        row()
            .absolute()
            .top(px(11.))
            .left(px(if w.is_fullscreen() { 12. } else { 88. }))
            .gap(px(8.))
            .child(self.sidebar_toggle_button(false, cx))
            .child(
                div()
                    .id("collapsed-new-session")
                    .role(gpui::Role::Button)
                    .aria_label("新建会话")
                    .tab_index(0)
                    .tab_stop(enabled)
                    .size(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .text_color(Paint::get(cx).secondary)
                    .when(enabled, |element| {
                        element
                            .cursor_pointer()
                            .hover(|style| style.bg(Paint::get(cx).hover))
                            .on_click(cx.listener(|this, _, w, cx| this.new_session(w, cx)))
                    })
                    .when(!enabled, |element| element.opacity(0.4).cursor_default())
                    .child(
                        svg()
                            .data(include_bytes!("../../assets/new-chat-outline.svg"))
                            .size(px(16.))
                            .text_color(Paint::get(cx).secondary),
                    ),
            )
    }

    fn sidebar_toggle_button(
        &self,
        collapse: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        div()
            .id(if collapse {
                "collapse-sidebar"
            } else {
                "expand-sidebar"
            })
            .role(gpui::Role::Button)
            .aria_label(if collapse {
                "收起侧栏"
            } else {
                "展开侧栏"
            })
            .tab_index(0)
            .size(px(28.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|style| style.bg(p.hover))
            .child(
                svg()
                    .data(include_bytes!("../../assets/sidebar-panel.svg"))
                    .size(px(16.))
                    .text_color(p.secondary),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.sidebar_collapsed = collapse;
                cx.notify();
            }))
    }

    fn new_session_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        let enabled = self.connected() && !self.running && !self.creating_session;
        let mut button = row()
            .id("new-session")
            .role(gpui::Role::Button)
            .aria_label("新建会话")
            .tab_index(0)
            .tab_stop(enabled)
            .w_full()
            .h(px(38.))
            .flex_none()
            .justify_center()
            .gap(px(6.))
            .rounded(px(12.))
            .border(px(0.5))
            .border_color(p.line)
            .bg(if dark(cx) {
                gpui::rgba(0xffffff0f).into()
            } else {
                p.panel
            })
            .text_size(px(14.))
            .font_weight(FontWeight::MEDIUM)
            .child(icon(Glyph::Plus, 15., p.ink))
            .child("新建会话");
        if enabled {
            button = button
                .cursor_pointer()
                .hover(|style| style.bg(p.hover))
                .on_click(cx.listener(|this, _, w, cx| this.new_session(w, cx)));
        } else {
            button = button.opacity(0.45).cursor_default();
        }
        button
    }

    pub(super) fn render_session_row(
        &self,
        session: SessionSummary,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        let selected = self.active_session.as_deref() == Some(session.id.as_str());
        let title = session.title().to_string();
        let for_click = session.clone();
        row()
            .id(gpui::ElementId::Name(session.id.clone().into()))
            .w_full()
            .h(px(32.))
            .flex_none()
            .gap(px(6.))
            .px(px(9.))
            .rounded(px(12.))
            .cursor_pointer()
            .when(selected, |element| element.bg(p.hover))
            .hover(|style| style.bg(p.hover))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .text_color(if selected || !session.blank {
                        p.ink
                    } else {
                        p.secondary
                    })
                    .child(title),
            )
            .when(selected && self.running, |element| {
                element.child(div().size(px(6.)).rounded_full().bg(p.success))
            })
            .on_click(cx.listener(move |this, _, w, cx| {
                if this.active_session.as_deref() != Some(for_click.id.as_str()) {
                    this.resume(for_click.clone(), w, cx);
                }
            }))
    }

    fn render_header(&mut self, w: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = Paint::get(cx);
        let blank = self.entries.is_empty();
        let leading_clearance = if w.is_fullscreen() { 84. } else { 160. };
        let title = if blank {
            String::new()
        } else {
            self.entries
                .iter()
                .find(|entry| entry.role == EntryRole::User)
                .map(|entry| short_text(&entry.text, 52))
                .unwrap_or_else(|| "已恢复的会话".to_string())
        };
        div()
            .relative()
            .w_full()
            .h(if blank { px(40.) } else { px(50.) })
            .flex_none()
            .when(!blank, |element| element.border_b_1().border_color(p.line))
            .child(
                row()
                    .size_full()
                    .pl(if self.sidebar_collapsed {
                        px(leading_clearance)
                    } else {
                        px(20.)
                    })
                    .pr(px(28.))
                    .pt(px(10.))
                    .when(!blank, |element| element.pb(px(10.)))
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .truncate()
                            .text_size(px(14.))
                            .line_height(px(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(title)
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, _| begin_window_drag()),
                    )
                    .when(self.running, |element| {
                        element
                            .child(div().size(px(6.)).rounded_full().bg(p.success))
                            .child(note("运行中", cx))
                    })
                    .when(blank && !self.show_start_panel, |element| {
                        element.child(self.render_start_panel_toggle(cx))
                    }),
            )
            .when(self.sidebar_collapsed, |element| {
                element.child(self.render_leading_controls(w, cx))
            })
    }

    fn composer_width(&self, w: &Window) -> f32 {
        let width = composer_axis(w, self.sidebar_collapsed, self.sidebar_width);
        if self.page == WorkspacePage::Conversation
            && self.entries.is_empty()
            && self.show_start_panel
            && !self.start_panel_expanded
        {
            let main_width = f32::from(w.viewport_size().width)
                - if self.sidebar_collapsed {
                    0.
                } else {
                    self.sidebar_width
                };
            let panel = super::start_panel::start_panel_width(
                w,
                self.sidebar_collapsed,
                self.sidebar_width,
            );
            width.min((main_width - panel - 48.).max(240.))
        } else {
            width
        }
    }

    fn render_hero(&mut self, w: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = Paint::get(cx);
        let composer_width = self.composer_width(w);
        let mut workspace_menu = column().w_full().gap(px(2.));
        let mut workspaces = self
            .sessions
            .iter()
            .map(|session| session.cwd.clone())
            .collect::<Vec<_>>();
        if !self.workspace.is_empty() {
            workspaces.push(self.workspace.clone());
        }
        workspaces.sort();
        workspaces.dedup();
        for workspace in workspaces {
            let selected = workspace == self.workspace;
            let value = workspace.clone();
            workspace_menu = workspace_menu.child(
                row()
                    .id(gpui::ElementId::Name(
                        format!("workspace-{workspace}").into(),
                    ))
                    .w_full()
                    .h(px(32.))
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|style| style.bg(p.hover))
                    .child(icon(Glyph::Folder, 16., p.secondary))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .child(path_name(&workspace)),
                    )
                    .when(selected, |element| {
                        element.child(icon(Glyph::Check, 14., p.ink))
                    })
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.show_workspace_menu = false;
                        if this.running || this.creating_session {
                            return;
                        }
                        replace(&this.workspace_editor, value.clone(), w, cx);
                        this.start_service(w, cx);
                    })),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .px(px(24.))
            .pb(px(32.))
            .child(
                column()
                    .w(px(composer_width))
                    .max_w_full()
                    .gap(px(8.))
                    .child(
                        row()
                            .w_full()
                            .justify_center()
                            .gap(px(10.))
                            .child(self.brand_mark(w, cx, 34.))
                            .child(
                                row()
                                    .gap(px(7.))
                                    .child(
                                        div()
                                            .text_size(px(26.))
                                            .line_height(px(32.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child("探索未至之境"),
                                    )
                                    .child(
                                        div()
                                            .mt(px(2.))
                                            .px(px(7.))
                                            .rounded_full()
                                            .border(px(0.5))
                                            .border_color(p.line)
                                            .bg(if dark(cx) {
                                                gpui::rgb(0x273449)
                                            } else {
                                                gpui::rgb(0xeaf2ff)
                                            })
                                            .font_family("monospace")
                                            .text_size(px(12.))
                                            .line_height(px(18.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(if dark(cx) {
                                                gpui::rgb(0xa9c7ff)
                                            } else {
                                                gpui::rgb(0x4176e6)
                                            })
                                            .child("预览版"),
                                    ),
                            ),
                    )
                    .when(self.resumed_session, |element| {
                        element.child(row().w_full().justify_center().pb(px(4.)).child(note(
                            "会话已恢复。ACP 不回放历史消息，可从下方继续发送新任务。",
                            cx,
                        )))
                    })
                    .child(
                        column()
                            .relative()
                            .w_full()
                            .mt(px(4.))
                            .gap(px(8.))
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                if this.show_workspace_menu {
                                    this.show_workspace_menu = false;
                                    cx.notify();
                                }
                            }))
                            .child(
                                row()
                                    .w_full()
                                    .pl(px(20.))
                                    .pr(px(16.))
                                    .gap(px(4.))
                                    .child(
                                        row()
                                            .id("hero-workspace")
                                            .h(px(28.))
                                            .min_w_0()
                                            .max_w(px(360.))
                                            .px(px(8.))
                                            .gap(px(4.))
                                            .rounded(px(8.))
                                            .cursor_pointer()
                                            .when(self.show_workspace_menu, |element| {
                                                element.bg(p.hover)
                                            })
                                            .hover(|style| style.bg(p.hover))
                                            .child(icon(Glyph::Folder, 16., p.ink))
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_size(px(13.))
                                                    .line_height(px(20.))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(path_name(&self.workspace)),
                                            )
                                            .child(icon(Glyph::Down, 12., p.faint))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.show_workspace_menu =
                                                    !this.show_workspace_menu;
                                                this.composer_menu = None;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        row()
                                            .id("hero-agent-preset")
                                            .aria_label("标准模式，当前 ACP 服务固定使用此模式")
                                            .h(px(28.))
                                            .flex_none()
                                            .px(px(8.))
                                            .gap(px(5.))
                                            .rounded(px(8.))
                                            .text_color(p.secondary)
                                            .child(icon(Glyph::Rocket, 14., p.secondary))
                                            .child(
                                                div()
                                                    .text_size(px(13.))
                                                    .line_height(px(20.))
                                                    .child("标准模式"),
                                            ),
                                    ),
                            )
                            .child(self.render_composer(w, cx, true))
                            // Paint the picker after the composer so its surface is never
                            // covered by the card, matching the official portal layering.
                            .when(self.show_workspace_menu, |element| {
                                element.child(
                                    column()
                                        .id("workspace-menu")
                                        .absolute()
                                        .top(px(36.))
                                        .left(px(20.))
                                        .w(px(360.))
                                        .max_h(px(330.))
                                        .overflow_y_scroll()
                                        .gap(px(2.))
                                        .p(px(4.))
                                        .rounded(px(16.))
                                        .bg(menu_bg(cx))
                                        .shadow_lg()
                                        .occlude()
                                        .child(workspace_menu)
                                        .child(div().h(px(0.5)).mx(px(4.)).my(px(3.)).bg(p.line))
                                        .child(
                                            row()
                                                .id("manage-workspace")
                                                .role(gpui::Role::Button)
                                                .aria_label("添加工作区")
                                                .tab_index(0)
                                                .w_full()
                                                .h(px(32.))
                                                .px(px(8.))
                                                .gap(px(8.))
                                                .rounded(px(8.))
                                                .cursor_pointer()
                                                .hover(|style| style.bg(p.hover))
                                                .child(icon(Glyph::Plus, 16., p.secondary))
                                                .child(
                                                    div()
                                                        .text_size(px(13.))
                                                        .line_height(px(20.))
                                                        .child("添加工作区"),
                                                )
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.show_workspace_menu = false;
                                                    this.settings_page = SettingsPage::General;
                                                    this.show_settings = true;
                                                    cx.notify();
                                                })),
                                        ),
                                )
                            }),
                    ),
            )
    }

    fn render_conversation(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        let content_width = conversation_axis(w, self.sidebar_collapsed, self.sidebar_width);
        let mut content = column()
            .w(px(content_width))
            .max_w_full()
            .mx_auto()
            .gap_0()
            .pt(px(16.))
            .pb(px(16.));
        let mut previous_role = None;
        for entry in &self.entries {
            let gap = match (previous_role, entry.role) {
                (None, _) => 0.,
                (_, EntryRole::User) => 16.,
                (Some(EntryRole::Assistant), _) | (_, EntryRole::Assistant) => 12.,
                _ => 4.,
            };
            let expanded = self.expanded_entries.contains(&entry.key);
            content = content.child(render_entry(entry, expanded, cx).mt(px(gap)));
            previous_role = Some(entry.role);
        }
        if self.running {
            content = content.child(
                row()
                    .w_full()
                    .h(px(28.))
                    .mt(px(16.))
                    .gap(px(6.))
                    .child(static_brand_mark(14., p.secondary))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(22.))
                            .text_color(p.secondary)
                            .child("正在思考…"),
                    ),
            );
        }
        div()
            .id("conversation")
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(32.))
            .child(content)
    }

    fn render_permission(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let permission = self.permission.as_ref()?;
        let p = Paint::get(cx);
        let title = permission
            .tool_call
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("工具调用");
        let mut actions = row().gap(px(8.));
        for (id, name) in &permission.options {
            let option_id = id.clone();
            let button = if id == "allow-once" {
                Button::new(gpui::ElementId::Name(format!("permission-{id}").into())).primary()
            } else {
                Button::new(gpui::ElementId::Name(format!("permission-{id}").into())).secondary()
            };
            actions = actions.child(
                button
                    .label(name.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.respond_permission(option_id.clone(), cx);
                    }))
                    .into_themed(cx),
            );
        }
        Some(
            column()
                .w_full()
                .gap(px(10.))
                .p(px(12.))
                .rounded(px(10.))
                .border_1()
                .border_color(p.danger.opacity(0.45))
                .bg(composer_bg(cx))
                .child(
                    row()
                        .child(icon(Glyph::Alert, 17., p.danger))
                        .child(div().font_weight(FontWeight::MEDIUM).child("需要执行权限"))
                        .child(div().flex_1())
                        .child(note(title, cx)),
                )
                .child(actions),
        )
    }

    fn render_composer(&mut self, w: &mut Window, cx: &mut Context<Self>, hero: bool) -> gpui::Div {
        let p = Paint::get(cx);
        let usage = self.usage.map(|(used, size)| {
            if size > 0 {
                format!("{} / {} tokens", used, size)
            } else {
                format!("{used} tokens")
            }
        });
        let empty = self.composer.read(cx).value().trim().is_empty();
        let composer_width = self.composer_width(w);
        let model = self.current_config_label("model");
        let reasoning = self.current_config_label("reasoning_effort");
        column()
            .relative()
            .when(!hero, |element| {
                element.w(px(composer_width)).max_w_full().mx_auto()
            })
            .when(hero, |element| element.w_full())
            .flex_none()
            .gap(px(6.))
            .when(!hero, |element| element.pt(px(4.)).pb(px(4.)))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.composer_menu.is_some() {
                    this.composer_menu = None;
                    cx.notify();
                }
            }))
            .children(self.render_permission(cx))
            .when_some(self.error.clone(), |element, error| {
                element.child(
                    row()
                        .w_full()
                        .p(px(8.))
                        .gap(px(8.))
                        .child(icon(Glyph::Alert, 15., p.danger))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(p.danger)
                                .child(error),
                        )
                        .child(icon_btn(
                            "dismiss-error",
                            "关闭错误",
                            Glyph::Close,
                            cx,
                            |this, _, cx| {
                                this.error = None;
                                cx.notify();
                            },
                        )),
                )
            })
            .child(
                column()
                    .relative()
                    .w_full()
                    .gap(px(12.))
                    .pt(px(8.))
                    .rounded(px(28.))
                    .border(px(0.5))
                    .border_color(p.line)
                    .bg(composer_bg(cx))
                    .shadow_sm()
                    .child(
                        div()
                            .w_full()
                            .h(if hero { px(52.) } else { px(36.) })
                            .pl(px(14.))
                            .pr(px(8.))
                            .child(self.composer.clone()),
                    )
                    .child(
                        row()
                            .w_full()
                            .min_h(px(40.))
                            .px(px(8.))
                            .pt(px(2.))
                            .pb(px(6.))
                            .gap(px(12.))
                            .child(
                                div()
                                    .id("composer-command")
                                    .role(gpui::Role::Button)
                                    .aria_label("调用指令")
                                    .tab_index(0)
                                    .size(px(28.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .bg(p.hover)
                                    .hover(|style| style.bg(p.hover))
                                    .child(icon(Glyph::Plus, 14., p.ink))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_composer_menu(ComposerMenu::Commands);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                row()
                                    .id("composer-permission")
                                    .role(gpui::Role::ComboBox)
                                    .aria_label("审批模式：工作区内修改")
                                    .tab_index(0)
                                    .h(px(28.))
                                    .max_w(px(220.))
                                    .pl(px(8.))
                                    .pr(px(4.))
                                    .gap(px(4.))
                                    .rounded(px(8.))
                                    .cursor_pointer()
                                    .font_weight(FontWeight::MEDIUM)
                                    .hover(|style| style.bg(p.hover))
                                    .child(permission_icon("workspace-write", 14., p.secondary))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(13.))
                                            .line_height(px(20.))
                                            .text_color(p.secondary)
                                            .child("工作区内修改"),
                                    )
                                    .child(icon(
                                        if self.composer_menu == Some(ComposerMenu::Permission) {
                                            Glyph::Up
                                        } else {
                                            Glyph::Down
                                        },
                                        11.,
                                        p.faint,
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_composer_menu(ComposerMenu::Permission);
                                        cx.notify();
                                    })),
                            )
                            .child(div().flex_1().min_w(px(8.)))
                            .children(model.map(|label| {
                                row()
                                    .id("composer-model")
                                    .role(gpui::Role::ComboBox)
                                    .aria_label("选择模型")
                                    .tab_index(0)
                                    .h(px(28.))
                                    .max_w(px(360.))
                                    .pl(px(8.))
                                    .pr(px(4.))
                                    .gap(px(4.))
                                    .rounded(px(8.))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(p.hover))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(13.))
                                            .line_height(px(20.))
                                            .text_color(p.secondary)
                                            .child(label),
                                    )
                                    .children(reasoning.map(|effort| {
                                        div()
                                            .max_w(px(100.))
                                            .truncate()
                                            .text_size(px(13.))
                                            .line_height(px(20.))
                                            .text_color(p.faint)
                                            .child(effort)
                                    }))
                                    .child(icon(Glyph::Down, 11., p.faint))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_composer_menu(ComposerMenu::Model);
                                        cx.notify();
                                    }))
                            }))
                            .child(self.render_send_button(empty, cx)),
                    )
                    .when_some(self.composer_menu, |element, menu| {
                        element.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .h_0()
                                .child(self.render_composer_menu(menu, w, cx)),
                        )
                    }),
            )
            .when_some(usage, |element, usage| {
                element.child(
                    row()
                        .w_full()
                        .justify_center()
                        .min_h(px(20.))
                        .child(note(usage, cx)),
                )
            })
    }

    fn render_send_button(&self, empty: bool, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let blue = if dark(cx) {
            gpui::rgb(0x7aaaff)
        } else {
            gpui::rgb(0x4176e6)
        };
        let ink = gpui::rgb(0xffffff);
        let enabled = self.running
            || (self.connected()
                && !self.creating_session
                && !empty
                && self.active_session.is_some());
        let mut button = div()
            .id(if self.running {
                "cancel-prompt"
            } else {
                "send-prompt"
            })
            .role(gpui::Role::Button)
            .aria_label(if self.running { "停止" } else { "发送" })
            .tab_index(0)
            .tab_stop(enabled)
            .size(px(34.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(blue)
            .text_color(ink);
        button = if self.running {
            button.child(div().size(px(12.)).rounded(px(2.)).bg(ink))
        } else {
            button.child(icon(Glyph::ArrowUp, 18., ink.into()))
        };
        if enabled {
            button = button
                .cursor_pointer()
                .hover(move |style| style.bg(blue.opacity(0.86)));
            if self.running {
                button.on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
            } else {
                button.on_click(cx.listener(|this, _, w, cx| this.send_prompt(w, cx)))
            }
        } else {
            button.opacity(0.4).cursor_default()
        }
    }

    fn render_composer_menu(
        &self,
        menu: ComposerMenu,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let composer_width = self.composer_width(w);
        let model_width = (composer_width - 62.).clamp(240., 360.);
        let model_height = popup_max_height(w, 360.);
        match menu {
            ComposerMenu::Model => composer_menu_card("model-menu", model_height, cx)
                .right(px(54.))
                .w(px(model_width))
                .child(self.model_root_row("模型", "model", ComposerMenu::ModelChoices, cx))
                .when_some(
                    self.current_config_label("reasoning_effort"),
                    |element, label| {
                        element.child(self.model_root_row(
                            "思考强度",
                            &label,
                            ComposerMenu::ReasoningChoices,
                            cx,
                        ))
                    },
                ),
            ComposerMenu::ModelChoices => {
                composer_menu_card("model-choices-menu", model_height, cx)
                    .right(px(54.))
                    .w(px(model_width))
                    .overflow_y_scroll()
                    .child(self.model_back_row(cx))
                    .child(self.render_config_choices("model", cx))
            }
            ComposerMenu::ReasoningChoices => {
                composer_menu_card("reasoning-choices-menu", model_height, cx)
                    .right(px(54.))
                    .w(px(model_width.min(280.)))
                    .overflow_y_scroll()
                    .child(self.render_config_choices("reasoning_effort", cx))
            }
            ComposerMenu::Permission => composer_menu_card("permission-menu", model_height, cx)
                .left(px(40.))
                .min_w(px(164.))
                .w(px(176.))
                .gap_0()
                .child(self.permission_option("read-only", "仅可查看", cx))
                .child(self.permission_option("workspace-write", "工作区内修改", cx))
                .child(self.permission_option("danger-full-access", "完全权限", cx)),
            ComposerMenu::Commands => {
                composer_menu_card("commands-menu", popup_max_height(w, 400.), cx)
                    .left_0()
                    .right_0()
                    .gap_0()
                    .overflow_y_scroll()
                    .child(command_section_title("添加", false, cx))
                    .child(self.command_row("file", "文件", "", Glyph::Link, true, cx))
                    .child(self.command_row(
                        "goal",
                        "目标",
                        "设置或查看长期任务目标",
                        Glyph::Check,
                        false,
                        cx,
                    ))
                    .child(self.command_row(
                        "plan",
                        "计划",
                        "进入或退出计划模式",
                        Glyph::Card,
                        false,
                        cx,
                    ))
                    .child(self.command_row(
                        "feedback",
                        "反馈",
                        "发送关于当前会话的反馈",
                        Glyph::Comment,
                        false,
                        cx,
                    ))
                    .child(command_section_title("指令", true, cx))
                    .child(self.command_row(
                        "compact",
                        "压缩",
                        "压缩以上对话内容",
                        Glyph::Redo,
                        false,
                        cx,
                    ))
                    .child(self.command_row(
                        "permission",
                        "权限",
                        "切换权限预设（沙箱模式与审批策略）",
                        Glyph::Lock,
                        true,
                        cx,
                    ))
                    .child(self.command_row(
                        "model",
                        "模型",
                        "选择本会话使用的模型",
                        Glyph::Card,
                        true,
                        cx,
                    ))
                    .child(self.command_row(
                        "export",
                        "下载日志",
                        "将当前会话内容导出为 ZIP",
                        Glyph::Download,
                        false,
                        cx,
                    ))
            }
            ComposerMenu::Files => {
                let files = self.visible_files();
                let mut list = column().w_full().gap_0().child(command_section_title(
                    "文件与文件夹",
                    false,
                    cx,
                ));
                if self.file_loading && self.file_entries.is_empty() {
                    list = list
                        .child(file_status_row("正在读取工作区…", cx))
                        .child(file_status_row("正在读取工作区…", cx));
                }
                if let Some(error) = &self.file_error {
                    list = list.child(file_status_row(error, cx));
                }
                for (index, file) in files.iter().cloned().enumerate() {
                    list = list.child(self.file_row(file, index, index == self.file_selected, cx));
                }
                if !self.file_loading && self.file_error.is_none() && files.is_empty() {
                    list = list.child(file_status_row(
                        if self.file_filter.is_empty() {
                            "此目录为空"
                        } else {
                            "没有匹配的文件或文件夹"
                        },
                        cx,
                    ));
                }
                if self.file_truncated {
                    list = list.child(file_status_row("仅显示前 200 项，可继续输入以筛选", cx));
                }
                composer_menu_card("files-menu", popup_max_height(w, 400.), cx)
                    .left_0()
                    .right_0()
                    .gap_0()
                    .when(self.file_show_crumbs, |element| {
                        element.child(self.file_breadcrumbs(cx))
                    })
                    .child(
                        div()
                            .id("file-list")
                            .w_full()
                            .min_h_0()
                            .flex_1()
                            .overflow_y_scroll()
                            .child(list),
                    )
            }
        }
    }

    fn file_breadcrumbs(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut crumbs = row()
            .w_full()
            .flex_none()
            .gap(px(2.))
            .px(px(3.))
            .pt(px(3.))
            .pb(px(5.))
            .mb(px(2.))
            .border_b_1()
            .border_color(Paint::get(cx).line)
            .child(self.file_crumb("工作区".to_string(), String::new(), false, cx));
        let mut directory = String::new();
        let parts = self
            .file_directory
            .split('/')
            .filter(|part| !part.is_empty());
        let count = parts.clone().count();
        for (index, part) in parts.enumerate() {
            if !directory.is_empty() {
                directory.push('/');
            }
            directory.push_str(part);
            crumbs = crumbs
                .child(
                    div()
                        .flex_none()
                        .child(icon(Glyph::Arrow, 10., Paint::get(cx).faint)),
                )
                .child(self.file_crumb(
                    part.to_string(),
                    directory.clone(),
                    index + 1 == count,
                    cx,
                ));
        }
        crumbs
    }

    fn file_crumb(
        &self,
        label: String,
        directory: String,
        current: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        let crumb = div()
            .id(gpui::ElementId::Name(
                format!("file-crumb-{directory}").into(),
            ))
            .role(gpui::Role::Button)
            .aria_label(label.clone())
            .tab_index(0)
            .tab_stop(!current)
            .h(px(20.))
            .max_w(px(180.))
            .px(px(5.))
            .rounded(px(4.))
            .text_size(px(11.))
            .line_height(px(16.))
            .text_color(if current { p.ink } else { p.faint })
            .truncate()
            .child(label);
        if current {
            crumb
        } else {
            crumb
                .cursor_pointer()
                .hover(|style| style.bg(p.hover).text_color(p.ink))
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.drill_file_directory(directory.clone(), w, cx);
                }))
        }
    }

    fn file_row(
        &self,
        file: WorkspaceFile,
        index: usize,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        let path = file.path.clone();
        let directory_path = file.path.clone();
        let is_directory = file.is_directory;
        let mut item = row()
            .id(gpui::ElementId::Name(
                format!("file-option-{}", file.path).into(),
            ))
            .role(gpui::Role::Button)
            .aria_label(file.name.clone())
            .aria_selected(selected)
            .w_full()
            .h(px(34.))
            .flex_none()
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(13.))
            .line_height(px(20.))
            .when(selected, |element| element.bg(p.hover))
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if this.file_selected != index {
                    this.file_selected = index;
                    cx.notify();
                }
            }))
            .child(icon(
                if is_directory {
                    Glyph::Folder
                } else {
                    Glyph::FileText
                },
                14.,
                p.secondary,
            ))
            .child(div().flex_1().min_w_0().truncate().child(if is_directory {
                format!("{}/", file.name)
            } else {
                file.name.clone()
            }))
            .on_click(cx.listener(move |this, _, w, cx| {
                this.insert_file_reference(path.clone(), is_directory, w, cx);
            }));
        if is_directory {
            item = item.child(
                row()
                    .flex_none()
                    .gap(px(3.))
                    .when(selected, |element| {
                        element
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .line_height(px(16.))
                                    .text_color(p.faint)
                                    .child("进入目录"),
                            )
                            .child(
                                div()
                                    .h(px(16.))
                                    .px(px(4.))
                                    .rounded(px(4.))
                                    .bg(p.hover)
                                    .text_size(px(10.))
                                    .line_height(px(16.))
                                    .text_color(p.faint)
                                    .child("Tab"),
                            )
                    })
                    .child(
                        div()
                            .id(gpui::ElementId::Name(
                                format!("file-drill-{directory_path}").into(),
                            ))
                            .role(gpui::Role::Button)
                            .aria_label("进入目录")
                            .tab_index(0)
                            .size(px(18.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .hover(|style| style.bg(p.hover))
                            .child(icon(Glyph::Arrow, 12., p.secondary))
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _, w, cx| {
                                    cx.stop_propagation();
                                    this.drill_file_directory(directory_path.clone(), w, cx);
                                }),
                            ),
                    ),
            );
        }
        item
    }

    fn model_root_row(
        &self,
        label: &'static str,
        value: &str,
        target: ComposerMenu,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        let value = value.to_string();
        row()
            .id(gpui::ElementId::Name(format!("model-root-{label}").into()))
            .w_full()
            .h(px(34.))
            .px(px(8.))
            .gap(px(8.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|style| style.bg(p.hover))
            .child(div().text_size(px(13.)).child(label))
            .child(div().flex_1())
            .child(
                div()
                    .max_w(px(170.))
                    .truncate()
                    .text_size(px(13.))
                    .text_color(p.secondary)
                    .child(value),
            )
            .child(icon(Glyph::Arrow, 12., p.faint))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.composer_menu = Some(target);
                cx.notify();
            }))
    }

    fn model_back_row(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        row()
            .id("model-menu-back")
            .w_full()
            .h(px(30.))
            .px(px(8.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|style| style.bg(p.hover))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(p.secondary)
                    .child("‹ 模型与思考强度"),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.composer_menu = Some(ComposerMenu::Model);
                cx.notify();
            }))
    }

    fn render_config_choices(&self, config_id: &str, cx: &mut Context<Self>) -> gpui::Div {
        let p = Paint::get(cx);
        let mut list = column().w_full().gap(px(2.));
        let Some(option) = self
            .config_options
            .iter()
            .find(|option| option.get("id").and_then(Value::as_str) == Some(config_id))
        else {
            return list.child(note("当前会话没有可选项", cx));
        };
        let current = option.get("currentValue").and_then(Value::as_str);
        for item in option
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(group) = item.get("options").and_then(Value::as_array) {
                list = list.child(
                    div()
                        .h(px(26.))
                        .px(px(8.))
                        .pt(px(4.))
                        .text_size(px(11.))
                        .text_color(p.faint)
                        .child(
                            item.get("name")
                                .or_else(|| item.get("group"))
                                .and_then(Value::as_str)
                                .unwrap_or("模型")
                                .to_string(),
                        ),
                );
                for choice in group.iter().filter_map(picker_choice) {
                    list = list.child(self.config_choice_row(config_id, choice, current, cx));
                }
            } else if let Some(choice) = picker_choice(item) {
                list = list.child(self.config_choice_row(config_id, choice, current, cx));
            }
        }
        list
    }

    fn config_choice_row(
        &self,
        config_id: &str,
        choice: PickerOption,
        current: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        let config_id = config_id.to_string();
        let value = choice.value.to_string();
        let selected = current == Some(value.as_str());
        row()
            .id(gpui::ElementId::Name(
                format!("{config_id}-choice-{value}").into(),
            ))
            .w_full()
            .h(px(34.))
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|style| style.bg(p.hover))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .child(choice.label),
            )
            .when(selected, |element| {
                element.child(icon(Glyph::Check, 14., p.ink))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.composer_menu = None;
                this.set_config(config_id.clone(), value.clone());
                cx.notify();
            }))
    }

    fn permission_option(
        &self,
        id: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        let selected = id == "workspace-write";
        let full_access = id == "danger-full-access";
        let option = row()
            .id(gpui::ElementId::Name(format!("permission-{id}").into()))
            .role(gpui::Role::Button)
            .aria_label(if selected {
                format!("{label}，当前审批模式")
            } else if full_access {
                format!("{label}，需要确认风险")
            } else {
                format!("{label}，当前 ACP 连接不支持切换")
            })
            .aria_selected(selected)
            .tab_index(0)
            .tab_stop(selected || full_access)
            .w_full()
            .h(px(34.))
            .flex_none()
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(8.))
            .text_size(px(13.))
            .line_height(px(20.))
            .child(permission_icon(id, 14., p.secondary))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when(selected, |element| {
                element.child(icon(Glyph::Check, 14., p.ink))
            });
        if full_access {
            option
                .cursor_pointer()
                .hover(|style| style.bg(p.hover))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.composer_menu = None;
                    this.confirm_full_access = true;
                    this.full_access_acknowledged = false;
                    cx.notify();
                }))
        } else {
            option
        }
    }

    fn command_row(
        &self,
        command: &'static str,
        title: &'static str,
        description: &'static str,
        glyph: Glyph,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        let availability = if enabled { "" } else { "，ACP 暂不支持" };
        let mut item = row()
            .id(gpui::ElementId::Name(format!("command-{command}").into()))
            .role(gpui::Role::Button)
            .aria_label(format!("{title} {command}，{description}{availability}"))
            .tab_index(0)
            .tab_stop(enabled)
            .w_full()
            .h(px(34.))
            .flex_none()
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(8.))
            .text_size(px(13.))
            .line_height(px(20.))
            .child(icon(glyph, 14., p.secondary))
            .child(
                div()
                    .max_w(gpui::relative(0.4))
                    .flex_none()
                    .truncate()
                    .child(title),
            )
            .child(
                div()
                    .max_w(gpui::relative(0.2))
                    .flex_none()
                    .truncate()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(p.faint)
                    .child(command),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_align(gpui::TextAlign::Right)
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(p.faint)
                    .child(description),
            );
        if enabled {
            item = item
                .cursor_pointer()
                .hover(|style| style.bg(p.hover))
                .on_click(cx.listener(move |this, _, w, cx| {
                    match command {
                        "file" => this.open_file_picker(w, cx),
                        "model" => this.composer_menu = Some(ComposerMenu::Model),
                        "permission" => this.composer_menu = Some(ComposerMenu::Permission),
                        _ => {}
                    }
                    cx.notify();
                }));
        } else {
            item = item.opacity(0.45).cursor_default();
        }
        item
    }

    fn current_config_label(&self, id: &str) -> Option<String> {
        let option = self
            .config_options
            .iter()
            .find(|option| option.get("id").and_then(Value::as_str) == Some(id))?;
        let current = option.get("currentValue").and_then(Value::as_str)?;
        config_choices(option)
            .into_iter()
            .find(|choice| choice.value.as_ref() == current)
            .map(|choice| choice.label.to_string())
            .or_else(|| Some(current.to_string()))
    }

    fn render_account_menu(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let signed_in = self.welcome.read(cx).signed_in();
        let busy = self.welcome.read(cx).busy;
        let left = 12.;
        column()
            .id("account-menu")
            .absolute()
            .left(px(left))
            .bottom(px(56.))
            .w(px(240.))
            .gap(px(2.))
            .p(px(4.))
            .rounded(px(16.))
            .bg(menu_bg(cx))
            .shadow_lg()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.show_account_menu = false;
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.show_account_menu = false;
                    cx.notify();
                }
            }))
            .child(account_menu_row(
                "account-menu-settings",
                "设置",
                Glyph::Settings,
                true,
                cx,
                |this, _, cx| {
                    this.show_account_menu = false;
                    this.settings_page = SettingsPage::Account;
                    this.show_settings = true;
                    invoke("auth_profile", json!({}));
                    cx.notify();
                },
            ))
            .child(account_menu_row(
                "account-menu-auth",
                if signed_in { "退出登录" } else { "登录" },
                if signed_in {
                    Glyph::LogOut
                } else {
                    Glyph::User
                },
                !busy,
                cx,
                move |this, _, cx| {
                    this.show_account_menu = false;
                    this.welcome.update(cx, |welcome, cx| {
                        if signed_in {
                            welcome.busy = true;
                            invoke("sign_out", json!({}));
                            cx.notify();
                        } else {
                            welcome.start_sign_in(cx);
                        }
                    });
                    cx.notify();
                },
            ))
    }

    fn render_settings(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::get(cx);
        let account_controls = self
            .welcome
            .update(cx, |welcome, cx| welcome.render_inline(cx));
        let (status, detail) = match self.status {
            ServiceStatus::Starting => ("正在连接", "正在启动 ACP 服务"),
            ServiceStatus::Connected => ("服务已连接", self.agent_name.as_str()),
            ServiceStatus::Error => ("连接异常", "请检查路径后重新连接"),
        };
        let page = self.settings_page;
        let content: gpui::Div = match page {
            SettingsPage::Account => settings_section(
                "账号与余额",
                column()
                    .w_full()
                    .gap(px(18.))
                    .child(self.render_account_summary(cx))
                    .child(account_controls),
                cx,
            ),
            SettingsPage::General => settings_section(
                "通用设置",
                column()
                    .w_full()
                    .gap(px(14.))
                    .child(note("外观设置立即生效。", cx))
                    .child(
                        Button::new("toggle-theme")
                            .label(if dark(cx) {
                                "切换为浅色主题"
                            } else {
                                "切换为深色主题"
                            })
                            .with_icon(if dark(cx) { Glyph::Sun } else { Glyph::Moon }, false)
                            .secondary()
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_theme(cx)))
                            .into_themed(cx),
                    )
                    .child(div().h(px(1.)).w_full().bg(p.line))
                    .child(labeled_control(
                        "工作区绝对路径",
                        Some(input(&self.workspace_editor, cx)),
                        cx,
                    ))
                    .child(labeled_control(
                        "dsh-acp 服务目录",
                        Some(input(&self.service_editor, cx)),
                        cx,
                    ))
                    .child(
                        Button::new("connect-service")
                            .label("应用并重新连接")
                            .secondary()
                            .loading(self.status == ServiceStatus::Starting)
                            .disabled(self.running || self.creating_session)
                            .on_click(cx.listener(|this, _, w, cx| this.start_service(w, cx)))
                            .into_themed(cx),
                    )
                    .child(note(format!("{status} · {detail}"), cx)),
                cx,
            ),
            SettingsPage::Model => settings_section(
                "模型与推理",
                column()
                    .w_full()
                    .gap(px(14.))
                    .child(note("选择当前 ACP 会话使用的模型和思考强度。", cx))
                    .child(labeled_control(
                        "模型",
                        self.model_select
                            .clone()
                            .map(|select| div().w_full().child(select)),
                        cx,
                    ))
                    .child(labeled_control(
                        "推理强度",
                        self.reasoning_select
                            .clone()
                            .map(|select| div().w_full().child(select)),
                        cx,
                    )),
                cx,
            ),
            SettingsPage::Plugins => settings_section(
                "内置插件",
                column()
                    .w_full()
                    .gap(px(14.))
                    .child(note(
                        "当前 ACP 连接不提供内置插件配置清单。插件主面板可以通过真实会话检查环境，不会显示虚构的开关。",
                        cx,
                    ))
                    .child(
                        Button::new("settings-open-plugins")
                            .label("打开插件面板")
                            .secondary()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_settings = false;
                                this.open_page(WorkspacePage::Plugins, cx);
                            }))
                            .into_themed(cx),
                    ),
                cx,
            ),
            SettingsPage::AgentPresets => settings_section(
                "Agent 预设",
                column()
                    .w_full()
                    .gap(px(12.))
                    .child(note(
                        "当前 ACP 服务没有提供 Agent 预设读取或切换接口。工具执行仍会在会话中逐项请求审批。",
                        cx,
                    )),
                cx,
            ),
        };
        let nav = [
            (SettingsPage::Account, "账号与余额", Glyph::User),
            (SettingsPage::General, "通用设置", Glyph::Settings),
            (SettingsPage::Model, "模型", Glyph::Card),
            (SettingsPage::Plugins, "内置插件", Glyph::Bulb),
            (SettingsPage::AgentPresets, "Agent 预设", Glyph::Archive),
        ];
        let mut nav_list = column().w_full().gap(px(4.));
        for (target, label, glyph) in nav {
            let active = page == target;
            nav_list = nav_list.child(
                row()
                    .id(gpui::ElementId::Name(format!("settings-{label}").into()))
                    .w_full()
                    .h(px(40.))
                    .px(px(12.))
                    .gap(px(8.))
                    .rounded(px(12.))
                    .cursor_pointer()
                    .when(active, |element| element.bg(p.hover))
                    .hover(|style| style.bg(p.hover))
                    .child(icon(glyph, 16., p.secondary))
                    .child(div().text_size(px(14.)).child(label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_page = target;
                        cx.notify();
                    })),
            );
        }
        div()
            .id("settings-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(24.))
            .occlude()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.show_settings = false;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .id("settings-mask")
                    .absolute()
                    .inset_0()
                    .bg(if dark(cx) {
                        gpui::rgba(0x00000080)
                    } else {
                        gpui::rgba(0x0000003d)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_settings = false;
                        cx.notify();
                    })),
            )
            .child(
                row()
                    .id("settings-panel")
                    .relative()
                    .w_full()
                    .max_w(px(800.))
                    .h_full()
                    .max_h(px(800.))
                    .flex_none()
                    .gap_0()
                    .overflow_hidden()
                    .rounded(px(28.))
                    .bg(composer_bg(cx))
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        column()
                            .w(px(188.))
                            .h_full()
                            .flex_none()
                            .gap(px(10.))
                            .px(px(12.))
                            .pb(px(12.))
                            .child(
                                div()
                                    .h(px(54.))
                                    .flex_none()
                                    .pt(px(22.))
                                    .pl(px(12.))
                                    .text_size(px(16.))
                                    .line_height(px(24.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("设置"),
                            )
                            .child(nav_list),
                    )
                    .child(
                        column()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .gap_0()
                            .child(
                                row()
                                    .w_full()
                                    .h(px(54.))
                                    .flex_none()
                                    .justify_end()
                                    .px(px(14.))
                                    .pt(px(14.))
                                    .child(icon_btn(
                                        "close-settings",
                                        "关闭设置",
                                        Glyph::Close,
                                        cx,
                                        |this, _, cx| {
                                            this.show_settings = false;
                                            cx.notify();
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .id("settings-content")
                                    .flex_1()
                                    .min_h_0()
                                    .w_full()
                                    .overflow_y_scroll()
                                    .px(px(24.))
                                    .pb(px(24.))
                                    .child(content),
                            ),
                    ),
            )
    }

    fn render_account_summary(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = Paint::get(cx);
        let (signed_in, name, contact, balance, bonus) = {
            let welcome = self.welcome.read(cx);
            let profile = welcome.profile.as_ref();
            (
                welcome.signed_in(),
                profile
                    .and_then(|profile| profile.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or(if welcome.signed_in() {
                        "已登录"
                    } else {
                        "未登录"
                    })
                    .to_string(),
                profile
                    .and_then(|profile| profile.get("contact"))
                    .and_then(Value::as_str)
                    .unwrap_or(if welcome.signed_in() {
                        "DeepSeek 账户"
                    } else {
                        "登录后查看账户资料与余额"
                    })
                    .to_string(),
                welcome.balance.as_ref().and_then(|balance| {
                    format_wallets(balance.get("value").and_then(Value::as_array))
                }),
                welcome.balance.as_ref().and_then(|balance| {
                    format_wallets(balance.get("bonusWallets").and_then(Value::as_array))
                }),
            )
        };
        column()
            .w_full()
            .gap(px(16.))
            .child(
                row()
                    .w_full()
                    .min_h(px(78.))
                    .p(px(16.))
                    .gap(px(12.))
                    .rounded(px(16.))
                    .bg(p.stage)
                    .child(account_avatar(
                        self.welcome.read(cx).avatar.as_deref(),
                        36.,
                        cx,
                    ))
                    .child(
                        column()
                            .flex_1()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(name),
                            )
                            .child(note(contact, cx)),
                    )
                    .when(signed_in, |element| {
                        element.child(
                            Button::new("account-usage-info")
                                .label("更多账号信息")
                                .ghost()
                                .on_click(cx.listener(|_, _, _, _| {
                                    invoke("open_account_page", json!({"page": "usage"}));
                                }))
                                .into_themed(cx),
                        )
                    }),
            )
            .child(
                column()
                    .w_full()
                    .gap(px(14.))
                    .p(px(16.))
                    .rounded(px(16.))
                    .bg(p.stage)
                    .child(
                        row()
                            .w_full()
                            .child(div().flex_1().text_size(px(14.)).child("充值余额"))
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(balance.unwrap_or_else(|| "暂不可用".to_string())),
                            ),
                    )
                    .when_some(bonus, |element, bonus| {
                        element.child(
                            row()
                                .w_full()
                                .child(div().flex_1().text_size(px(14.)).child("赠送余额"))
                                .child(div().text_size(px(14.)).child(bonus)),
                        )
                    })
                    .child(div().h(px(1.)).w_full().bg(p.line))
                    .child(
                        row()
                            .w_full()
                            .child(note("更多", cx))
                            .child(div().flex_1())
                            .child(
                                Button::new("account-open-usage")
                                    .label("查询用量")
                                    .secondary()
                                    .disabled(!signed_in)
                                    .on_click(cx.listener(|_, _, _, _| {
                                        invoke("open_account_page", json!({"page": "usage"}));
                                    }))
                                    .into_themed(cx),
                            )
                            .child(
                                Button::new("account-open-top-up")
                                    .label("充值")
                                    .primary()
                                    .disabled(!signed_in)
                                    .on_click(cx.listener(|_, _, _, _| {
                                        invoke("open_account_page", json!({"page": "topUp"}));
                                    }))
                                    .into_themed(cx),
                            ),
                    ),
            )
    }

    fn brand_mark(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
        size: f32,
    ) -> gpui::Stateful<gpui::Div> {
        let hover = self.brand_hover.get(w, cx).clamp(0., 1.);
        div()
            .id(gpui::ElementId::Name(format!("brand-{size}").into()))
            .flex_none()
            .on_hover(cx.listener(|this, inside, _, cx| {
                this.brand_hover.set(if *inside { 1. } else { 0. }, cx);
                cx.notify();
            }))
            .child(
                svg()
                    .data(include_bytes!("../../assets/deepseek-fish.svg"))
                    .w(px(size))
                    .h(px(size * 17.04 / 23.16))
                    .flex_none()
                    .text_color(Paint::get(cx).ink)
                    .with_transformation(
                        Transformation::default()
                            .with_scaling(gpui::size(1. + 0.035 * hover, 1. + 0.035 * hover))
                            .with_rotation(gpui::radians(-0.075 * hover)),
                    ),
            )
    }
}

fn format_wallets(wallets: Option<&Vec<Value>>) -> Option<String> {
    let text = wallets?
        .iter()
        .filter_map(|wallet| {
            let amount = wallet.get("balance")?.as_str()?;
            let symbol = match wallet.get("currency")?.as_str()? {
                "CNY" => "¥",
                "USD" => "$",
                _ => return None,
            };
            Some(format!("{symbol}{amount}"))
        })
        .collect::<Vec<_>>()
        .join(" · ");
    (!text.is_empty()).then_some(text)
}

fn account_menu_row(
    id: &'static str,
    label: &'static str,
    glyph: Glyph,
    enabled: bool,
    cx: &mut Context<DshApp>,
    action: impl Fn(&mut DshApp, &mut Window, &mut Context<DshApp>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let p = Paint::get(cx);
    let row = row()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .tab_index(0)
        .tab_stop(enabled)
        .w_full()
        .h(px(36.))
        .px(px(10.))
        .gap(px(10.))
        .rounded(px(8.))
        .text_size(px(14.))
        .child(icon(glyph, 16., p.secondary))
        .child(label);
    if enabled {
        row.cursor_pointer()
            .hover(|style| style.bg(p.hover))
            .on_click(cx.listener(move |this, _, w, cx| action(this, w, cx)))
    } else {
        row.opacity(0.45).cursor_default()
    }
}

fn view_option_label(label: &'static str, cx: &App) -> gpui::Div {
    div()
        .h(px(21.))
        .flex_none()
        .px(px(8.))
        .pt(px(3.))
        .text_size(px(11.))
        .line_height(px(15.))
        .text_color(Paint::get(cx).faint)
        .child(label)
}

fn command_section_title(label: &'static str, separated: bool, cx: &App) -> gpui::Div {
    div()
        .h(px(23.))
        .flex_none()
        .px(px(8.))
        .pt(px(5.))
        .pb(px(2.))
        .when(separated, |element| element.mt(px(3.)))
        .text_size(px(11.))
        .line_height(px(16.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(Paint::get(cx).faint)
        .child(label)
}

fn file_status_row(label: &str, cx: &App) -> gpui::Div {
    row()
        .w_full()
        .h(px(34.))
        .flex_none()
        .px(px(8.))
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(Paint::get(cx).faint)
        .child(label.to_string())
}

fn composer_menu_card(id: &'static str, max_height: f32, cx: &App) -> gpui::Stateful<gpui::Div> {
    column()
        .id(id)
        .absolute()
        .bottom(px(4.))
        .min_w(px(240.))
        .max_h(px(max_height))
        .gap(px(2.))
        .p(px(4.))
        .rounded(px(16.))
        .bg(menu_bg(cx))
        .shadow_lg()
        .occlude()
}

fn popup_max_height(w: &Window, preferred: f32) -> f32 {
    (f32::from(w.viewport_size().height) - 96.)
        .max(160.)
        .min(preferred)
}

fn labeled_control(label: &'static str, control: Option<gpui::Div>, cx: &App) -> gpui::Div {
    let missing = control.is_none();
    column()
        .w_full()
        .gap(px(6.))
        .child(note(label, cx))
        .children(control)
        .when(missing, |element| {
            element.child(note("会话就绪后可配置", cx))
        })
}

fn settings_section(label: &'static str, content: impl IntoElement, cx: &App) -> gpui::Div {
    column()
        .w_full()
        .gap(px(18.))
        .child(
            div()
                .text_size(px(20.))
                .line_height(px(28.))
                .font_weight(FontWeight::MEDIUM)
                .child(label),
        )
        .child(content)
        .child(div().h(px(1.)).w_full().bg(Paint::get(cx).line))
}

fn render_entry(entry: &ChatEntry, expanded: bool, cx: &mut Context<DshApp>) -> gpui::Div {
    let p = Paint::get(cx);
    match entry.role {
        EntryRole::User => row().w_full().justify_end().child(
            div()
                .max_w(gpui::relative(0.702))
                .px(px(16.))
                .py(px(10.))
                .rounded(px(20.))
                .bg(if dark(cx) {
                    gpui::rgb(0x2c2c2e)
                } else {
                    gpui::rgb(0xedf3fe)
                })
                .text_size(px(14.))
                .line_height(px(22.))
                .child(entry.text.clone()),
        ),
        EntryRole::Assistant => div()
            .w_full()
            .text_size(px(14.))
            .line_height(px(24.))
            .text_color(p.ink)
            .child(entry.text.clone()),
        EntryRole::Thought => {
            let key = entry.key.clone();
            let summary = short_text(&entry.text, 72);
            column()
                .w_full()
                .gap_0()
                .child(
                    row()
                        .id(gpui::ElementId::Name(format!("entry-{key}").into()))
                        .role(gpui::Role::Button)
                        .aria_label(if expanded {
                            "收起思考过程"
                        } else {
                            "展开思考过程"
                        })
                        .aria_expanded(expanded)
                        .tab_index(0)
                        .w_full()
                        .h(px(24.))
                        .gap(px(6.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .text_color(p.faint)
                        .hover(|style| style.text_color(p.secondary))
                        .child(icon(Glyph::Bulb, 14., p.faint))
                        .child(div().flex_none().text_size(px(13.)).child("思考过程"))
                        .when(!expanded && !summary.is_empty(), |element| {
                            element
                                .child(div().size(px(2.)).mx(px(2.)).rounded_full().bg(p.faint))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(13.))
                                        .child(summary),
                                )
                        })
                        .child(icon(
                            if expanded { Glyph::Up } else { Glyph::Down },
                            12.,
                            p.faint,
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            toggle_entry(&mut this.expanded_entries, &key);
                            cx.notify();
                        })),
                )
                .when(expanded, |element| {
                    element.child(
                        div()
                            .pl(px(22.))
                            .py(px(4.))
                            .text_size(px(13.))
                            .line_height(px(24.))
                            .text_color(p.faint)
                            .child(entry.text.clone()),
                    )
                })
        }
        EntryRole::Tool => {
            let status = match entry.status.as_str() {
                "in_progress" => "正在运行",
                "completed" => "已完成",
                "failed" => "执行失败",
                "approval" => "等待授权",
                _ => &entry.status,
            };
            let title = if entry.title.is_empty() {
                "工具调用".to_string()
            } else {
                short_text(&entry.title, 52)
            };
            let summary = if entry.text.is_empty() {
                status.to_string()
            } else {
                short_text(&entry.text, 64)
            };
            let key = entry.key.clone();
            column()
                .w_full()
                .gap_0()
                .child(
                    row()
                        .id(gpui::ElementId::Name(format!("entry-{key}").into()))
                        .role(gpui::Role::Button)
                        .aria_label(format!("{title}，{status}"))
                        .aria_expanded(expanded)
                        .tab_index(0)
                        .w_full()
                        .h(px(24.))
                        .gap(px(6.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .text_color(if entry.status == "failed" {
                            p.danger
                        } else {
                            p.faint
                        })
                        .hover(|style| style.text_color(p.secondary))
                        .child(icon(Glyph::Settings, 14., p.faint))
                        .child(div().flex_none().text_size(px(13.)).child(title))
                        .child(div().size(px(2.)).mx(px(2.)).rounded_full().bg(p.faint))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.))
                                .child(summary),
                        )
                        .child(icon(
                            if expanded { Glyph::Up } else { Glyph::Down },
                            12.,
                            p.faint,
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            toggle_entry(&mut this.expanded_entries, &key);
                            cx.notify();
                        })),
                )
                .when(expanded, |element| {
                    let mut detail = column().w_full().pl(px(4.)).py(px(4.)).gap(px(6.));
                    if !entry.input.is_empty() {
                        detail = detail.child(code_text(&entry.input, p));
                    }
                    if !entry.text.is_empty() {
                        detail = detail.child(code_text(&entry.text, p));
                    }
                    element.child(detail)
                })
        }
        EntryRole::System => row()
            .w_full()
            .justify_center()
            .child(note(entry.text.clone(), cx)),
        EntryRole::Error => row()
            .w_full()
            .gap(px(8.))
            .child(icon(Glyph::Alert, 16., p.danger))
            .child(div().text_color(p.danger).child(entry.text.clone())),
    }
}

fn toggle_entry(expanded: &mut std::collections::HashSet<String>, key: &str) {
    if !expanded.remove(key) {
        expanded.insert(key.to_string());
    }
}

fn code_text(text: &str, p: Paint) -> gpui::Div {
    div()
        .w_full()
        .px(px(16.))
        .py(px(12.))
        .rounded(px(16.))
        .border(px(0.5))
        .border_color(p.line)
        .bg(p.stage)
        .font_family("monospace")
        .text_size(px(12.))
        .line_height(px(20.))
        .text_color(p.secondary)
        .child(text.to_string())
}

fn conversation_axis(w: &Window, collapsed: bool, sidebar_width: f32) -> f32 {
    let main_width =
        f32::from(w.viewport_size().width) - if collapsed { 0. } else { sidebar_width };
    (main_width * 0.64)
        .clamp(680., 920.)
        .min((main_width - 64.).max(240.))
}

fn composer_axis(w: &Window, collapsed: bool, sidebar_width: f32) -> f32 {
    let main_width =
        f32::from(w.viewport_size().width) - if collapsed { 0. } else { sidebar_width };
    (conversation_axis(w, collapsed, sidebar_width) + 32.).min((main_width - 32.).max(272.))
}

fn permission_icon(id: &str, size: f32, color: gpui::Hsla) -> gpui::Svg {
    let data: &'static [u8] = match id {
        "read-only" => include_bytes!("../../assets/permission-read-only.svg"),
        "workspace-write" => include_bytes!("../../assets/permission-workspace-write.svg"),
        "danger-full-access" => include_bytes!("../../assets/permission-full-access.svg"),
        _ => include_bytes!("../../assets/permission-read-only.svg"),
    };
    svg()
        .data(data)
        .size(px(size))
        .flex_none()
        .text_color(color)
}

fn account_avatar(avatar: Option<&str>, size: f32, cx: &App) -> gpui::Div {
    let p = Paint::get(cx);
    let content = if let Some(data) = avatar.filter(|data| data.starts_with("data:image/")) {
        gpui::img(data.to_string())
            .w_full()
            .h_full()
            .into_any_element()
    } else {
        icon(Glyph::User, size * 0.62, p.secondary).into_any_element()
    };
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .rounded_full()
        .bg(p.hover)
        .child(content)
}

fn static_brand_mark(size: f32, color: gpui::Hsla) -> gpui::Svg {
    svg()
        .data(include_bytes!("../../assets/deepseek-fish.svg"))
        .w(px(size))
        .h(px(size * 17.04 / 23.16))
        .flex_none()
        .text_color(color)
}

pub(super) fn begin_window_drag() {
    invoke("start_window_dragging", json!({}));
}

pub(super) fn dark(cx: &App) -> bool {
    Theme::global(cx).mode == ThemeMode::Dark
}

fn sidebar_bg(cx: &App) -> gpui::Hsla {
    if dark(cx) {
        gpui::rgb(0x1b1b1c).into()
    } else {
        Paint::get(cx).stage
    }
}

pub(super) fn conversation_bg(cx: &App) -> gpui::Hsla {
    if dark(cx) {
        gpui::rgb(0x151517).into()
    } else {
        Paint::get(cx).panel
    }
}

pub(super) fn composer_bg(cx: &App) -> gpui::Hsla {
    if dark(cx) {
        gpui::rgb(0x2c2c2e).into()
    } else {
        Paint::get(cx).panel
    }
}

pub(super) fn menu_bg(cx: &App) -> gpui::Hsla {
    if dark(cx) {
        gpui::rgb(0x303136).into()
    } else {
        gpui::rgb(0xf8f9fa).into()
    }
}

pub(super) fn path_name(path: &str) -> String {
    if path.is_empty() {
        return "选择工作区".to_string();
    }
    if path
        .chars()
        .all(|character| matches!(character, '/' | '\\'))
    {
        return "默认工作区".to_string();
    }
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod workspace_name_tests {
    #[test]
    fn workspace_names_are_friendly_and_consistent() {
        assert_eq!(super::path_name("/"), "默认工作区");
        assert_eq!(super::path_name("/work/aster/"), "aster");
        assert_eq!(super::path_name(""), "选择工作区");
    }
}

fn short_text(text: &str, limit: usize) -> String {
    let single_line = text.lines().next().unwrap_or(text).trim();
    let mut chars = single_line.chars();
    let prefix = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix.to_string()
    }
}
