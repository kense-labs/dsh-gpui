//! Harness-shaped workspace chrome over the ACP-backed application state.

use super::header_chrome::{MENU_GAP, MENU_MARGIN, menu_origin};
use super::menu_groups::{MenuHeading, heading_space};
use super::menu_surface::{menu_elevation, menu_material};
use super::session_context_menu::{
    SessionMenuAnchor, SessionMenuOrigin, mac_control_click, session_menu_gestures,
};
use super::*;
use gpui::{AvailableSpace, canvas, point, size, svg};
use kui::primitives::ops_surface::icon_btn;

const HERO_WORKSPACE_HEIGHT: f32 = 28.;
const HERO_COMPOSER_GAP: f32 = 8.;

fn workspace_menu_height(
    anchor: gpui::Bounds<gpui::Pixels>,
    viewport_height: gpui::Pixels,
) -> gpui::Pixels {
    let clearance = px(MENU_GAP + MENU_MARGIN);
    let above = anchor.top() - clearance;
    let below = viewport_height - anchor.bottom() - clearance;
    above.max(below)
}

impl DshApp {
    pub(super) fn render_workspace(&mut self, w: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let material = bridge::has_mac_window_material();
        // First-run API Key configuration can be deferred.
        if self.show_welcome {
            return div()
                .relative()
                .size_full()
                .child(self.welcome.clone())
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(42.))
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, _| begin_window_drag()),
                );
        }
        let rail = self.rail_active(w);
        let drawer = self.sidebar_drawer;
        let animate = self.sidebar_slide.is_some() && !cx.reduce_motion();
        let (sidebar_progress, sliding) = self
            .sidebar_slide
            .get_or_insert_with(DockSlide::default)
            .update(!rail, bridge::monotonic_ms(), animate);
        if sliding {
            w.request_animation_frame();
        }
        let sidebar = (!drawer || sidebar_progress > 0.)
            .then(|| self.render_sidebar(w, sidebar_progress, cx));
        let (sidebar_column, sidebar_overlay) = if drawer {
            (None, sidebar)
        } else {
            (sidebar, None)
        };
        let mut main = column()
            .flex_1()
            .h_full()
            .min_w_0()
            .gap_0()
            .bg(conversation_bg(cx))
            .when(material, |main| {
                main.border_l(px(0.5)).border_color(p.border_l3)
            });
        if self.page != WorkspacePage::Plugins {
            self.close_plugin_detail();
        }
        match self.page {
            WorkspacePage::Conversation => {
                main = main.child(self.render_header(w, cx));
                let approval = self.conversation.permission.is_some();
                let plan = !self.conversation.plan_reviews.is_empty();
                let question = self.conversation.visible_question().is_some();
                if self.conversation.entries.is_empty()
                    && self
                        .conversation
                        .assistant_stream
                        .entries()
                        .next()
                        .is_none()
                    && !approval
                    && !plan
                    && self.conversation.questions.is_empty()
                {
                    main = main.child(self.render_hero(w, cx));
                } else {
                    let rail_items = crate::turns::turn_items(
                        self.conversation
                            .entries
                            .iter()
                            .chain(self.conversation.assistant_stream.entries())
                            .filter(|entry| !entry.is_subcall()),
                    );
                    let rail =
                        (!rail_items.is_empty()).then(|| self.render_turn_rail(&rail_items, w, cx));
                    let conversation = self.render_conversation(w, cx);
                    let mut overlay = div()
                        .relative()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .child(conversation);
                    if let Some(rail) = rail {
                        overlay = overlay.child(rail);
                    }
                    main = main.child(overlay);
                    main = if approval {
                        main.children(self.render_permission(w, cx))
                    } else if plan {
                        main.children(self.render_plan_review(self.composer_width(w), w, cx))
                    } else if question {
                        main.children(self.render_questions(self.composer_width(w), w, cx))
                    } else {
                        main.child(self.render_composer(w, cx, false))
                    };
                }
            }
            WorkspacePage::Plugins => {
                main = main.child(self.render_plugins_panel(w, cx));
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
            .when(!material, |root| root.bg(conversation_bg(cx)))
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
                        this.save_workspace_preferences(cx);
                        cx.notify();
                    }
                }),
            )
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, w, cx| {
                if event.keystroke.key == "escape"
                    && this.stats_dialog_bounds.get().is_some()
                    && this.dismiss_stats_dialog(cx)
                {
                    w.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, w, cx| {
                if event.keystroke.key != "escape" {
                    return;
                }
                if this.session_feedback.visible() {
                    this.dismiss_session_feedback(w, cx);
                    w.prevent_default();
                    cx.stop_propagation();
                } else if this.session_downloads.visible() {
                    this.dismiss_session_download(w, cx);
                    w.prevent_default();
                    cx.stop_propagation();
                } else if this.confirm_permission.is_some() {
                    this.close_permission_confirmation(w, cx);
                } else if this.header_menu.is_some() {
                    if !event.keystroke.modifiers.modified() {
                        if !event.is_held {
                            this.escape_header_menu(w, cx);
                        }
                        w.prevent_default();
                        cx.stop_propagation();
                    }
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
                } else if this.sidebar_drawer && this.narrow_sidebar_expanded {
                    this.set_left_sidebar_collapsed(true, w, cx);
                    w.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .child(
                row()
                    .size_full()
                    .gap_0()
                    .overflow_hidden()
                    .children(sidebar_column)
                    .when(!drawer && !rail, |element| {
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
                    .child(main)
                    .child(
                        div()
                            .h_full()
                            .w(px(self.right_dock_frame.track))
                            .bg(conversation_bg(cx))
                            .flex_none(),
                    ),
            )
            .when(drawer || (bridge::is_mac() && rail), |root| {
                root.child(self.render_leading_controls(w, cx))
            })
            .when(self.right_dock_frame.visible, |element| {
                element.child(self.render_start_panel(w, cx))
            })
            .when(
                self.right_panel_visible()
                    && self.right_dock_frame.visible
                    && !self.right_dock_frame.fullscreen,
                |element| {
                    element.child(self.render_dock_resize_handle(
                        super::start_panel::DockResize::Outer,
                        self.right_dock_frame.left,
                        self.right_dock_frame.width,
                        cx,
                    ))
                },
            )
            .when(
                self.floats_on_screen(cx) && !self.sidebar.surface.layout.floats().is_empty(),
                |element| element.child(self.render_float_panes(w, cx)),
            )
            .when(sidebar_overlay.is_some(), |element| {
                let mask = div()
                    .id("sidebar-drawer-mask")
                    .absolute()
                    .inset_0()
                    .bg(p.bg_mask_1.opacity(sidebar_progress));
                element.child(if rail {
                    mask
                } else {
                    mask.occlude().on_click(cx.listener(|this, _, w, cx| {
                        this.set_left_sidebar_collapsed(true, w, cx);
                    }))
                })
            })
            .children(sidebar_overlay)
            .when(self.show_more_menu, |element| {
                element.child(menu_in(
                    "sidebar-more-menu-in",
                    self.render_more_menu(cx).into_any_element(),
                ))
            })
            .when_some(
                self.workspace_context_menu.clone(),
                |element, (cwd, pointer_y)| {
                    element.child(menu_in(
                        "workspace-context-in",
                        self.render_workspace_context_menu(cwd, pointer_y, w, cx)
                            .into_any_element(),
                    ))
                },
            )
            .children(self.render_exiting_header_menu(cx))
            .children(self.render_open_session_context(cx))
            .child(self.header_menu_pointer_watch(cx))
            .children(self.render_dock_hints(w, cx))
            .children(self.render_dock_tab_menus(w, cx))
            .children(self.dock_menu_pointer_watch(cx))
            .children(self.render_terminal_shell_menu(w, cx))
            .when(
                self.workspace_rename.is_some()
                    || self.workspace_delete.is_some()
                    || self.session_rename.is_some(),
                |element| {
                    element.child(modal_in(
                        "workspace-dialog-in",
                        self.render_workspace_dialog(w, cx).into_any_element(),
                    ))
                },
            )
            .when(self.confirm_permission.is_some(), |element| {
                element.child(modal_in(
                    "full-access-in",
                    self.render_full_access_dialog(w, cx).into_any_element(),
                ))
            })
            .when(self.show_settings, |element| {
                element.child(modal_in(
                    "settings-in",
                    self.render_settings(w, cx).into_any_element(),
                ))
            })
            .when(self.shortcuts.open, |element| {
                element.child(modal_in(
                    "shortcuts-in",
                    self.render_shortcut_reference(w, cx),
                ))
            })
            .children(
                self.render_remove_confirmation(cx)
                    .map(|dialog| modal_in("plugin-remove-in", dialog.into_any_element())),
            )
            .children(
                self.render_plugin_installer(w, cx)
                    .map(|dialog| modal_in("plugin-install-in", dialog.into_any_element())),
            )
            .when(!self.toasts.is_empty(), |element| {
                element.child(self.render_toasts(cx))
            })
            .children(self.render_session_download(w, cx))
            .children(self.render_session_feedback(w, cx))
            .children(self.render_session_feedback_notice(w, cx))
            .children(self.render_archive_confirmation(cx))
            .children(self.render_session_markdown_notice(w, cx))
            .when(self.drop_active, |element| {
                let p = dsh(cx);
                element.child(
                    div()
                        .id("drop-overlay")
                        .absolute()
                        .inset_0()
                        .bg(p.bg_mask_1)
                        .flex()
                        .items_center()
                        .justify_center()
                        .occlude()
                        .child(
                            column()
                                .w(px(360.))
                                .gap(px(10.))
                                .items_center()
                                .rounded(px(crate::tokens::Tokens::RADIUS_XL))
                                .border(px(0.5))
                                .border_color(p.line)
                                .bg(composer_bg(cx))
                                .shadow(elevation_prominent())
                                .px(px(28.))
                                .py(px(24.))
                                .child(
                                    div()
                                        .text_size(px(16.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child("放到此处以添加附件"),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .line_height(px(18.))
                                        .text_color(p.faint)
                                        .child("支持图片与常见文档；单张图片预览上限 4MB"),
                                ),
                        ),
                )
            })
    }

    fn render_global_panel(
        &mut self,
        panel: impl IntoElement,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let _ = (w, cx);
        div().relative().flex_1().min_h_0().w_full().child(panel)
    }

    fn render_sidebar(
        &mut self,
        w: &mut Window,
        progress: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        let drawer = self.sidebar_drawer;
        let viewport = f32::from(w.viewport_size().width);
        let expanded_width = if drawer {
            self.sidebar_width.min((viewport - 48.).max(0.))
        } else {
            self.sidebar_width
        };
        let collapsed = collapsed_sidebar_width();
        let width = if drawer {
            expanded_width
        } else {
            collapsed + (expanded_width - collapsed) * progress
        };
        let content_width = (expanded_width - 16.).max(0.);

        let query = self.search_editor.read(cx).value().trim().to_lowercase();
        let mut filtered = self
            .sessions
            .iter()
            .filter(|session| {
                // 官方：移除 Workspace 分组后，它的 Session 仍留在 Ungrouped 下。
                // 真实 cwd 保留在 SessionSummary 上供 ACP resume 使用，只在
                // 分组时把隐藏路径映射到空 cwd。
                (match self.workspace_archived_filter {
                    WorkspaceArchivedFilter::Hide => !self.archived_sessions.contains(&session.id),
                    WorkspaceArchivedFilter::Only => self.archived_sessions.contains(&session.id),
                    WorkspaceArchivedFilter::Show => true,
                }) && (query.is_empty()
                    || self
                        .session_display_title(session)
                        .to_lowercase()
                        .contains(&query)
                    || session.cwd.to_lowercase().contains(&query)
                    || self
                        .workspace_name(&session.cwd)
                        .to_lowercase()
                        .contains(&query))
            })
            .cloned()
            .collect::<Vec<_>>();
        // 官方：非空白查询以单一扁平列表呈现，最多 20 条结果。
        if !query.is_empty() {
            filtered.truncate(20);
        }
        // 官方排序：置顶会话稳定排在组内最前；「最近更新」在置顶/普通分区内
        // 严格按最近一次用户提示词或 steering 时间降序，置顶时间不影响它；
        // 「手动排序」沿用服务端完整序列（sort_by 是稳定排序）。
        let pinned = &self.pinned_sessions;
        let activity = &self.session_activity;
        let order_by = self.workspace_order_by;
        filtered.sort_by(|a, b| {
            let a_pinned = pinned.contains(&a.id);
            let b_pinned = pinned.contains(&b.id);
            b_pinned.cmp(&a_pinned).then_with(|| {
                if order_by == WorkspaceOrderBy::Updated {
                    let at = |session: &crate::sessions::SessionSummary| {
                        activity
                            .get(&session.id)
                            .and_then(|item| item.last_prompt_at_ms)
                            .max(session.created_at_ms)
                            .unwrap_or(0)
                    };
                    at(b).cmp(&at(a))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
        });
        // 官方：当前选中的空白「新会话」在首条提示词落地前保留临时首位。
        let pinned_count = filtered
            .iter()
            .take_while(|session| pinned.contains(&session.id))
            .count();
        if let Some(active) = self.active_session.as_deref()
            && let Some(index) = filtered
                .iter()
                .position(|session| session.id == active && session.blank)
            && index > pinned_count
        {
            let session = filtered.remove(index);
            filtered.insert(pinned_count, session);
        }
        let filtered_count = filtered.len();
        for session in &filtered {
            self.session_row_focus
                .entry(session.id.clone())
                .or_insert_with(|| cx.focus_handle());
            if !session.blank {
                self.session_row_more_focus
                    .entry(session.id.clone())
                    .or_insert_with(|| cx.focus_handle().tab_stop(true));
            }
        }
        // 官方：非空白查询用单一扁平结果列表替代任一浏览模式。
        let grouping = if query.is_empty() {
            self.workspace_group_by
        } else {
            WorkspaceGroupBy::Flat
        };
        let mut session_groups = column().w_full().gap(px(match grouping {
            WorkspaceGroupBy::Workspace | WorkspaceGroupBy::WorkspaceTree => 4.,
            WorkspaceGroupBy::Flat => 2.,
        }));
        let mut empty = filtered.is_empty();
        match grouping {
            WorkspaceGroupBy::Flat => {
                for session in filtered {
                    session_groups = session_groups.child(self.render_session_row(session, w, cx));
                }
            }
            WorkspaceGroupBy::Workspace | WorkspaceGroupBy::WorkspaceTree => {
                let mut groups: Vec<(String, Vec<SessionSummary>)> = Vec::new();
                for session in filtered {
                    // 已移除的 Workspace 分组：Session 归入 Ungrouped。
                    let group_cwd = if self.hidden_workspaces.contains(&session.cwd) {
                        String::new()
                    } else {
                        session.cwd.clone()
                    };
                    if let Some((_, sessions)) =
                        groups.iter_mut().find(|(cwd, _)| cwd == &group_cwd)
                    {
                        sessions.push(session);
                    } else {
                        groups.push((group_cwd, vec![session]));
                    }
                }
                if self.workspace_archived_filter != WorkspaceArchivedFilter::Only {
                    for cwd in &self.added_workspaces {
                        if !self.hidden_workspaces.contains(cwd)
                            && !groups.iter().any(|(path, _)| path == cwd)
                            && (query.is_empty()
                                || cwd.to_lowercase().contains(&query)
                                || self.workspace_name(cwd).to_lowercase().contains(&query))
                        {
                            groups.push((cwd.clone(), Vec::new()));
                        }
                    }
                }
                empty = groups.is_empty();
                for (cwd, sessions) in groups {
                    session_groups =
                        session_groups.child(self.render_workspace_group(cwd, sessions, w, cx));
                }
            }
        }
        if empty {
            session_groups =
                session_groups.child(self.render_empty_sessions(!query.is_empty(), cx));
        }
        let wide_opacity = progress;
        let material = !drawer && bridge::has_mac_window_material();
        let reduced_transparency = material && bridge::reduces_transparency();
        let dark = dark(cx);
        let tint = match (material, reduced_transparency, dark) {
            (false, _, _) => p.sidebar_fill,
            (_, true, _) => p.sidebar_fill.opacity(0.9),
            (_, _, true) => p.sidebar_fill.opacity(0.5),
            _ => gpui::Rgba {
                r: 245.19 / 255.,
                g: 247.15 / 255.,
                b: 250.67 / 255.,
                a: 0.4,
            }
            .into(),
        };
        let mut sidebar = column()
            .relative()
            .w(px(width))
            .h_full()
            .flex_none()
            .overflow_hidden()
            .bg(tint)
            .when(drawer, |sidebar| {
                sidebar
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(-width * (1. - progress)))
                    .occlude()
            })
            .when(!material && !drawer, |sidebar| {
                sidebar.border_r(px(0.5)).border_color(p.border_l3)
            })
            .when(material && !reduced_transparency, |sidebar| {
                let top = gpui::rgb(0x7a9bf0).opacity(if dark { 0.08 } else { 0.1 });
                let bottom = gpui::rgb(0x8f89b8).opacity(if dark { 0.07 } else { 0.09 });
                sidebar
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(gpui::relative(0.35))
                            .bg(gpui::linear_gradient(
                                180.,
                                gpui::linear_color_stop(top, 0.),
                                gpui::linear_color_stop(top.opacity(0.), 1.),
                            )),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .h(gpui::relative(0.32))
                            .bg(gpui::linear_gradient(
                                180.,
                                gpui::linear_color_stop(bottom.opacity(0.), 0.),
                                gpui::linear_color_stop(bottom, 1.),
                            )),
                    )
            });
        if progress > 0.001 {
            sidebar = sidebar.child(
                column()
                    .id("sidebar-wide")
                    .when(drawer, |sidebar| {
                        sidebar
                            .track_focus(&self.sidebar_drawer_focus.scope)
                            .tab_stop(false)
                    })
                    .w(px(expanded_width))
                    .h_full()
                    .gap(px(4.))
                    .px(px(8.))
                    .py(px(6.))
                    .opacity(if drawer { 1. } else { wide_opacity })
                    .child(
                        row()
                            .w(px(content_width))
                            .h(px(52.))
                            .flex_none()
                            .mt(px(-6.))
                            .justify_end()
                            .pb(px(if bridge::is_mac() { 2. } else { 0. }))
                            .pr(px(if bridge::is_mac() && !self.shell_fullscreen(w) {
                                0.
                            } else {
                                4.
                            }))
                            .when(bridge::is_mac() && self.shell_fullscreen(w), |strip| {
                                strip
                                    .pl(px(4.))
                                    .child(self.sidebar_toggle_button(true, false, cx))
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .h_full()
                                    .on_mouse_down(gpui::MouseButton::Left, |_, _, _| {
                                        begin_window_drag()
                                    }),
                            )
                            .when(!bridge::is_mac() || !self.shell_fullscreen(w), |strip| {
                                strip.child(self.sidebar_toggle_button(true, false, cx))
                            }),
                    )
                    .child(
                        row()
                            .w(px(content_width))
                            .h(px(60.))
                            .flex_none()
                            .mt(px(-16.))
                            .px(px(8.))
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, _| begin_window_drag())
                            .child(crate::brand::wordmark(24., 18., dark)),
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
                            .mt(px(6.))
                            .px(px(3.))
                            .child(self.render_workspace_navigation(cx)),
                    )
                    .child(self.render_workspace_search(content_width, filtered_count, w, cx))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .w(px(content_width))
                            .child(
                                div()
                                    .id("session-list")
                                    .h_full()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.sessions_scroll)
                                    .px(px(3.))
                                    .child(session_groups),
                            )
                            .child(scrollbar_overlay(
                                "session-scrollbar",
                                &self.sessions_scroll,
                                cx,
                            )),
                    )
                    .child(
                        row()
                            .id("sidebar-more-settings")
                            .role(gpui::Role::Button)
                            .aria_label("更多设置")
                            .when(drawer, |more| {
                                more.track_focus(&self.sidebar_drawer_focus.more)
                            })
                            .tab_index(0)
                            .w(px(content_width))
                            .h(px(44.))
                            .flex_none()
                            .px(px(9.))
                            .gap(px(8.))
                            .rounded(px(12.))
                            .cursor_pointer()
                            .hover(|style| style.bg(p.hover))
                            .child(settings_mark(24., cx))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(14.))
                                    .child("更多"),
                            )
                            .child(icon(Glyph::Up, 14., p.faint))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_more_menu = !this.show_more_menu;
                                this.show_workspace_menu = false;
                                this.show_workspace_view_options = false;
                                this.composer_menu = None;
                                cx.notify();
                            })),
                    ),
            );
        }
        if !drawer && !bridge::is_mac() && progress < 0.999 {
            let rail_opacity = 1. - progress;
            sidebar = sidebar.child(self.render_sidebar_rail(rail_opacity, cx));
        }
        sidebar
    }

    fn render_empty_sessions(&self, searching: bool, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let archived = self.workspace_archived_filter == WorkspaceArchivedFilter::Only;
        let (glyph, text): (&[u8], &str) = if archived && !searching {
            (
                include_bytes!("../../assets/workspace-empty-archived.svg"),
                "暂无已归档会话",
            )
        } else {
            (
                include_bytes!("../../assets/workspace-empty.svg"),
                if searching {
                    "无匹配结果"
                } else {
                    "暂无会话"
                },
            )
        };
        column()
            .w_full()
            .mt(px(80.))
            .px(px(12.))
            .items_center()
            .gap(px(8.))
            .text_size(px(13.))
            .line_height(px(20.))
            .text_color(p.label_tertiary)
            .child(
                div()
                    .mb(px(4.))
                    .child(svg().data(glyph).size(px(24.)).text_color(p.label_caption)),
            )
            .child(text)
            .when(archived && !searching, |empty| {
                empty.child(dock_action_button(
                    div()
                        .id("show-other-sessions")
                        .role(gpui::Role::Button)
                        .aria_label("查看其他会话")
                        .tab_index(0)
                        .cursor_pointer()
                        .text_color(p.link)
                        .child("查看其他会话"),
                    cx.listener(|this, _, _, cx| {
                        this.workspace_archived_filter = WorkspaceArchivedFilter::Hide;
                        this.save_workspace_preferences(cx);
                        cx.notify();
                    }),
                ))
            })
    }

    /// 折叠 rail（56px）：toggle、品牌标、新建、导航图标、底部头像。
    /// 入场自右 49px 滑入（官方 rail-in）。
    fn render_sidebar_rail(
        &mut self,
        opacity: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let enabled = self.connected() && !self.creating_session;
        let dx = 49. * (1. - opacity);
        let mut rail = column()
            .id("sidebar-rail")
            .absolute()
            .top_0()
            .left_0()
            .h_full()
            .w(px(SIDEBAR_RAIL_WIDTH))
            .pt(px(8.))
            .pb(px(6.))
            .gap(px(12.))
            .items_center()
            .opacity(opacity)
            .ml(px(dx))
            .child(self.sidebar_toggle_button(false, true, cx))
            .child(
                div()
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .child(crate::brand::mark(22., dark(cx))),
            )
            .child(
                div()
                    .id("rail-new-session")
                    .role(gpui::Role::Button)
                    .aria_label("新建会话")
                    .tab_index(0)
                    .tab_stop(enabled)
                    .size(px(36.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(12.))
                    .text_color(p.label_primary)
                    .when(enabled, |element| {
                        element
                            .cursor_pointer()
                            .hover(|style| style.bg(p.hover))
                            .on_click(cx.listener(|this, _, w, cx| this.new_session(w, cx)))
                    })
                    .when(!enabled, |element| element.opacity(0.4).cursor_default())
                    .child(icon(Glyph::Plus, 16., p.label_primary)),
            );
        for (page, label, glyph) in [
            (WorkspacePage::Plugins, "插件", Glyph::Bulb),
            (WorkspacePage::Automations, "自动化任务", Glyph::Clock),
            (WorkspacePage::More, "更多", Glyph::More),
        ] {
            let active = self.page == page;
            rail = rail.child(
                div()
                    .id(gpui::ElementId::Name(format!("rail-nav-{label}").into()))
                    .role(gpui::Role::Button)
                    .aria_label(label)
                    .aria_selected(active)
                    .tab_index(0)
                    .size(px(36.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(12.))
                    .cursor_pointer()
                    .when(active, |element| element.bg(p.hover))
                    .hover(|style| style.bg(p.hover))
                    .child(icon(glyph, 16., p.label_primary))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_page(page, cx);
                    })),
            );
        }
        rail.child(div().flex_1()).child(
            div()
                .id("rail-more-settings")
                .role(gpui::Role::Button)
                .aria_label("更多设置")
                .tab_index(0)
                .size(px(36.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(12.))
                .cursor_pointer()
                .hover(|style| style.bg(p.hover))
                .child(settings_mark(24., cx))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_more_menu = !this.show_more_menu;
                    this.show_workspace_menu = false;
                    this.show_workspace_view_options = false;
                    this.composer_menu = None;
                    cx.notify();
                })),
        )
    }

    pub(super) fn render_workspace_view_options(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = dsh(cx);
        let trigger_bounds = Rc::new(Cell::new(None::<gpui::Bounds<gpui::Pixels>>));
        let outside_bounds = trigger_bounds.clone();
        let card = column()
            .id("workspace-view-options-menu")
            .role(gpui::Role::Menu)
            .gap_0()
            .p(px(4.))
            .rounded(px(12.))
            .bg(menu_bg(cx))
            .shadow(elevation_prominent())
            .occlude()
            .on_mouse_down_out(
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    if !outside_bounds
                        .get()
                        .is_some_and(|bounds| bounds.contains(&event.position))
                    {
                        this.show_workspace_view_options = false;
                        cx.notify();
                    }
                }),
            )
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
            ));
        canvas(
            move |trigger, w, cx| {
                trigger_bounds.set(Some(trigger));
                let viewport = w.viewport_size();
                let margin = px(MENU_MARGIN);
                let max_width = viewport.width - margin * 2.;
                let max_height = workspace_menu_height(trigger, viewport.height);
                if max_width <= px(0.) || max_height <= px(0.) {
                    return;
                }
                let mut card = menu_in(
                    "view-options-in",
                    card.w(px(200.).min(max_width))
                        .max_h(max_height)
                        .overflow_y_scroll(),
                )
                .into_any_element();
                let measured = card.layout_as_root(
                    size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                    w,
                    cx,
                );
                w.defer_draw(
                    card,
                    menu_origin(trigger, measured, viewport, false),
                    4,
                    None,
                );
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
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
        let p = dsh(cx);
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
            .rounded(px(12.))
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
                this.save_workspace_preferences(cx);
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
            let p = dsh(cx);
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
    pub(super) fn sidebar_toggle_button(
        &self,
        collapse: bool,
        rail: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let button = div()
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
            .when(self.sidebar_drawer, |button| {
                button.track_focus(if collapse {
                    &self.sidebar_drawer_focus.close
                } else {
                    &self.sidebar_drawer_focus.opener
                })
            })
            .tab_index(0)
            .tab_stop(true)
            .when_some(
                self.shortcuts
                    .aria(crate::shortcuts::Command::ToggleSidebar),
                |button, aria| button.aria_keyshortcuts(aria),
            )
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            // 宽模式 28×28 r8；rail 36×36 r12（官方 .kB3qhq_iconButton 折叠态）。
            .when(rail, |element| element.size(px(36.)).rounded(px(12.)))
            .when(!rail, |element| element.size(px(28.)).rounded(px(8.)))
            .cursor_pointer()
            .hover(|style| style.bg(p.hover))
            .child(
                svg()
                    .data(include_bytes!("../../assets/sidebar-panel.svg"))
                    .size(px(16.))
                    .text_color(if rail { p.label_primary } else { p.secondary }),
            );
        super::dock_controls::dock_action_button(
            button,
            cx.listener(move |this, _, w, cx| this.set_left_sidebar_collapsed(collapse, w, cx)),
        )
    }

    fn new_session_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let enabled = self.connected() && !self.creating_session;
        let mut button = row()
            .id("new-session")
            .role(gpui::Role::Button)
            .aria_label("新会话")
            .tab_index(0)
            .tab_stop(enabled)
            .w_full()
            .h(px(38.))
            .flex_none()
            .justify_center()
            .gap(px(6.))
            .rounded(px(12.))
            .border(px(0.5))
            .border_color(p.border_l3)
            // 官方 darwin 覆盖：浅色 #ffffff8c / 深色 #ffffff26。
            .bg::<gpui::Hsla>(if dark(cx) {
                gpui::rgba(0xffffff26).into()
            } else {
                gpui::rgba(0xffffff8c).into()
            })
            .text_size(px(14.))
            .font_weight(FontWeight::MEDIUM)
            .child(icon(Glyph::Plus, 15., p.ink))
            .child("新会话");
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
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let selected = self.active_session.as_deref() == Some(session.id.as_str());
        let title = self.session_display_title(&session);
        let for_click = session.clone();
        let for_row_menu = session.id.clone();
        let for_rename = session.id.clone();
        let for_hover = session.id.clone();
        let for_pin = session.id.clone();
        let menu = self.session_context_for(&session.id);
        let menu_open = menu.is_some();
        let more_expanded =
            menu.is_some_and(|menu| matches!(menu.anchor, SessionMenuAnchor::Button(_)));
        let focus = self.session_row_focus.get(&session.id);
        let more_focus = self.session_row_more_focus.get(&session.id);
        let pointer_hover = bridge::has_pointer_hover();
        let revealed = menu_open
            || self.hovered_session.as_deref() == Some(session.id.as_str())
            || (w.last_input_was_keyboard()
                && focus.is_some_and(|focus| focus.contains_focused(w, cx)));
        let archived = self.archived_sessions.contains(&session.id);
        let pinned = !archived && self.pinned_sessions.contains(&session.id);
        let (status, subagents) = self.session_indicator(&session.id);
        let pending = matches!(
            status,
            SessionIndicator::Approval | SessionIndicator::PlanReview | SessionIndicator::Question
        );
        let visible_status =
            (!session.blank || pending) && !archived && status != SessionIndicator::Idle;
        let title_color = if archived {
            p.label_caption
        } else {
            p.label_primary
        };
        let time = match status {
            SessionIndicator::Approval => Some("待审批".to_owned()),
            SessionIndicator::PlanReview => Some("计划待审".to_owned()),
            SessionIndicator::Question => Some("待回答".to_owned()),
            _ if pointer_hover => self
                .session_activity
                .get(&session.id)
                .and_then(|activity| activity.last_prompt_at_ms)
                .max(session.created_at_ms)
                .map(|at| crate::sessions::relative_session_age(at, bridge::now_ms() as u64)),
            _ => None,
        };
        let describe = || {
            let status_text = match status {
                SessionIndicator::Idle => "空闲",
                SessionIndicator::Running => "进行中",
                SessionIndicator::Subagents(_) => return format!("{subagents} 个子智能体运行中"),
                SessionIndicator::Approval => "等待审批",
                SessionIndicator::PlanReview => "计划待审",
                SessionIndicator::Question => "待回答",
                SessionIndicator::Unread => "已完成",
            };
            if subagents > 0 {
                format!("{status_text}，{subagents} 个子智能体运行中")
            } else {
                status_text.to_owned()
            }
        };
        row()
            .id(gpui::ElementId::Name(session.id.clone().into()))
            .role(gpui::Role::TreeItem)
            .aria_selected(selected)
            .aria_label(title.clone())
            .when(visible_status, |row| row.aria_description(describe()))
            .when_some(focus, |row, focus| row.track_focus(focus).tab_index(0))
            .w_full()
            .h(px(32.))
            .flex_none()
            .gap_0()
            .pl(px(9.))
            .pr(px(8.))
            .rounded(px(12.))
            .cursor_pointer()
            .when(selected || menu_open, |element| element.bg(p.hover))
            .hover(|style| style.bg(p.hover))
            .when(pointer_hover, |row| {
                row.hover_listener_mode(gpui::HoverListenerMode::InputModalityIndependent)
                    .on_hover(cx.listener(move |this, inside: &bool, _, cx| {
                        let was_hovered =
                            this.hovered_session.as_deref() == Some(for_hover.as_str());
                        if *inside != was_hovered {
                            this.hovered_session = inside.then(|| for_hover.clone());
                            cx.notify();
                        }
                    }))
            })
            .when(!session.blank, |row| {
                session_menu_gestures(row, session.id.clone(), SessionMenuOrigin::Row, focus, cx)
            })
            .child(
                row()
                    .w(px(16.))
                    .h(px(20.))
                    .flex_none()
                    .justify_center()
                    .when(visible_status, |slot| match status {
                        SessionIndicator::Running | SessionIndicator::Subagents(_) => {
                            slot.child(crate::anim::state_spinner(14., 0., p.label_tertiary))
                        }
                        SessionIndicator::Approval
                        | SessionIndicator::PlanReview
                        | SessionIndicator::Question
                        | SessionIndicator::Unread => {
                            slot.child(div().size(px(6.)).rounded_full().bg(if pending {
                                p.state_warn_primary
                            } else {
                                p.state_success_primary
                            }))
                        }
                        SessionIndicator::Idle => slot,
                    }),
            )
            .child(
                div()
                    .id("session-row-title")
                    .flex_1()
                    .min_w_0()
                    .mx(px(6.))
                    .truncate()
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .text_color(title_color)
                    .child(title.clone())
                    .when(!session.blank, |title| {
                        title.on_click(cx.listener(move |this, event: &gpui::ClickEvent, w, cx| {
                            if !event.is_right_click()
                                && !mac_control_click(event.modifiers())
                                && event.click_count() == 2
                            {
                                cx.stop_propagation();
                                this.begin_session_rename(for_rename.clone(), w, cx);
                            }
                        }))
                    }),
            )
            .when(!session.blank && (!revealed || !pointer_hover), |element| {
                element.child(
                    row()
                        .gap(px(6.))
                        .flex_none()
                        .when_some(time, |tail, time| {
                            tail.child(
                                div()
                                    .flex_none()
                                    .mr(px(-1.))
                                    .text_size(px(10.))
                                    .line_height(px(16.))
                                    .text_color(p.label_tertiary)
                                    .child(time),
                            )
                        })
                        .when(pinned && pointer_hover, |tail| {
                            tail.child(
                                row().w(px(16.)).h(px(20.)).justify_center().child(
                                    svg()
                                        .data(include_bytes!("../../assets/session-pin-fill.svg"))
                                        .size(px(14.))
                                        .text_color(p.label_caption),
                                ),
                            )
                        }),
                )
            })
            .when(!session.blank && (revealed || !pointer_hover), |element| {
                let anchor = Rc::new(Cell::new(None));
                let button_focus = more_focus.cloned();
                let more = div()
                    .id(gpui::ElementId::Name(
                        format!("session-more-{}", session.id).into(),
                    ))
                    .group("session-row-more")
                    .relative()
                    .when_some(more_focus, |button, focus| button.track_focus(focus))
                    .role(gpui::Role::Button)
                    .aria_label(format!("会话“{title}”的操作"))
                    .aria_expanded(more_expanded)
                    .tab_index(0)
                    .size(px(24.))
                    .mx(px(-4.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_color(p.label_tertiary)
                    .hover(|style| style.text_color(p.label_primary))
                    .child(
                        svg()
                            .data(include_bytes!("../../assets/workspace-ellipsis.svg"))
                            .size(px(14.))
                            .text_color(p.label_tertiary)
                            .group_hover("session-row-more", |style| {
                                style.text_color(p.label_primary)
                            }),
                    )
                    .child(self.track_header_menu_button(Some(anchor.clone())))
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, w, cx| {
                        if !event.is_right_click()
                            && !mac_control_click(event.modifiers())
                            && let Some(bounds) = anchor.get()
                        {
                            this.open_session_context(
                                &for_row_menu,
                                SessionMenuOrigin::Row,
                                SessionMenuAnchor::Button(bounds),
                                button_focus.clone(),
                                w,
                                cx,
                            );
                        }
                    }));
                // 官方：置顶/取消置顶按钮位于「更多」之前。
                element.child(
                    row()
                        .flex_none()
                        .gap(px(13.))
                        .when(!archived && (pointer_hover || pinned), |actions| {
                            actions.child(
                                div()
                                    .id("session-row-pin")
                                    .group("session-row-pin")
                                    .role(gpui::Role::Button)
                                    .aria_label(if pinned {
                                        "取消置顶"
                                    } else {
                                        "置顶会话"
                                    })
                                    .tab_index(0)
                                    .size(px(24.))
                                    .mx(px(-4.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .text_color(p.label_tertiary)
                                    .hover(|style| style.text_color(p.label_primary))
                                    .child(
                                        svg()
                                            .size(px(14.))
                                            .text_color(p.label_tertiary)
                                            .group_hover("session-row-pin", |style| {
                                                style.text_color(p.label_primary)
                                            })
                                            .data(if pinned {
                                                include_bytes!("../../assets/session-pin-fill.svg")
                                                    .as_slice()
                                            } else {
                                                include_bytes!(
                                                    "../../assets/session-pin-outline.svg"
                                                )
                                                .as_slice()
                                            }),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_session_pin(&for_pin, cx);
                                    })),
                            )
                        })
                        .child(more),
                )
            })
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, w, cx| {
                if event.is_right_click() || mac_control_click(event.modifiers()) {
                    return;
                }
                if this.active_session.as_deref() == Some(for_click.id.as_str()) {
                    this.open_page(WorkspacePage::Conversation, cx);
                } else {
                    this.resume(for_click.clone(), w, cx);
                }
            }))
    }

    fn active_session_summary(&self) -> Option<&SessionSummary> {
        self.active_session
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|session| &session.id == id))
    }

    pub(super) fn conversation_header_is_blank(&self) -> bool {
        self.active_session_summary()
            .is_none_or(|session| session.blank)
            && self.conversation.entries.is_empty()
            && !self.conversation.running
            && self.conversation.permission.is_none()
            && self.conversation.plan_reviews.is_empty()
    }

    fn render_header(&mut self, w: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let session = self.active_session_summary();
        let blank = self.conversation_header_is_blank();
        let corner = !self.right_panel_visible();
        let left = if bridge::is_mac() && (self.sidebar_drawer || self.rail_active(w)) {
            if self.shell_fullscreen(w) { 84. } else { 160. }
        } else if self.sidebar_drawer {
            84.
        } else {
            20.
        };
        let title = if blank {
            String::new()
        } else {
            session
                .and_then(|session| self.session_title_name(session))
                .map(|title| short_text(title, 52))
                .or_else(|| {
                    self.conversation
                        .entries
                        .iter()
                        .find(|entry| entry.role == EntryRole::User)
                        .map(|entry| short_text(&entry.text, 52))
                })
                .unwrap_or_else(|| "已恢复的会话".to_string())
        };
        let title_menu_id = session
            .filter(|session| !session.blank)
            .map(|session| session.id.clone());
        let mut crumb = div()
            .id("session-title-crumb")
            .max_w_full()
            .min_w_0()
            .truncate()
            .text_size(px(14.))
            .line_height(px(20.))
            .font_weight(FontWeight::MEDIUM)
            .child(title);
        if let Some(id) = title_menu_id {
            crumb = session_menu_gestures(
                crumb.track_focus(&self.session_title_focus).tab_index(0),
                id,
                SessionMenuOrigin::Title,
                Some(&self.session_title_focus),
                cx,
            );
        }
        div()
            .relative()
            .w_full()
            .h(px(if blank { 40. } else { 50. }))
            .flex_none()
            .when(!blank, |element| {
                element.border_b(px(0.5)).border_color(p.border_l3)
            })
            .child(
                row()
                    .size_full()
                    .pl(px(left))
                    .pr(px(if corner { 12. } else { 28. }))
                    .pt(px(10.))
                    .when(!blank, |element| element.pb(px(10.)))
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .items_start()
                            .child(crumb)
                            .on_mouse_down(gpui::MouseButton::Left, |event, _, _| {
                                if !mac_control_click(event.modifiers) {
                                    begin_window_drag();
                                }
                            }),
                    )
                    .when(!blank && self.active_session.is_some(), |element| {
                        element.child(self.render_conversation_header_actions(cx))
                    })
                    .when(corner, |element| {
                        element.child(self.render_start_panel_toggle(false, cx))
                    }),
            )
    }

    fn composer_width(&self, w: &Window) -> f32 {
        let main_width = f32::from(w.viewport_size().width) - self.sidebar_track_width(w);
        let width =
            (crate::layout::conversation_axis(main_width) + 32.).min((main_width - 32.).max(272.));
        if self.right_dock_frame.track > 0. {
            width.min((main_width - self.right_dock_frame.track - 32.).max(240.))
        } else {
            width
        }
    }

    fn render_hero_workspace_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = dsh(cx);
        let anchor = Rc::new(Cell::new(None::<gpui::Bounds<gpui::Pixels>>));
        let outside_anchor = anchor.clone();
        let mut choices = column()
            .id("workspace-menu-choices")
            .w_full()
            .min_h_0()
            .gap(px(2.))
            .overflow_y_scroll();
        let mut workspaces = self
            .sessions
            .iter()
            .map(|session| session.cwd.clone())
            .chain(self.added_workspaces.iter().cloned())
            .filter(|path| !self.hidden_workspaces.contains(path))
            .collect::<Vec<_>>();
        if !self.workspace.is_empty() {
            workspaces.push(self.workspace.clone());
        }
        workspaces.sort();
        workspaces.dedup();
        let has_workspaces = !workspaces.is_empty();
        for workspace in workspaces {
            let selected = workspace == self.workspace;
            let label = path_name(&workspace);
            choices = choices.child(
                row()
                    .id(gpui::ElementId::Name(
                        format!("workspace-{workspace}").into(),
                    ))
                    .role(gpui::Role::MenuItem)
                    .aria_label(label.clone())
                    .tab_index(0)
                    .w_full()
                    .h(px(32.))
                    .flex_none()
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
                            .child(label),
                    )
                    .when(selected, |element| {
                        element.child(icon(Glyph::Check, 14., p.ink))
                    })
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.show_workspace_menu = false;
                        this.new_session_at(workspace.clone(), None, w, cx);
                        cx.notify();
                    })),
            );
        }
        let card = column()
            .id("workspace-menu")
            .role(gpui::Role::Menu)
            .gap(px(2.))
            .p(px(4.))
            .rounded(px(16.))
            .bg(menu_bg(cx))
            .shadow(elevation_prominent())
            .occlude()
            .on_mouse_down_out(
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    if !outside_anchor
                        .get()
                        .is_some_and(|bounds| bounds.contains(&event.position))
                    {
                        this.show_workspace_menu = false;
                        cx.notify();
                    }
                }),
            )
            .when(has_workspaces, |card| {
                card.child(choices).child(
                    div()
                        .h(px(0.5))
                        .flex_none()
                        .mx(px(4.))
                        .my(px(3.))
                        .bg(p.line),
                )
            })
            .child(dock_action_button(
                row()
                    .id("manage-workspace")
                    .role(gpui::Role::MenuItem)
                    .aria_label("添加工作区")
                    .tab_index(0)
                    .w_full()
                    .h(px(32.))
                    .flex_none()
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
                    ),
                cx.listener(|this, _, _, cx| {
                    this.show_workspace_menu = false;
                    this.request_workspace_picker();
                    cx.notify();
                }),
            ));
        canvas(
            move |trigger, w, cx| {
                anchor.set(Some(trigger));
                let viewport = w.viewport_size();
                let max_width = viewport.width - px(2. * MENU_MARGIN);
                let max_height = workspace_menu_height(trigger, viewport.height);
                if max_width <= px(0.) || max_height <= px(0.) {
                    return;
                }
                let mut card = card
                    .w(px(360.).min(max_width))
                    .max_h(px(330.).min(max_height))
                    .into_any_element();
                let measured = card.layout_as_root(
                    size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                    w,
                    cx,
                );
                w.defer_draw(
                    card,
                    menu_origin(trigger, measured, viewport, true),
                    4,
                    None,
                );
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    fn render_hero(&mut self, w: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let composer_width = self.composer_width(w);
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
                                            .font_family("Montserrat")
                                            .text_size(px(26.))
                                            .line_height(px(32.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child("探索未至之境"),
                                    )
                                    .child(
                                        div()
                                            .mt(px(1.))
                                            .px(px(7.))
                                            .rounded_full()
                                            .border(px(0.5))
                                            .border_color(p.hover)
                                            .bg(p.state_business_tertiary)
                                            .font_family("monospace")
                                            .text_size(px(12.))
                                            .line_height(px(18.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(p.label_primary_bluish)
                                            .child("预览版"),
                                    ),
                            ),
                    )
                    .when(
                        self.conversation.resumed_session
                            && self.conversation.entries.is_empty()
                            && !self.conversation.history_loading,
                        |element| {
                            element.child(row().w_full().justify_center().pb(px(4.)).child(note(
                                "会话已恢复。未找到可显示的历史记录，可从下方继续发送新任务。",
                                cx,
                            )))
                        },
                    )
                    .when(
                        self.conversation.resumed_session && self.conversation.history_loading,
                        |element| {
                            element.child(
                                row()
                                    .w_full()
                                    .justify_center()
                                    .pb(px(4.))
                                    .child(note("正在载入历史…", cx)),
                            )
                        },
                    )
                    .child(
                        column()
                            .relative()
                            .w_full()
                            .mt(px(4.))
                            .gap(px(HERO_COMPOSER_GAP))
                            .child(
                                row().w_full().pl(px(20.)).pr(px(16.)).gap(px(4.)).child(
                                    row()
                                        .relative()
                                        .min_w_0()
                                        .max_w(px(360.))
                                        .child(dock_action_button(
                                            row()
                                                .id("hero-workspace")
                                                .role(gpui::Role::Button)
                                                .aria_expanded(self.show_workspace_menu)
                                                .tab_index(0)
                                                .h(px(HERO_WORKSPACE_HEIGHT))
                                                .min_w_0()
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
                                                .child(icon(Glyph::Down, 12., p.faint)),
                                            cx.listener(|this, _, _, cx| {
                                                this.show_workspace_menu =
                                                    !this.show_workspace_menu;
                                                this.composer_menu = None;
                                                cx.notify();
                                            }),
                                        ))
                                        .when(self.show_workspace_menu, |anchor| {
                                            anchor.child(self.render_hero_workspace_menu(cx))
                                        }),
                                ),
                            )
                            .child(self.render_composer(w, cx, true)),
                    ),
            )
    }

    fn render_transcript_entry(
        &self,
        entry: &ChatEntry,
        tree: &crate::tool_calls::ToolTree<'_>,
        show_thought_preview: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let rendered = if entry.role == EntryRole::Tool
            && entry.tool_name() == "ask_user_question"
            && let Some(id) = entry.external_id.as_deref()
        {
            self.render_question_tool(id, Some(entry), cx)
        } else {
            render_entry(
                entry,
                self.conversation.expanded_entries.contains(&entry.key),
                self.copied_mark.as_deref(),
                &self.conversation.wrapped_code,
                self.content_font_delta,
                show_thought_preview,
                cx,
            )
        };
        if entry.role != EntryRole::Tool {
            return rendered;
        }
        let children = tree.children(entry.external_id.as_deref());
        if children.is_empty() {
            return rendered;
        }
        let mut branch = column()
            .mt(px(4.))
            .mb(px(2.))
            .ml(px(22.))
            .pl(px(8.))
            .border_l(px(0.5))
            .border_color(dsh(cx).border_l2)
            .gap(px(4.));
        for child in children {
            branch =
                branch.child(self.render_transcript_entry(child, tree, show_thought_preview, cx));
        }
        column()
            .w_full()
            .child(rendered)
            .child(div().w_full().child(branch))
    }

    fn render_conversation(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Self-heal the live turn clock before any immutable borrows of the
        // conversation are created (the tool tree below holds one).
        self.start_turn_ticker(cx);
        self.prepare_rating_rows(w, cx);
        let p = dsh(cx);
        let flow_rows = super::flow_scroll::FlowRows::default();
        let content_width = crate::layout::conversation_axis(
            f32::from(w.viewport_size().width) - self.sidebar_track_width(w),
        );
        let mut content = column()
            .w(px(content_width))
            .max_w_full()
            .mx_auto()
            .gap_0()
            .pt(px(16.))
            .pb(px(16.));
        if let Some(label) = self.subagent_view_label().map(str::to_owned) {
            content = content.child(self.render_subagent_banner(&label, cx));
        }
        if self.conversation.history_has_more || self.conversation.history_loading {
            let label = if self.conversation.history_loading {
                "载入历史…"
            } else {
                "加载更早"
            };
            content = content.child(
                row().w_full().mb(px(8.)).justify_center().child(
                    div()
                        .id("load-older-history")
                        .role(gpui::Role::Button)
                        .aria_label(label)
                        .tab_index(0)
                        .px(px(12.))
                        .py(px(4.))
                        .rounded(px(8.))
                        .bg(p.hover)
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(p.secondary)
                        .child(label)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.request_older_history(cx);
                        })),
                ),
            );
        }
        let tree = crate::tool_calls::ToolTree::new(&self.conversation.entries);
        let entries: Vec<_> = self
            .conversation
            .entries
            .iter()
            .chain(self.conversation.assistant_stream.entries())
            .filter(|entry| !entry.is_subcall())
            .collect();
        let layout = self.conversation.turn_process.layout(
            &entries,
            &self.conversation.turn_disclosures,
            crate::history::turn_process::Options {
                fold: self.work_details != WorkDetailsMode::Verbose,
                defer: self.collapse_next_input,
                keep_replies: self.keep_folded_replies,
            },
        );
        let now = bridge::monotonic_ms();
        let animate_folds = self.collapse_next_input
            && (self.conversation.running
                || self
                    .conversation
                    .last_turn_settled
                    .is_some_and(|settled| now - settled < super::flow_motion::SETTLED_MOTION_MS));
        self.conversation.flow_motion.begin_frame(
            animate_folds,
            cx.reduce_motion(),
            now,
            self.conversation.scroll.bounds(),
        );
        let active_turn_start = entries
            .iter()
            .rposition(|entry| entry.role == EntryRole::User)
            .unwrap_or(0);
        let compact = self.work_details == WorkDetailsMode::Compact;
        let delta = self.content_font_delta as f32;
        let mut index = 0;
        let mut open_process = false;
        let is_process =
            |entry: &ChatEntry| matches!(entry.role, EntryRole::Thought | EntryRole::Tool);
        let running_tool = self
            .conversation
            .running
            .then(|| {
                let turn = &entries[active_turn_start..];
                let start = turn
                    .iter()
                    .rposition(|entry| !is_process(entry))
                    .map_or(0, |index| index + 1);
                let mut tools = Vec::new();
                for &entry in turn[start..]
                    .iter()
                    .filter(|entry| entry.role == EntryRole::Tool)
                {
                    tree.append_branch(entry, &mut tools);
                }
                tools
                    .into_iter()
                    .filter(|tool| tool.is_running_tool())
                    .max_by(|a, b| {
                        a.at_ms
                            .unwrap_or_default()
                            .total_cmp(&b.at_ms.unwrap_or_default())
                    })
            })
            .flatten();
        let footer_answer =
            if !self.conversation.running && self.conversation.turn_summary.is_some() {
                self.conversation.turn_tails.latest().and_then(|message| {
                    self.conversation
                        .entries
                        .iter()
                        .rev()
                        .take_while(|entry| entry.role != EntryRole::User)
                        .find(|entry| {
                            entry.role == EntryRole::Assistant
                                && entry.external_id.as_deref() == Some(message)
                        })
                })
            } else {
                None
            };
        let mut previous_role = None;
        let mut visible_viewports = std::collections::HashSet::new();
        while index < entries.len() {
            if let Some(control) = layout.controls.get(&index).copied() {
                content = content.child(flow_rows.row(
                    format!("control-{}", control.number),
                    self.render_turn_process_control(control, previous_role.is_some(), cx),
                ));
                previous_role = Some(EntryRole::System);
            }
            let entry = entries[index];
            let hidden = layout.hidden(index);
            let process_member = layout.process_member(index);
            if hidden && !process_member {
                index += 1;
                continue;
            }
            let turn_live = self.conversation.running && index >= active_turn_start;
            let row_key = process_member.then(|| format!("turn-row-{}", entry.key));
            let closing_flat = hidden
                && animate_folds
                && row_key
                    .as_deref()
                    .is_some_and(|key| self.conversation.flow_motion.showed(key));
            let fold_process = match self.work_details {
                WorkDetailsMode::Verbose => false,
                WorkDetailsMode::Detailed => {
                    !closing_flat && !layout.flat(index).unwrap_or(turn_live)
                }
                WorkDetailsMode::Standard | WorkDetailsMode::Compact => true,
            };
            if fold_process && is_process(entry) {
                let start = index;
                index += 1;
                while index < entries.len()
                    && is_process(entries[index])
                    && layout.turn(index) == layout.turn(start)
                    && layout.hidden(index) == hidden
                    && !layout.controls.contains_key(&index)
                {
                    index += 1;
                }
                let process = &entries[start..index];
                let group_key = format!("process-{}", entry.key);
                let gap = match previous_role {
                    None => 0.,
                    Some(EntryRole::User | EntryRole::Assistant) => 12.,
                    _ => 4.,
                };
                let group_frame = self.conversation.flow_motion.update(
                    &format!("turn-group-{}", entry.key),
                    !hidden,
                    gap,
                    true,
                );
                if !group_frame.show {
                    self.conversation.expanded_entries.remove(&group_key);
                    for member in process {
                        self.conversation.expanded_entries.remove(&member.key);
                    }
                    continue;
                }
                let expanded = self.conversation.expanded_entries.contains(&group_key);
                let mut tools = Vec::new();
                for member in process
                    .iter()
                    .filter(|member| member.role == EntryRole::Tool)
                {
                    tree.append_branch(member, &mut tools);
                }
                let mut seen = std::collections::HashSet::new();
                tools.retain(|tool| tool.external_id.as_deref().is_none_or(|id| seen.insert(id)));
                let ranked = crate::process::ranked_kinds(
                    tools
                        .iter()
                        .filter(|tool| tool.tool.is_some())
                        .map(|tool| tool.tool_name()),
                );
                let live = turn_live && index == entries.len();
                open_process |= live;
                let active = running_tool.filter(|_| live);
                let live_kind = active.map(|tool| crate::process::activity(tool.tool_name()));
                let label = if live {
                    live_kind
                        .unwrap_or(crate::process::ProcessKind::Thinking)
                        .live_label()
                        .to_owned()
                } else {
                    crate::process::process_title(&ranked)
                };
                let detail = if live && !compact {
                    active
                        .map(|tool| crate::process::live_tool_detail(tool.tool_name(), &tool.input))
                        .unwrap_or_else(|| {
                            crate::process::live_reasoning_detail(
                                process
                                    .iter()
                                    .rev()
                                    .filter(|entry| entry.role == EntryRole::Thought)
                                    .map(|entry| entry.text.as_str()),
                            )
                        })
                } else {
                    String::new()
                };
                let title = if detail.is_empty() {
                    label
                } else {
                    format!("{label} · {detail}")
                };
                let header_key = group_key.clone();
                let process_group = format!("process-row-{}", entry.key);
                let activity = if live {
                    live_kind
                } else {
                    ranked.first().copied()
                };
                let activity_glyph =
                    match activity.unwrap_or(crate::process::ProcessKind::Thinking) {
                        crate::process::ProcessKind::Commands
                        | crate::process::ProcessKind::Code => Glyph::Play,
                        crate::process::ProcessKind::Read
                        | crate::process::ProcessKind::ReadImage => Glyph::Book,
                        crate::process::ProcessKind::Search
                        | crate::process::ProcessKind::WebSearch => Glyph::Search,
                        crate::process::ProcessKind::WebFetch => Glyph::Globe,
                        crate::process::ProcessKind::Write | crate::process::ProcessKind::Edit => {
                            Glyph::Edit
                        }
                        crate::process::ProcessKind::Plan => Glyph::Card,
                        crate::process::ProcessKind::Questions => Glyph::HelpCircle,
                        _ => Glyph::Bulb,
                    };
                let body_id = format!("process-body-{}", entry.key);
                let body_frame = self.conversation.flow_motion.update(
                    &body_id,
                    expanded,
                    8.,
                    !group_frame.closing,
                );
                let viewport = body_frame.show.then(|| {
                    visible_viewports.insert(group_key.clone());
                    self.conversation
                        .process_viewports
                        .entry(group_key.clone())
                        .or_insert_with(|| super::process_scroll::ProcessViewport::new(live, cx))
                        .clone()
                });
                let group = column()
                    .w_full()
                    .gap_0()
                    .items_start()
                    .child(
                        row()
                            .id(gpui::ElementId::Name(group_key.clone().into()))
                            .group(process_group.clone())
                            .role(gpui::Role::Button)
                            .aria_label(if expanded {
                                format!("收起{title}")
                            } else {
                                format!("展开{title}")
                            })
                            .aria_expanded(expanded)
                            .tab_index(0)
                            .max_w_full()
                            .min_w_0()
                            .h(px(24. + delta))
                            .gap(px(6.))
                            .rounded(px(8.))
                            .cursor_pointer()
                            .text_color(p.faint)
                            .hover(|style| style.text_color(p.secondary))
                            // 官方 leading：默认活动图标，hover/展开时换成 chevron。
                            .child(
                                div()
                                    .relative()
                                    .size(px(16. + delta))
                                    .flex_none()
                                    .child(
                                        div()
                                            .absolute()
                                            .inset_0()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .opacity(if expanded { 0. } else { 1. })
                                            .group_hover(process_group.as_str(), |style| {
                                                style.opacity(0.)
                                            })
                                            .child(icon(activity_glyph, 14. + delta, p.faint)),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .inset_0()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .opacity(if expanded { 1. } else { 0. })
                                            .group_hover(process_group.as_str(), |style| {
                                                style.opacity(1.)
                                            })
                                            // Official ChatGroupSeat: open → ChevronUp, closed → ChevronDown.
                                            .child(icon(
                                                if expanded { Glyph::Up } else { Glyph::Down },
                                                14. + delta,
                                                p.faint,
                                            )),
                                    ),
                            )
                            .child({
                                // Official: TextShimmer while the process group is still open.
                                if live {
                                    // `LiveShimmer` keeps its sweep phase independent of the
                                    // text, so the streaming detail can update every frame
                                    // without restarting the highlight.
                                    self.process_shimmer.update(cx, |shimmer, cx| {
                                        shimmer.set_text(title.clone(), cx);
                                        shimmer.set_active(true, cx);
                                    });
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(px(14. + delta))
                                        .child(self.process_shimmer.clone())
                                } else {
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(14. + delta))
                                        .child(title)
                                }
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                toggle_entry(&mut this.conversation.expanded_entries, &header_key);
                                cx.notify();
                            })),
                    )
                    .when_some(viewport, |element, viewport| {
                        let cap = 400f32.min(w.viewport_size().height.as_f32() * 0.5);
                        let mut body = column().gap(px(4.));
                        for entry in process {
                            body = body.child(self.render_transcript_entry(
                                entry,
                                &tree,
                                !compact || live,
                                cx,
                            ));
                        }
                        element.child(body_frame.wrap(viewport.body(body_id, cap, body, cx), 8.))
                    });
                content = content.child(flow_rows.row(group_key, group_frame.wrap(group, gap)));
                previous_role = Some(EntryRole::Thought);
                continue;
            }

            let gap = match (previous_role, entry.role) {
                (None, _) => 0.,
                (Some(EntryRole::Plan), EntryRole::Plan) => 10.,
                (_, EntryRole::Plan) => 16.,
                (_, EntryRole::User | EntryRole::QuestionReply) => 16.,
                (Some(EntryRole::Assistant), _) | (_, EntryRole::Assistant) => 12.,
                _ => 4.,
            };
            let frame = row_key.as_deref().map(|key| {
                self.conversation
                    .flow_motion
                    .update(key, !hidden, gap, true)
            });
            if frame.as_ref().is_some_and(|frame| !frame.show) {
                self.conversation.expanded_entries.remove(&entry.key);
                index += 1;
                continue;
            }
            let rendered = self.render_transcript_entry(entry, &tree, !compact || turn_live, cx);
            let rendered = match frame {
                Some(frame) => frame.wrap(rendered, gap),
                None => rendered.mt(px(gap)),
            };
            let answer = entry.external_id.as_deref().filter(|id| {
                entry.role == EntryRole::Assistant
                    && self.conversation.turn_tails.contains(id)
                    && !self.conversation.turn_tails.follows_plan(id)
                    && footer_answer.is_none_or(|footer| footer.key != entry.key)
            });
            let action_entry = if let Some(message) = answer {
                Some((entry, message))
            } else if entry.role == EntryRole::Plan {
                entry
                    .external_id
                    .as_deref()
                    .and_then(|call_id| self.conversation.turn_tails.answer_after_plan(call_id))
                    .and_then(|message| {
                        entries[..index]
                            .iter()
                            .copied()
                            .rev()
                            .find(|entry| {
                                entry.role == EntryRole::Assistant
                                    && entry.external_id.as_deref() == Some(message)
                            })
                            .filter(|entry| {
                                footer_answer.is_none_or(|footer| footer.key != entry.key)
                            })
                            .map(|entry| (entry, message))
                    })
            } else {
                None
            };
            content = content.child(flow_rows.row(entry.key.clone(), rendered));
            if let Some((answer, message)) = action_entry {
                content = content.child(
                    self.render_turn_actions(
                        answer,
                        message,
                        entries[index + 1..]
                            .iter()
                            .all(|entry| entry.role == EntryRole::Plan),
                        w,
                        cx,
                    ),
                );
            }
            previous_role = Some(entry.role);
            index += 1;
        }
        self.conversation
            .process_viewports
            .retain(|key, _| visible_viewports.contains(key));
        let flow = self.conversation.flow_motion.finish_frame(
            &self.conversation.scroll,
            self.conversation.follow_tail,
            w,
        );
        if !open_process {
            self.process_shimmer
                .update(cx, |shimmer, cx| shimmer.set_active(false, cx));
        }
        // The running-status row only exists while the turn is live, so stop its
        // sweep as soon as it is gone. Nothing else clears that flag, and an
        // `active` shimmer keeps requesting animation frames for a label that is
        // no longer on screen, which spins the WebView's renderer while idle.
        if !self.conversation.running {
            self.seeking_shimmer
                .update(cx, |shimmer, cx| shimmer.set_active(false, cx));
        }
        for panel in &self.conversation.questions {
            if let Some(id) = panel.call_id.as_deref()
                && panel.is_pending()
                && panel.hidden
                && !tree.contains(id)
            {
                content = content.child(self.render_question_tool(id, None, cx));
            }
        }
        if self.conversation.running {
            // 官方运行状态：鲸尾 + 阶段标签（思考中→深度思考中）+ 已处理/已消耗。
            let elapsed = self
                .conversation
                .turn_started
                .map(|started| ((bridge::monotonic_ms() - started) / 1000.) as f32)
                .unwrap_or(0.);
            let live_kind = running_tool.map(|entry| crate::process::activity(entry.tool_name()));
            let live_label = live_kind
                .map(|kind| kind.live_label().to_string())
                .unwrap_or_else(|| {
                    let thinking = matches!(
                        self.conversation.entries.last(),
                        Some(ChatEntry {
                            role: EntryRole::Thought,
                            ..
                        })
                    );
                    if thinking {
                        "正在分析请求".to_string()
                    } else {
                        "深度求索中".to_string()
                    }
                });
            let label = if elapsed >= 1. {
                format!("{live_label}，用时 {}s···", elapsed.floor() as u32)
            } else {
                format!("{live_label}···")
            };
            let consumed = self.turn_used_delta();
            self.seeking_shimmer.update(cx, |shimmer, cx| {
                shimmer.set_text(label, cx);
                shimmer.set_active(true, cx);
            });
            // 官方 RunningStatus：上方 0.5px 分隔线 + 14px 游动鲸鱼 + shimmer 文案。
            let status = column()
                .id("running-status")
                .relative()
                .w_full()
                .mt(px(12.))
                .pt(px(4.))
                .gap(px(0.))
                .text_color(p.state_business_primary)
                .child(
                    div()
                        .absolute()
                        .top(px(0.))
                        .left_0()
                        .right_0()
                        .h(px(0.5))
                        .bg(p.line)
                        .opacity(0.75),
                )
                .child(
                    row()
                        .w_full()
                        .h(px(28.))
                        .mt(px(4.))
                        .gap(px(6.))
                        .items_center()
                        .child(animated_brand_mark("running-kense", 16., dark(cx)))
                        .child(self.seeking_shimmer.clone()),
                );
            if let Some(consumed) = consumed {
                content = content.child(
                    status.child(
                        div()
                            .w_full()
                            .pl(px(20.))
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(p.faint)
                            .child(format!(
                                "已处理 {}s ｜ 已消耗 ◇ {consumed}",
                                elapsed.floor() as u32
                            )),
                    ),
                );
            } else {
                content = content.child(status);
            }
        } else if let Some(summary) = self.conversation.turn_summary
            && self.conversation.entries.iter().any(|entry| {
                matches!(
                    entry.role,
                    EntryRole::Assistant | EntryRole::Tool | EntryRole::Thought
                )
            })
        {
            content =
                content.child(self.render_turn_footer(summary, !layout.has_latest_control, cx));
            if let Some(entry) = footer_answer
                && let Some(message) = entry.external_id.as_deref()
            {
                content = content.child(
                    self.render_turn_actions(
                        entry,
                        message,
                        self.conversation
                            .entries
                            .iter()
                            .rfind(|entry| entry.role != EntryRole::Plan)
                            .is_some_and(|last| last.key == entry.key),
                        w,
                        cx,
                    ),
                );
            }
        }
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                super::flow_scroll::TranscriptScroll::new(
                    content,
                    flow_rows,
                    &self.conversation.scroll,
                    self.conversation.flow_scroll.clone(),
                    self.work_details,
                    self.conversation.follow_tail,
                    flow,
                )
                .on_scroll(cx.listener(
                    |this, event: &gpui::ScrollEvent, w, cx| {
                        this.sync_follow_from_scroll(
                            event.delta.pixel_delta(w.line_height()).y.as_f32(),
                        );
                        cx.notify();
                    },
                )),
            )
            .child(scrollbar_overlay(
                "conversation-scrollbar",
                &self.conversation.scroll,
                cx,
            ))
            .when(
                !self.conversation.entries.is_empty() && !self.conversation.follow_tail,
                |element| {
                    element.child(
                        div()
                            .id("scroll-to-bottom")
                            .role(gpui::Role::Button)
                            .aria_label("回到底部")
                            .tab_index(0)
                            .absolute()
                            .bottom(px(16.))
                            .right(px(28.))
                            .size(px(32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(menu_bg(cx))
                            .shadow(elevation_prominent())
                            .cursor_pointer()
                            .child(icon(Glyph::Down, 16., p.secondary))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pin_follow_tail();
                                cx.notify();
                            })),
                    )
                },
            )
    }

    fn render_turn_process_control(
        &self,
        control: crate::history::turn_process::Control,
        follows_row: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        use crate::history::turn_process::EndKind;

        let p = dsh(cx);
        let delta = self.content_font_delta as f32;
        let mut label = row().min_w_0().gap_0().text_size(px(14. + delta));
        let mut accessible = match (control.kind, control.seconds) {
            (EndKind::Stopped, _) => "已停止",
            (EndKind::Failed, _) => "处理失败",
            (EndKind::Completed, Some(_)) => "已完成，用时 ",
            (EndKind::Completed, None) => "已完成",
        }
        .to_owned();
        label = label.child(accessible.clone());
        if control.kind == EndKind::Completed
            && let Some(seconds) = control.seconds
        {
            for (value, unit, visible) in [
                (seconds / 3600, "小时", seconds >= 3600),
                (seconds / 60 % 60, "分", seconds >= 60),
                (seconds % 60, "秒", true),
            ] {
                if visible {
                    let value = value.to_string();
                    accessible.push_str(&value);
                    accessible.push_str(unit);
                    label = label
                        .child(div().font_family("monospace").child(value))
                        .child(unit);
                }
            }
        }
        let line = row()
            .w_full()
            .h(px(33. + delta))
            .pb(px(8.))
            .gap(px(4.))
            .items_center()
            .border_b(px(0.5))
            .border_color(p.border_l2)
            .text_color(p.label_tertiary)
            .child(label);
        let surface = div().w_full().mt(px(if follows_row { 12. } else { 0. }));
        if control.can_toggle {
            surface.child(
                line.id(gpui::ElementId::Name(
                    format!("turn-process-{}", control.number).into(),
                ))
                .role(gpui::Role::Button)
                .aria_label(accessible)
                .aria_expanded(control.open)
                .tab_index(0)
                .cursor_pointer()
                .hover(|style| style.text_color(p.label_secondary))
                .focus_visible(|style| style.text_color(p.label_secondary))
                .child(icon(
                    if control.open { Glyph::Up } else { Glyph::Down },
                    14.,
                    p.label_tertiary,
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.conversation
                        .turn_disclosures
                        .insert(control.number, (control.answer_step, !control.open));
                    cx.notify();
                })),
            )
        } else {
            surface.child(line)
        }
    }

    /// 官方轮次收尾：消耗行 + 33px 分隔线 + 轮尾操作行。
    /// 官方 TurnNavigator：对话右侧每轮一个刻度。
    ///
    /// 悬停显示该轮的输入与最新回答预览，点击跳到这一轮的开头行。少于两轮
    /// 时不显示（和官方一致）；轨上的高亮跟随视口顶部所在的轮次。
    fn render_turn_rail(
        &self,
        items: &[crate::turns::TurnItem],
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if items.len() < 2 {
            return div().into_any_element();
        }
        let p = dsh(cx);
        let scroll_top = -self.conversation.scroll.offset().y.as_f32();
        let max_offset = self.conversation.scroll.max_offset().y.as_f32();
        let near_bottom = max_offset - scroll_top <= 24.;
        let tops: Vec<Option<f32>> = items
            .iter()
            .map(|item| self.conversation.flow_scroll.row_top(&item.anchor))
            .collect();
        let active = crate::turns::active_turn(&tops, scroll_top, 24., near_bottom);
        let hover = self.conversation.turn_rail_hover;
        let total = items.len();
        // Official rail: fixed 10px pitch between marks, capped at 420px tall.
        // Once a session has more turns than that cap fits, compress the pitch
        // (never the other way around) so every turn stays reachable. Each mark
        // box is exactly one pitch tall; the tick scales in place inside it.
        let pitch = (420. / total as f32).clamp(3., 10.);
        let mut marks = column().w_full().items_center().gap(px(0.));
        for (index, item) in items.iter().enumerate() {
            let is_active = active == Some(index);
            let anchor = item.anchor.clone();
            // Official mark button label: chat.turnNavigation.jump.
            let label = format!("跳转到第 {} 轮", item.turn);
            let mut mark = div()
                .id(gpui::ElementId::Name(format!("turn-mark-{index}").into()))
                .relative()
                .role(gpui::Role::Button)
                .aria_label(label)
                .tab_index(0)
                .w_full()
                .h(px(pitch))
                .flex()
                .items_center()
                .justify_end()
                .cursor_pointer()
                .child(
                    div()
                        .w(px(if is_active {
                            20.
                        } else if hover == Some(index) {
                            18.
                        } else {
                            12.
                        }))
                        .h(px(if pitch >= 4. { 2. } else { 1. }))
                        .rounded_full()
                        .bg(if is_active {
                            p.ink
                        } else if hover == Some(index) {
                            p.faint
                        } else {
                            p.border_l4
                        }),
                )
                .on_hover(cx.listener(move |this, inside: &bool, _, cx| {
                    let next = if *inside {
                        Some(index)
                    } else if this.conversation.turn_rail_hover == Some(index) {
                        None
                    } else {
                        this.conversation.turn_rail_hover
                    };
                    if this.conversation.turn_rail_hover != next {
                        this.conversation.turn_rail_hover = next;
                        cx.notify();
                    }
                }))
                // The rail is a press target: act on press so the jump lands
                // without waiting for a click pair on an 8px hit area.
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.jump_to_turn(&anchor, index, total, cx);
                    }),
                );
            if hover == Some(index) {
                mark = mark.child(self.render_turn_preview(item, cx));
            }
            marks = marks.child(mark);
        }
        // Sit in the gutter just right of the conversation column: the
        // transcript's own right edge can fall outside the window's clickable
        // area, while this gutter stays inside it (and next to the reading
        // column, like the official rail).
        let main_width = f32::from(w.viewport_size().width) - self.sidebar_track_width(w);
        let column = crate::layout::conversation_axis(main_width);
        let dock = self.right_dock_frame.track;
        let gutter = ((main_width - dock - column) / 2. + dock - 4.).max(6.);
        div()
            .id("turn-rail")
            .absolute()
            .right(px(gutter))
            .top(px(56.))
            .bottom(px(56.))
            .w(px(20.))
            .flex()
            .flex_col()
            .justify_center()
            .child(marks)
            .into_any_element()
    }

    /// 轮次刻度的悬停预览：第 N 轮 + 输入 + 最新回答。
    fn render_turn_preview(
        &self,
        item: &crate::turns::TurnItem,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        let (prompt, response) = crate::turns::preview_lines(item);
        // Official preview prompt falls back to the turn number when the turn
        // has no user text (a trigger-only turn).
        let prompt = if prompt.is_empty() {
            format!("第 {} 轮", item.turn)
        } else {
            prompt
        };
        let mut preview = column()
            .absolute()
            .right(px(30.))
            .top(px(-6.))
            .w(px(300.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(12.))
            .bg(p.stage)
            .shadow(crate::tokens::elevation_panel())
            .occlude()
            .child(
                div()
                    .truncate()
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.ink)
                    .child(prompt),
            );
        if !response.is_empty() {
            preview = preview.child(
                div()
                    .mt(px(4.))
                    .line_clamp(3)
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(p.faint)
                    .child(response),
            );
        }
        preview
    }

    /// Jump the transcript to one turn's opening row and stop following the tail.
    fn jump_to_turn(&mut self, anchor: &str, turn: usize, total: usize, cx: &mut Context<Self>) {
        let max = self.conversation.scroll.max_offset().y.as_f32();
        // Prefer the measured row position; before the first measurement pass a
        // proportional guess still lands near the turn instead of doing nothing.
        let target = match self.conversation.flow_scroll.row_top(anchor) {
            Some(top) => (top - 12.).max(0.).min(max),
            None => {
                let fraction = if total <= 1 {
                    0.
                } else {
                    turn as f32 / (total - 1) as f32
                };
                (max * fraction).max(0.)
            }
        };
        self.conversation.follow_tail = false;
        let x = self.conversation.scroll.offset().x;
        self.conversation.scroll.set_offset(point(x, px(-target)));
        self.conversation.turn_rail_hover = None;
        cx.notify();
    }

    fn render_turn_footer(
        &self,
        summary: TurnSummary,
        show_divider: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        // 官方：简洁/详细两种模式的已完成轮次页脚都不显示耗时。
        let text = match summary.end {
            TurnEnd::Completed => "已完成".to_owned(),
            other => other.label().to_string(),
        };
        // 官方消耗行：“共消耗 ◇ N ✓ 均衡 (模型)”。模型名来自会话当前配置。
        let model = self.current_config_label("model");
        let effort = self.current_config_label("reasoning_effort");
        let model_chip = match (effort, model) {
            (Some(effort), Some(model)) => format!("{effort} ({model})"),
            (None, Some(model)) => model,
            _ => String::new(),
        };
        column()
            .w_full()
            .mt(px(20.))
            .gap(px(6.))
            .when_some(summary.used, |element, used| {
                element.child(
                    row()
                        .id("turn-consumption")
                        .w_full()
                        .gap(px(8.))
                        .items_center()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(p.faint)
                        .child(format!("共消耗 ◇ {used}"))
                        .when(
                            summary.end == TurnEnd::Completed && !model_chip.is_empty(),
                            |element| {
                                element
                                    .child(icon(Glyph::Check, 12., p.success))
                                    .child(div().min_w_0().truncate().child(model_chip))
                            },
                        ),
                )
            })
            .when(show_divider, |element| {
                element.child(
                    row()
                        .id("turn-divider")
                        .w_full()
                        .h(px(33.))
                        .gap(px(10.))
                        .items_center()
                        .child(div().flex_1().h(px(0.5)).bg(p.line))
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(p.faint)
                                .child(text),
                        )
                        .child(div().flex_1().h(px(0.5)).bg(p.line)),
                )
            })
    }

    fn render_turn_actions(
        &self,
        entry: &ChatEntry,
        message: &str,
        always: bool,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let mark = format!("turn-tail-{message}");
        let copied = self.copied_mark.as_deref() == Some(mark.as_str());
        let text = entry.text.clone();
        let branch_available = self
            .conversation
            .turn_tails
            .branch_boundary(message)
            .is_some();
        let branch_message = message.to_owned();
        let delta = self.content_font_delta as f32;
        let group = format!("turn-actions-{message}");
        let focus = self.rating_row_focus(message);
        let visible = always || focus.is_some_and(|focus| focus.contains_focused(w, cx));
        row()
            .id(group.clone())
            .group(group.clone())
            .when_some(focus, |row, focus| row.track_focus(focus).tab_stop(false))
            .w_full()
            .h(px(28. + delta))
            .mt(px(4.))
            .ml(px(-6.))
            .gap(px(8.))
            .child(
                row()
                    .id("turn-actions-content")
                    .gap(px(8.))
                    .text_color(p.label_tertiary)
                    .when(!visible, |row| {
                        row.opacity(0.)
                            .group_hover(&group, |style| style.opacity(1.))
                    })
                    .child(turn_action_button(
                        "turn-copy",
                        if copied { "已复制" } else { "复制" },
                        if copied {
                            include_bytes!("../../assets/document-copied.svg")
                        } else {
                            include_bytes!("../../assets/document-copy.svg")
                        },
                        None,
                        delta,
                        cx,
                        move |this, _, cx| {
                            bridge::copy_text(&text);
                            mark_copied(this, &mark, cx);
                            this.push_toast("已复制", cx);
                        },
                    ))
                    .child(self.render_message_ratings(message, cx))
                    .child(turn_action_button(
                        "turn-branch",
                        "在新对话中分支",
                        include_bytes!("../../assets/turn-branch.svg"),
                        (!branch_available).then_some("仅可从已完成轮次的最后一条消息分支"),
                        delta,
                        cx,
                        move |this, _, cx| {
                            if let Some(session) = this.active_session.clone()
                                && let Some(boundary) = this
                                    .conversation
                                    .turn_tails
                                    .branch_boundary(&branch_message)
                            {
                                this.fork_session(&session, ForkOrigin::Answer(boundary), cx);
                            }
                        },
                    )),
            )
    }

    /// 官方审批卡：替换输入区原位出现（非模态），warn 黄 1px 边框 r20。
    /// 官方 toast：底部居中，.16s 淡入，停留 3s，1s 淡出。
    fn render_toasts(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        // Official Toast is a top-center banner: `position: fixed; top: 40px;
        // left: 50%; transform: translateX(-50%)` (Toast.module.css). It never
        // intercepts clicks, so the layer is inert.
        let mut list = column()
            .id("toast-layer")
            .absolute()
            .top(px(40.))
            .left_0()
            .right_0()
            .flex_none()
            .items_center()
            .gap(px(8.));
        for toast in &self.toasts {
            let age = ((bridge::monotonic_ms() - toast.at) / 1000.) as f32;
            if age >= crate::anim::TOAST_TOTAL_SECONDS {
                continue;
            }
            // `dsh-toast-in 160ms ease-out` then `dsh-toast-fade 1000ms ease`
            // delayed by the hold, which is what the layer's lifetimes encode.
            let (opacity, rise) = crate::anim::toast_frame(age);
            list = list.child(
                div()
                    .max_w(px(640.))
                    .px(px(16.))
                    .py(px(12.))
                    .rounded(px(16.))
                    .bg(p.toast_bg)
                    .text_color(p.toast_label)
                    .shadow(elevation_prominent())
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .opacity(opacity.clamp(0., 1.))
                    .top(px(rise))
                    .child(toast.message.clone()),
            );
        }
        list
    }

    fn render_permission(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        let permission = self.conversation.permission.as_ref()?;
        let p = dsh(cx);
        let title = permission
            .tool_call
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("工具调用");
        let mut actions = row().w_full().justify_end().gap(px(8.));
        // 官方：拒绝 → 允许 → 始终允许；有「始终允许」时只有它是主按钮。
        let mut options = permission.options.clone();
        options.sort_by_key(|(id, _)| {
            let id = id.to_lowercase();
            if id.contains("reject") || id.contains("deny") || id.contains("cancel") {
                0
            } else if id.contains("always") {
                2
            } else {
                1
            }
        });
        let has_always = options
            .iter()
            .any(|(id, _)| id.to_lowercase().contains("always"));
        for (id, name) in &options {
            let option_id = id.clone();
            let primary = if has_always {
                id.to_lowercase().contains("always")
            } else {
                id.to_lowercase().contains("allow")
            };
            let button = if primary {
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
        let width = self.composer_width(w);
        Some(
            column()
                .id("approval-card")
                .w(px(width))
                .max_w_full()
                .mx_auto()
                .flex_none()
                .relative()
                .gap(px(12.))
                .px(px(16.))
                .pt(px(12.))
                .pb(px(12.))
                .rounded(px(20.))
                .border_1()
                .border_color(p.state_warn_primary.opacity(0.6))
                .bg(composer_bg(cx))
                .shadow(elevation_soft())
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    row()
                        .w_full()
                        .gap(px(8.))
                        .items_center()
                        .child(pending_dot(
                            "approval-dot",
                            div().size(px(8.)).rounded_full().bg(p.state_warn_primary),
                        ))
                        .child(
                            div()
                                .text_size(px(14.))
                                .line_height(px(22.))
                                .font_weight(FontWeight::MEDIUM)
                                .child("等待审批"),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(p.secondary)
                                .child(format!("工具 {title} 请求越权执行")),
                        ),
                )
                .child(actions),
        )
    }

    fn render_composer(&mut self, w: &mut Window, cx: &mut Context<Self>, hero: bool) -> gpui::Div {
        let p = dsh(cx);
        let usage = self.conversation.usage;
        let empty = self.composer.read(cx).value().trim().is_empty()
            && self.conversation.draft_attachments.is_empty();
        let mut composer_width = self.composer_width(w);
        if hero {
            let main_width = f32::from(w.viewport_size().width)
                - self.sidebar_track_width(w)
                - self.right_dock_frame.track;
            composer_width = composer_width.min(main_width - 48.);
        }
        let model = self.current_config_label("model");
        let reasoning = self.current_config_label("reasoning_effort");
        let permission_open = self.composer_menu == Some(ComposerMenu::Permission);
        let model_open = self.composer_menu.is_some_and(ComposerMenu::is_model);
        let permission_chevron = composer_chevron(
            &mut self.permission_menu.chevron,
            permission_open,
            p.label_caption,
            w,
            cx,
        );
        let model_chevron = composer_chevron(
            &mut self.model_menu.chevron,
            model_open,
            p.label_caption,
            w,
            cx,
        );
        let has_model = model.is_some();
        let toolbar_width = px(composer_width - 17.);
        let toolbar_gap = px(if toolbar_width <= px(560.) { 8. } else { 12. });
        let permission = self.conversation.permission_mode.state.as_ref();
        let permission_glyph =
            permission.and_then(|state| permission_icon(&state.current_value, 14., p.secondary));
        let compact_permission = permission_glyph.is_some() && toolbar_width <= px(460.);
        let stopping = self.conversation.running && !empty;
        let plan = self.conversation.plan_active == Some(true);
        let compact_model = model.as_ref().is_some_and(|label| {
            let model_width = (px(28.)
                + composer_label_width(label, FontWeight::NORMAL, w, cx)
                + reasoning.as_ref().map_or(px(0.), |effort| {
                    px(4.) + composer_label_width(effort, FontWeight::NORMAL, w, cx)
                }))
            .min(px(360.));
            let permission_width = permission.map_or(px(0.), |state| {
                px(24.)
                    + if permission_glyph.is_some() {
                        px(18.)
                    } else {
                        px(0.)
                    }
                    + if compact_permission {
                        px(0.)
                    } else {
                        composer_label_width(state.current_label(), FontWeight::MEDIUM, w, cx)
                    }
                    + toolbar_gap
            });
            let plan_width = if plan {
                px(34.) + composer_label_width("计划", FontWeight::MEDIUM, w, cx) + toolbar_gap
            } else {
                px(0.)
            };
            let stop_width = if stopping {
                px(34.) + toolbar_gap
            } else {
                px(0.)
            };
            px(62.) + toolbar_gap * 2. + permission_width + plan_width + model_width + stop_width
                > toolbar_width
        });
        // 官方 hero / conversation 两套 placeholder；plan 模式生效期间，
        // 会话 composer 改用计划任务提示（hero 自带 surface placeholder）。
        let placeholder = if !hero && self.conversation.plan_active == Some(true) {
            "描述你的任务以生成计划"
        } else if hero {
            "描述你想要构建的内容, / 调用指令, @ 文件或对话"
        } else {
            "发消息或创建任务, / 调用指令, @ 文件或对话"
        };
        let brand = p.state_business_primary;
        self.composer.update(cx, |editor, _| {
            editor.set_placeholder(placeholder);
            // 官方：已认领的 slash command 是品牌蓝的原子 chip
            // （--dsw-alias-state-business-primary）。KUI Editor 通过 run
            // styler 给行首 token 上色；灰提示仍由下方单独一行渲染，因为
            // KUI 在 ghost 文本显示时会跳过 run styler。
            editor.set_run_styler(std::rc::Rc::new(move |text: &str, base: &gpui::TextRun| {
                let Some(len) = crate::commands::claimed_command_len(text) else {
                    return vec![base.clone()];
                };
                let mut chip = base.clone();
                chip.len = len;
                chip.color = brand;
                chip.font.weight = gpui::FontWeight::MEDIUM;
                let mut rest = base.clone();
                rest.len = text.len() - len;
                if rest.len == 0 {
                    vec![chip]
                } else {
                    vec![chip, rest]
                }
            }));
        });
        // Harness renders a grey ghost hint after an empty claimed command chip.
        let draft = self.composer.read(cx).value().to_owned();
        let claim_hint = crate::commands::command_claim_hint(&draft);
        let card_bounds = self.composer_card_bounds.clone();
        let card_open = self
            .composer_menu
            .is_some_and(ComposerMenu::is_card_overlay);
        column()
            .relative()
            .when(!hero, |element| {
                element.w(px(composer_width)).max_w_full().mx_auto()
            })
            .when(hero, |element| element.w_full())
            .flex_none()
            .gap(px(6.))
            .when(!hero, |element| element.pt(px(4.)).pb(px(4.)))
            .when_some(
                self.conversation
                    .error
                    .clone()
                    .or_else(|| self.error.clone()),
                |element, error| {
                    let auth_failure = crate::state::is_auth_failure(&error);
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
                            .when(auth_failure, |row| {
                                row.child(
                                    div()
                                        .id("open-api-key-settings")
                                        .px(px(8.))
                                        .py(px(3.))
                                        .rounded(px(6.))
                                        .border(px(0.5))
                                        .border_color(p.danger)
                                        .text_size(px(12.))
                                        .text_color(p.danger)
                                        .child("去设置")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.error = None;
                                            this.conversation.error = None;
                                            this.show_more_menu = false;
                                            this.settings_page = SettingsPage::Credentials;
                                            this.show_settings = true;
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(icon_btn(
                                "dismiss-error",
                                "关闭错误",
                                Glyph::Close,
                                cx,
                                |this, _, cx| {
                                    this.error = None;
                                    this.conversation.error = None;
                                    cx.notify();
                                },
                            )),
                    )
                },
            )
            .when(self.conversation.goal.is_some(), |element| {
                element.child(self.render_goal_bar(cx))
            })
            .when(!self.conversation.subagents.is_empty(), |element| {
                element.child(self.render_subagent_catalog(cx))
            })
            .when(!self.conversation.queued_prompts.is_empty(), |element| {
                element.child(self.render_queue_cards(cx))
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
                    .shadow(elevation_soft())
                    .child(measure_composer_anchor(card_bounds.clone(), card_open))
                    .child(
                        div()
                            .w_full()
                            .h(if hero { px(52.) } else { px(36.) })
                            .pl(px(14.))
                            .pr(px(8.))
                            .child(self.composer.clone()),
                    )
                    .when_some(claim_hint, |element, hint| {
                        element.child(
                            div()
                                .w_full()
                                .pl(px(14.))
                                .pr(px(8.))
                                .pb(px(2.))
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(p.label_caption)
                                .child(hint),
                        )
                    })
                    .when(!self.conversation.draft_attachments.is_empty(), |element| {
                        element.child(self.render_attachment_rail(cx))
                    })
                    .child(
                        row()
                            .w_full()
                            .min_h(px(40.))
                            .px(px(8.))
                            .pt(px(2.))
                            .pb(px(6.))
                            .gap(toolbar_gap)
                            .child(
                                div()
                                    .id("composer-command")
                                    .role(gpui::Role::Button)
                                    .aria_label("调用指令")
                                    .aria_expanded(
                                        self.composer_menu == Some(ComposerMenu::Commands),
                                    )
                                    .tab_index(0)
                                    .size(px(28.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .bg(p.selector)
                                    .hover(|style| style.bg(p.hover_solid))
                                    .child(icon(Glyph::Plus, 14., p.ink))
                                    .on_click(cx.listener(|this, _, w, cx| {
                                        this.toggle_composer_menu(ComposerMenu::Commands);
                                        if this.composer_menu == Some(ComposerMenu::Commands) {
                                            let focus = this.composer.read(cx).focus.clone();
                                            focus.focus(w, cx);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .children(permission.map(|state| {
                                let mode_label = state.current_label();
                                let enabled = !self.conversation.permission_mode.changing()
                                    && self.confirm_permission.is_none();
                                let popup =
                                    self.render_button_popup(ComposerMenu::Permission, "", w, cx);
                                let trigger = row()
                                    .id("composer-permission")
                                    .relative()
                                    .role(gpui::Role::ComboBox)
                                    .aria_label(format!("审批模式：{mode_label}"))
                                    .aria_expanded(permission_open)
                                    .track_focus(&self.permission_menu.trigger)
                                    .tab_index(0)
                                    .tab_stop(enabled)
                                    .h(px(28.))
                                    .max_w(px(220.))
                                    .pl(px(8.))
                                    .pr(px(4.))
                                    .gap(px(4.))
                                    .rounded(px(8.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .when(enabled, |trigger| {
                                        trigger.cursor_pointer().hover(|style| style.bg(p.hover))
                                    })
                                    .when(
                                        enabled
                                            && self.permission_menu.trigger.is_focused(w)
                                            && bridge::keyboard_focus_visible(),
                                        |trigger| {
                                            trigger.child(composer_focus_ring(
                                                p.state_business_primary,
                                            ))
                                        },
                                    )
                                    .children(permission_glyph)
                                    .when(!compact_permission, |trigger| {
                                        trigger.child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(px(13.))
                                                .line_height(px(20.))
                                                .text_color(p.secondary)
                                                .child(mode_label.to_owned()),
                                        )
                                    })
                                    .child(permission_chevron);
                                div()
                                    .relative()
                                    .min_w_0()
                                    .child(super::dock_controls::dock_action_button(
                                        trigger,
                                        cx.listener(|this, _, w, cx| {
                                            this.toggle_permission_menu(w, cx);
                                        }),
                                    ))
                                    .children(popup)
                            }))
                            .children(self.plan_chip(cx))
                            .children(model.map(|label| {
                                let popup = self
                                    .composer_menu
                                    .filter(|menu| menu.is_model())
                                    .and_then(|menu| self.render_button_popup(menu, &label, w, cx));
                                let trigger = row()
                                    .id("composer-model")
                                    .relative()
                                    .role(gpui::Role::ComboBox)
                                    .aria_label(reasoning.as_ref().map_or_else(
                                        || format!("选择模型，{label}"),
                                        |effort| format!("选择模型，{label}，思考强度 {effort}"),
                                    ))
                                    .aria_expanded(model_open)
                                    .track_focus(&self.model_menu.trigger)
                                    .tab_index(0)
                                    .tab_stop(true)
                                    .h(px(28.))
                                    .max_w(px(360.))
                                    .pl(px(8.))
                                    .pr(px(4.))
                                    .gap(px(4.))
                                    .rounded(px(8.))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(p.hover))
                                    .when(
                                        !self.model_menu.selection_focus
                                            && self.model_menu.trigger.is_focused(w)
                                            && bridge::keyboard_focus_visible(),
                                        |trigger| {
                                            trigger.child(composer_focus_ring(
                                                p.state_business_primary,
                                            ))
                                        },
                                    )
                                    .when(compact_model, |trigger| {
                                        trigger.child(
                                            svg()
                                                .data(include_bytes!(
                                                    "../../assets/composer-model.svg"
                                                ))
                                                .size(px(16.))
                                                .flex_none()
                                                .text_color(p.secondary),
                                        )
                                    })
                                    .when(!compact_model, |trigger| {
                                        trigger
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
                                                    .min_w_0()
                                                    .flex_shrink(1000.)
                                                    .truncate()
                                                    .text_size(px(13.))
                                                    .line_height(px(20.))
                                                    .text_color(p.label_caption)
                                                    .child(effort)
                                            }))
                                    })
                                    .when(!self.model_menu_busy(), |trigger| {
                                        trigger.child(model_chevron)
                                    })
                                    .when(self.model_menu_busy(), |trigger| {
                                        trigger.child(pending_dot(
                                            "model-trigger-pending",
                                            div().size(px(7.)).rounded_full().bg(p.label_caption),
                                        ))
                                    });
                                div()
                                    .id("composer-model-scope")
                                    .track_focus(&self.model_menu.scope)
                                    .tab_stop(false)
                                    .relative()
                                    .ml_auto()
                                    .min_w_0()
                                    .child(super::dock_controls::dock_action_button(
                                        trigger,
                                        cx.listener(|this, _, w, cx| this.toggle_model_menu(w, cx)),
                                    ))
                                    .children(popup)
                            }))
                            .when(stopping, |element| {
                                element.child(
                                    self.render_stop_button(cx)
                                        .when(!has_model, |button| button.ml_auto()),
                                )
                            })
                            .child(
                                self.render_send_button(empty, cx)
                                    .when(!has_model && !stopping, |button| button.ml_auto()),
                            ),
                    )
                    .when_some(
                        self.composer_menu.filter(|menu| {
                            menu.is_card_overlay() && self.confirm_permission.is_none()
                        }),
                        |element, menu| {
                            let offset = px(if hero {
                                HERO_WORKSPACE_HEIGHT + HERO_COMPOSER_GAP
                            } else {
                                0.
                            });
                            let cap = px(if menu.is_command_picker() { 320. } else { 400. });
                            let max_height = card_bounds.get().map_or(cap, |card| {
                                (card.top() - offset - px(88.)).clamp(px(0.), cap)
                            });
                            if max_height <= px(0.) {
                                return element;
                            }
                            let menu = self
                                .render_composer_overlay(menu, max_height, cx)
                                .on_mouse_down_out(cx.listener(
                                    move |this, event: &gpui::MouseDownEvent, _, cx| {
                                        if menu.is_command_picker()
                                            || !card_bounds.get().is_some_and(|bounds| {
                                                bounds.contains(&event.position)
                                            })
                                        {
                                            this.composer_menu = None;
                                            cx.notify();
                                        }
                                    },
                                ));
                            element.child(
                                div()
                                    .absolute()
                                    .top(-offset)
                                    .left_0()
                                    .right_0()
                                    .h_0()
                                    .child(menu),
                            )
                        },
                    ),
            )
            .child(self.render_composer_stats(usage, w, cx))
    }

    /// 官方独立停止按钮：生成中出现在主发送键左侧的幽灵方块。
    fn render_stop_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let t = dsh(cx);
        div()
            .id("stop-generation")
            .role(gpui::Role::Button)
            .aria_label("停止生成")
            .tab_index(0)
            .size(px(34.))
            .mt(px(-2.))
            .mr(px(-6.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(t.selector)
            .cursor_pointer()
            .hover(|style| style.bg(t.hover_solid))
            .child(div().size(px(10.)).rounded(px(3.)).bg(t.ink))
            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
    }

    /// 官方 GoalBar：composer 上方 36px 的目标条。
    ///
    /// 图标 + 阶段标签 + 单行省略的目标正文 + 动作：进行中且可自动继续时给
    /// 暂停，非进行中的阶段给恢复，任何阶段都可以编辑或清除。创建目标仍归
    /// `/goal` 命令，这条只管理层状态。
    fn render_goal_bar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = dsh(cx);
        let Some(goal) = self.conversation.goal.clone() else {
            return div().into_any_element();
        };
        let busy = self.conversation.goal_busy;
        let can_pause = goal.can_pause();
        let can_resume = goal.can_resume();
        let action_button =
            |id: &'static str,
             label: &'static str,
             glyph: Glyph,
             enabled: bool,
             cx: &mut Context<Self>,
             on_click: fn(&mut DshApp, &mut Window, &mut Context<DshApp>)| {
                let button = div()
                    .id(gpui::ElementId::Name(id.into()))
                    .role(gpui::Role::Button)
                    .aria_label(label)
                    .size(px(24.))
                    .rounded(px(crate::tokens::Tokens::RADIUS_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(glyph, 14., p.faint));
                if enabled && !busy {
                    button
                        .cursor_pointer()
                        .hover(|style| style.bg(p.hover))
                        .on_click(cx.listener(move |this, _, w, cx| on_click(this, w, cx)))
                } else {
                    button.opacity(0.4)
                }
            };
        let mut bar = row()
            .id("goal-bar")
            .w_full()
            .h(px(36.))
            .px(px(12.))
            .py(px(4.))
            .gap(px(10.))
            .items_center()
            .rounded(px(crate::tokens::Tokens::RADIUS_SM))
            .border(px(0.5))
            .border_color(p.line)
            .bg(p.stage)
            .when_some(goal.title().map(str::to_owned), |bar, title| {
                let reason: gpui::SharedString = format!("受阻原因：{title}").into();
                bar.tooltip(move |_, cx| cx.new(|_| TurnActionHint(reason.clone())).into())
            })
            .child(
                gpui::svg()
                    .data(include_bytes!("../../assets/goal.svg") as &[u8])
                    .size(px(14.))
                    .flex_none()
                    .text_color(p.faint),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(13.))
                    .line_height(px(24.))
                    .text_color(p.ink)
                    .child(goal.label()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .text_color(p.label_dimmed)
                    .child(goal.objective_preview(120)),
            );
        if let Some(error) = self.conversation.goal_error.clone() {
            bar = bar.child(
                div()
                    .flex_none()
                    .max_w(px(180.))
                    .truncate()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(p.danger)
                    .child(error),
            );
        }
        // The official strip renders exactly the verbs the phase allows: pause
        // only for an armed active goal, resume for every other phase.
        if can_pause {
            bar = bar.child(action_button(
                "goal-pause",
                "暂停目标",
                Glyph::Pause,
                true,
                cx,
                |this, w, cx| this.run_goal_action("pause", w, cx),
            ));
        }
        if can_resume {
            bar = bar.child(action_button(
                "goal-resume",
                "恢复目标",
                Glyph::Play,
                true,
                cx,
                |this, w, cx| this.run_goal_action("resume", w, cx),
            ));
        }
        bar = bar
            .child(action_button(
                "goal-edit",
                "编辑目标",
                Glyph::Edit,
                true,
                cx,
                |this, w, cx| this.edit_goal_objective(w, cx),
            ))
            .child(action_button(
                "goal-clear",
                "清除目标",
                Glyph::Trash,
                true,
                cx,
                |this, w, cx| this.run_goal_action("clear", w, cx),
            ));
        bar.into_any_element()
    }

    /// composer 上方的子智能体目录（官方 session-header lineage 的功能对应）。
    ///
    /// 折叠时只有一行「子智能体 N · M 运行中」；展开后每个直接子会话一行，
    /// 显示标签、创建模式与活动状态，点一行打开它的只读执行记录。
    fn render_subagent_catalog(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let total = self.conversation.subagents.len();
        let running = self
            .conversation
            .subagents
            .iter()
            .filter(|row| self.running_subagents.contains_key(&row.child_id))
            .count();
        let expanded = self.conversation.subagents_expanded;
        let summary = if running > 0 {
            format!("子智能体 {total} · {running} 运行中")
        } else {
            format!("子智能体 {total}")
        };
        let mut column = column().w_full().gap(px(6.));
        column = column.child(
            row()
                .id("subagent-catalog-header")
                .role(gpui::Role::Button)
                .aria_label(&summary)
                .aria_expanded(expanded)
                .tab_index(0)
                .w_full()
                .h(px(28.))
                .px(px(10.))
                .gap(px(8.))
                .items_center()
                .rounded(px(crate::tokens::Tokens::RADIUS_SM))
                .cursor_pointer()
                .hover(|style| style.bg(p.hover))
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(p.label_caption)
                .child(icon(Glyph::User, 13., p.label_caption))
                .child(div().flex_1().min_w_0().truncate().child(summary.clone()))
                .child(icon(
                    if expanded { Glyph::Up } else { Glyph::Down },
                    12.,
                    p.faint,
                ))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_subagent_list(cx))),
        );
        if expanded {
            for catalog_row in self.conversation.subagents.clone() {
                let running = self.running_subagents.contains_key(&catalog_row.child_id);
                let activity = crate::subagents::activity_label(&catalog_row, running);
                let child_id = catalog_row.child_id.clone();
                let label = catalog_row.label.clone();
                let has_transcript = catalog_row.has_local_session();
                let item = row()
                    .id(gpui::ElementId::Name(
                        format!("subagent-{}", catalog_row.child_id).into(),
                    ))
                    .role(gpui::Role::Button)
                    .aria_label(format!("打开子智能体 {} 的执行记录", catalog_row.label))
                    .tab_stop(has_transcript)
                    .w_full()
                    .min_h(px(32.))
                    .px(px(12.))
                    .py(px(6.))
                    .gap(px(8.))
                    .items_center()
                    .rounded(px(crate::tokens::Tokens::RADIUS_SM))
                    .border(px(0.5))
                    .border_color(p.line)
                    .bg(p.stage)
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(6.))
                            .py(px(1.))
                            .rounded_full()
                            .border(px(0.5))
                            .border_color(p.line)
                            .text_size(px(10.))
                            .line_height(px(14.))
                            .text_color(p.label_caption)
                            .child(catalog_row.mode.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .text_color(p.secondary)
                            .child(catalog_row.label.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(if running {
                                p.state_business_primary
                            } else {
                                p.label_caption
                            })
                            .child(activity),
                    );
                column = column.child(if has_transcript {
                    item.cursor_pointer()
                        .hover(|style| style.bg(p.hover))
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.open_subagent_child(&child_id, &label, w, cx);
                        }))
                } else {
                    item.opacity(0.6)
                });
            }
        }
        column
    }

    /// 只读子会话顶部的导航条：说明当前看的是谁的执行记录，并提供返回父会话。
    fn render_subagent_banner(
        &self,
        label: &str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        row()
            .id("subagent-banner")
            .w_full()
            .mb(px(10.))
            .px(px(12.))
            .py(px(8.))
            .gap(px(8.))
            .items_center()
            .rounded(px(crate::tokens::Tokens::RADIUS_SM))
            .border(px(0.5))
            .border_color(p.line)
            .bg(p.stage)
            .child(icon(Glyph::User, 14., p.label_caption))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(p.label_caption)
                    .child(format!("子智能体 · {label} · 只读执行记录")),
            )
            .child(
                div()
                    .id("subagent-back")
                    .role(gpui::Role::Button)
                    .aria_label("返回父会话")
                    .tab_index(0)
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(6.))
                    .bg(p.hover)
                    .cursor_pointer()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(p.secondary)
                    .child("返回父会话")
                    .on_click(cx.listener(|this, _, w, cx| this.close_subagent_view(w, cx))),
            )
    }

    /// 官方 QueueDock：Host Inbox 里每条待投递消息一行。
    ///
    /// 行来自 Host（关闭应用也不会丢），这里只负责展示与动作：复制、取回
    /// 编辑、删除；排队中的行还可以改成插话，交给运行中的回合。
    fn render_queue_cards(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let running = self.conversation.running;
        let count = self.conversation.queued_prompts.len();
        let mut list = column().w_full().gap(px(6.));
        if count > 1 {
            list = list.child(
                row()
                    .w_full()
                    .px(px(4.))
                    .gap(px(6.))
                    .items_center()
                    .child(icon(Glyph::ArrowUp, 12., p.label_caption))
                    .child(
                        div()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(p.label_caption)
                            .child(format!("{count} 条排队消息")),
                    ),
            );
        }
        for (index, queued) in self.conversation.queued_prompts.iter().enumerate() {
            let preview = short_text(&queued.display, 96);
            let for_copy = queued.display.clone();
            let mode = queued.mode;
            let can_steer = running && mode == crate::submit::SubmitMode::Queue;
            let steer_label = if can_steer {
                "插话发送"
            } else {
                "排队中"
            };
            list = list.child(
                row()
                    .id(gpui::ElementId::Name(format!("queue-{index}").into()))
                    .w_full()
                    .min_h(px(40.))
                    .px(px(12.))
                    .py(px(8.))
                    .gap(px(8.))
                    .items_center()
                    .rounded(px(crate::tokens::Tokens::RADIUS_LG))
                    .border(px(0.5))
                    .border_color(p.line)
                    .bg(p.stage)
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(6.))
                            .py(px(1.))
                            .rounded_full()
                            .border(px(0.5))
                            .border_color(p.line)
                            .text_size(px(10.))
                            .line_height(px(14.))
                            .text_color(p.label_caption)
                            .child(mode.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .text_color(p.secondary)
                            .child(preview),
                    )
                    .child(if can_steer {
                        div()
                            .id(gpui::ElementId::Name(format!("queue-steer-{index}").into()))
                            .role(gpui::Role::Button)
                            .aria_label(steer_label)
                            .size(px(24.))
                            .rounded(px(8.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|style| style.bg(p.hover))
                            .child(icon(Glyph::Send, 13., p.faint))
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.steer_queued_prompt(index, w, cx);
                            }))
                            .into_any_element()
                    } else {
                        // 没有运行中的回合时插话没有落点，占位保持行对齐。
                        div().size(px(24.)).into_any_element()
                    })
                    .child(
                        div()
                            .id(gpui::ElementId::Name(format!("queue-copy-{index}").into()))
                            .role(gpui::Role::Button)
                            .aria_label("复制排队消息")
                            .size(px(24.))
                            .rounded(px(8.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|style| style.bg(p.hover))
                            .child(icon(Glyph::Copy, 13., p.faint))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                bridge::copy_text(&for_copy);
                                this.push_toast("已复制", cx);
                            })),
                    )
                    .child(
                        div()
                            .id(gpui::ElementId::Name(format!("queue-edit-{index}").into()))
                            .role(gpui::Role::Button)
                            .aria_label("取回编辑")
                            .size(px(24.))
                            .rounded(px(8.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|style| style.bg(p.hover))
                            .child(icon(Glyph::Edit, 13., p.faint))
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.edit_queued_prompt(index, w, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(gpui::ElementId::Name(format!("queue-del-{index}").into()))
                            .role(gpui::Role::Button)
                            .aria_label("删除排队消息")
                            .size(px(24.))
                            .rounded(px(8.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|style| style.bg(p.hover))
                            .child(icon(Glyph::Close, 13., p.faint))
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.remove_queued_prompt(index, w, cx);
                            })),
                    ),
            );
        }
        list
    }

    /// 官方草稿附件轨：64px 高，图片方块 + 240px 文件卡，可移除。
    fn render_attachment_rail(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let mut rail = row()
            .id("attachment-rail")
            .w_full()
            .h(px(72.))
            .px(px(12.))
            .gap(px(8.))
            .items_center()
            .overflow_x_hidden();
        for item in &self.conversation.draft_attachments {
            let id = item.id.clone();
            rail = rail.child(match item.kind {
                AttachmentKind::Image => {
                    let label = item.name.clone();
                    let thumb = if let Some(preview) = item
                        .preview
                        .as_deref()
                        .filter(|data| data.starts_with("data:image/"))
                    {
                        gpui::img(preview.to_string())
                            .size_full()
                            .object_fit(gpui::ObjectFit::Cover)
                            .into_any_element()
                    } else {
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.))
                            .text_color(p.faint)
                            .child("IMG")
                            .into_any_element()
                    };
                    column()
                        .relative()
                        .size(px(64.))
                        .flex_none()
                        .rounded(px(crate::tokens::Tokens::RADIUS_LG))
                        .border(px(0.5))
                        .border_color(p.line)
                        .bg(p.md_code_block)
                        .overflow_hidden()
                        .child(thumb)
                        .child(
                            div()
                                .id(gpui::ElementId::Name(format!("rm-att-{id}").into()))
                                .absolute()
                                .top(px(4.))
                                .right(px(4.))
                                .size(px(18.))
                                .rounded_full()
                                .bg(p.bg_mask_1)
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .child(icon(Glyph::Close, 10., p.ink))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.conversation
                                        .draft_attachments
                                        .retain(|item| item.id != id);
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .right_0()
                                .px(px(4.))
                                .py(px(2.))
                                .bg(p.bg_mask_1)
                                .truncate()
                                .text_size(px(9.))
                                .text_color(p.ink)
                                .child(label),
                        )
                }
                AttachmentKind::File => {
                    let name = item.name.clone();
                    let meta = {
                        let ext = name
                            .rsplit('.')
                            .next()
                            .filter(|part| part.len() <= 5 && *part != name)
                            .unwrap_or("FILE")
                            .to_uppercase();
                        format!("{ext} · {}", format_bytes(item.size))
                    };
                    row()
                        .relative()
                        .w(px(240.))
                        .h(px(64.))
                        .flex_none()
                        .px(px(12.))
                        .gap(px(10.))
                        .items_center()
                        .rounded(px(crate::tokens::Tokens::RADIUS_LG))
                        .border(px(0.5))
                        .border_color(p.line)
                        .bg(composer_bg(cx))
                        .child(icon(Glyph::FileText, 28., p.state_business_primary))
                        .child(
                            column()
                                .flex_1()
                                .min_w_0()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(14.))
                                        .line_height(px(22.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(name),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.))
                                        .line_height(px(15.))
                                        .text_color(p.faint)
                                        .child(meta),
                                ),
                        )
                        .child(
                            div()
                                .id(gpui::ElementId::Name(format!("rm-att-{id}").into()))
                                .size(px(22.))
                                .rounded(px(8.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|style| style.bg(p.hover))
                                .child(icon(Glyph::Close, 12., p.faint))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.conversation
                                        .draft_attachments
                                        .retain(|item| item.id != id);
                                    cx.notify();
                                })),
                        )
                }
            });
        }
        rail
    }

    fn render_send_button(&self, empty: bool, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let t = dsh(cx);
        let blue = t.button_info_fill;
        let ink = gpui::rgb(0xffffff);
        // 官方三态：生成中无文字 → 停止；生成中有文字 → 排队发送；空闲 → 发送。
        let stopping = self.conversation.running && empty;
        let queueing = self.conversation.running && !empty;
        let enabled = stopping
            || (!self.creating_session
                && !self.conversation.permission_mode.changing()
                && (self.conversation.running
                    || (self.connected()
                        && self.conversation.acp_open
                        && (!self.conversation.history_loading
                            || self.conversation.history_prepend_pending)
                        && !empty
                        && self.active_session.is_some())));
        let mut button = div()
            .id(if self.conversation.running {
                if stopping {
                    "stop-prompt"
                } else {
                    "queue-prompt"
                }
            } else {
                "send-prompt"
            })
            .role(gpui::Role::Button)
            .aria_label(if stopping {
                "停止生成"
            } else if queueing {
                "排队发送"
            } else {
                "发送"
            })
            .tab_index(0)
            .tab_stop(enabled)
            .size(px(34.))
            .mt(px(-2.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(blue)
            .text_color(ink);
        button = if stopping {
            // 官方停止图标：3,3 起 10×10、rx3 的方块。
            button.child(div().size(px(10.)).rounded(px(3.)).bg(ink))
        } else {
            button.child(icon(Glyph::ArrowUp, 18., ink.into()))
        };
        if enabled {
            let hover_blue = t.button_info_hover;
            button = button
                .cursor_pointer()
                .hover(move |style| style.bg(hover_blue));
            if stopping {
                button.on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
            } else {
                button.on_click(cx.listener(|this, _, w, cx| this.submit_prompt(w, cx)))
            }
        } else {
            button.opacity(0.4).cursor_default()
        }
    }

    fn render_button_popup(
        &self,
        pane: ComposerMenu,
        model: &str,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if self.composer_menu != Some(pane) || self.show_settings || self.shortcuts.open {
            return None;
        }
        let model_menu = pane.is_model();
        let bounds = ComposerAnchor::default();
        let dismissal_bounds = bounds.clone();
        let model_width = model_menu.then(|| self.model_menu_width(pane, model, w, cx));
        let permission_height = self
            .conversation
            .permission_mode
            .state
            .as_ref()
            .map_or(px(0.), |state| px(8. + 34. * state.options.len() as f32));
        let p = dsh(cx);
        let (id, label) = if model_menu {
            ("composer-model-popup", "模型与思考强度")
        } else {
            ("composer-permission-popup", "审批模式")
        };
        let card = composer_popup_surface(id, cx)
            .role(gpui::Role::Menu)
            .aria_label(label)
            .when(model_menu, |card| card.overflow_hidden())
            .when(!model_menu, |card| {
                card.w_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.permission_menu.scroll)
            })
            .on_mouse_down_out(
                cx.listener(move |app, event: &gpui::MouseDownEvent, _, cx| {
                    if !dismissal_bounds
                        .get()
                        .is_some_and(|trigger| trigger.contains(&event.position))
                    {
                        app.composer_menu = None;
                        cx.notify();
                    }
                }),
            );
        let card = match pane {
            ComposerMenu::Model => card
                .child(self.model_root_row("模型", model, ComposerMenu::ModelChoices, cx))
                .when_some(
                    self.current_config_label("reasoning_effort"),
                    |card, label| {
                        card.child(self.model_root_row(
                            "思考强度",
                            &label,
                            ComposerMenu::ReasoningChoices,
                            cx,
                        ))
                    },
                ),
            ComposerMenu::ModelChoices => {
                let searchable = self.model_menu_searchable();
                card.when(searchable, |card| {
                    card.child(
                        row()
                            .id("model-search")
                            .w_full()
                            .h(px(28.))
                            .flex_none()
                            .mt(px(2.))
                            .mb(px(3.))
                            .px(px(7.))
                            .gap(px(4.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(24.))
                                    .child(self.model_filter_editor.clone()),
                            )
                            .when(!self.model_menu.query.is_empty(), |search| {
                                search.child(
                                    div()
                                        .id("clear-model-search")
                                        .role(gpui::Role::Button)
                                        .aria_label("清除搜索")
                                        .size(px(24.))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_full()
                                        .cursor_pointer()
                                        .hover(|style| style.bg(p.hover))
                                        .child(icon(Glyph::Close, 14., p.secondary))
                                        .on_click(cx.listener(|this, _, w, cx| {
                                            replace(&this.model_filter_editor, "", w, cx);
                                            this.model_filter_editor
                                                .update(cx, |editor, cx| editor.focus(w, cx));
                                        })),
                                )
                            }),
                    )
                })
                .child(self.render_config_choices(pane, searchable, cx))
            }
            ComposerMenu::ReasoningChoices => {
                card.child(self.render_config_choices(pane, false, cx))
            }
            ComposerMenu::Permission => {
                if let Some(state) = self.conversation.permission_mode.state.as_ref() {
                    card.children(state.options.iter().enumerate().map(|(index, option)| {
                        self.permission_option(
                            option,
                            &state.current_value,
                            self.permission_menu.active == Some(index),
                            cx,
                        )
                    }))
                } else {
                    card
                }
            }
            _ => unreachable!("button popover pane"),
        };
        // Use this frame's trigger layout and paint above the composer clips.
        Some(
            canvas(
                move |trigger, w, cx| {
                    bounds.set(Some(trigger));
                    let viewport = w.viewport_size();
                    let margin = px(12.);
                    let gap = px(if model_menu { 8. } else { 4. });
                    let max_width =
                        px(if model_menu { 420. } else { 360. }).min(viewport.width - margin * 2.);
                    let max_height = if model_menu {
                        px(360.).min(viewport.height - px(96.))
                    } else {
                        viewport.height - margin * 2.
                    };
                    if max_width <= px(0.) || max_height <= px(0.) {
                        return;
                    }
                    let above = trigger.top() - gap - margin;
                    let below = viewport.height - trigger.bottom() - gap - margin;
                    let opens_below = !model_menu && above < permission_height && below > above;
                    let available = if opens_below { below } else { above };
                    if available <= px(0.) {
                        return;
                    }
                    let min_width = px(if model_menu { 240. } else { 144. }).min(max_width);
                    let mut card = card
                        .min_w(min_width)
                        .max_w(max_width)
                        .max_h(max_height.min(available))
                        .when_some(model_width, |card, width| {
                            card.w(width.clamp(min_width, max_width))
                        })
                        .into_any_element();
                    let measured = card.layout_as_root(
                        size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                        w,
                        cx,
                    );
                    let measured = if model_menu {
                        measured
                    } else {
                        // Fill the resolved card width even when the minimum width
                        // exceeds the intrinsic width of the permission labels.
                        card.layout_as_root(
                            size(
                                AvailableSpace::Definite(measured.width),
                                AvailableSpace::MaxContent,
                            ),
                            w,
                            cx,
                        )
                    };
                    let x = if model_menu {
                        trigger.right() - measured.width
                    } else {
                        trigger.left()
                    }
                    .clamp(
                        margin,
                        (viewport.width - margin - measured.width).max(margin),
                    );
                    let y = if opens_below {
                        trigger.bottom() + gap
                    } else {
                        trigger.top() - gap - measured.height
                    };
                    w.defer_draw(card, point(x.round(), y.round()), 2, None);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
    }

    fn render_composer_overlay(
        &self,
        menu: ComposerMenu,
        max_height: gpui::Pixels,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        match menu {
            ComposerMenu::PermissionChoices | ComposerMenu::CommandModels => {
                composer_overlay_card("command-picker", max_height, cx)
                    .left_0()
                    .right_0()
                    .overflow_hidden()
                    .child(self.render_command_picker(menu, cx))
            }
            ComposerMenu::Commands => {
                let catalog = self.command_catalog.catalog();
                let visible = visible_commands(&catalog, &self.command_filter);
                let mut list = column()
                    .id("command-list")
                    .w_full()
                    .min_h_0()
                    .gap_0()
                    .overflow_y_scroll();
                let mut index = 0usize;
                for section in ["添加", "指令"] {
                    let rows = visible
                        .iter()
                        .filter(|spec| spec.section == section)
                        .collect::<Vec<_>>();
                    if rows.is_empty() {
                        continue;
                    }
                    list = list.child(command_section_title(section, section == "指令", cx));
                    for spec in rows {
                        let selected = self.command_from_slash && index == self.command_selected;
                        list = list.child(self.command_row(spec, selected, cx));
                        index += 1;
                    }
                }
                composer_overlay_card("commands-menu", max_height, cx)
                    .left_0()
                    .right_0()
                    .overflow_hidden()
                    .child(list)
            }
            ComposerMenu::Files => {
                let files = self.visible_files();
                let mut list = column().w_full().gap_0().child(command_section_title(
                    "文件与文件夹",
                    false,
                    cx,
                ));
                if self.mention_files.loading() && self.mention_files.entries.is_empty() {
                    list = list
                        .child(file_status_row("正在读取工作区…", cx))
                        .child(file_status_row("正在读取工作区…", cx));
                }
                if let Some(error) = &self.mention_files.error {
                    list = list.child(file_status_row(error, cx));
                }
                for (index, file) in files.iter().cloned().enumerate() {
                    list = list.child(self.file_row(file, index, index == self.file_selected, cx));
                }
                if !self.mention_files.loading()
                    && self.mention_files.error.is_none()
                    && files.is_empty()
                {
                    list = list.child(file_status_row(
                        if self.file_filter.is_empty() {
                            "此目录为空"
                        } else {
                            "没有匹配的文件或文件夹"
                        },
                        cx,
                    ));
                }
                if self.mention_files.truncated {
                    list = list.child(file_status_row("仅显示前 200 项，可继续输入以筛选", cx));
                }
                composer_overlay_card("files-menu", max_height, cx)
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
            _ => unreachable!("button menus use their own anchor"),
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
            .border_color(dsh(cx).line)
            .child(self.file_crumb("工作区".to_string(), String::new(), false, cx));
        let mut directory = String::new();
        let parts = self
            .mention_files
            .directory
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
                        .child(icon(Glyph::Arrow, 10., dsh(cx).faint)),
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
        let p = dsh(cx);
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
        let p = dsh(cx);
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
            .rounded(px(12.))
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
        let p = dsh(cx);
        let value = value.to_string();
        let index = usize::from(target == ComposerMenu::ReasoningChoices);
        row()
            .id(gpui::ElementId::Name(format!("model-root-{label}").into()))
            .role(gpui::Role::MenuItem)
            .aria_label(format!("{label}，{value}"))
            .tab_stop(true)
            .when(self.model_menu.active == Some(index), |row| {
                row.track_focus(&self.model_menu.row).bg(p.hover)
            })
            .w_full()
            .h(px(34.))
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(12.))
            .cursor_pointer()
            .hover(|style| style.bg(p.hover))
            .focus_visible(|style| style.bg(p.hover))
            .text_size(px(13.))
            .line_height(px(20.))
            .text_color(p.label_primary)
            .child(div().flex_none().child(label))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_right()
                    .text_color(p.label_tertiary)
                    .child(value),
            )
            .child(
                svg()
                    .data(include_bytes!("../../assets/header-chevron-right.svg"))
                    .size(px(12.))
                    .flex_none()
                    .text_color(p.label_caption),
            )
            .on_click(cx.listener(move |this, _, w, cx| this.drill_model_menu(target, w, cx)))
    }

    /// 模型/推理选项按提供商分组；无分组的平铺进“模型”一组。
    pub(super) fn model_choice_groups(&self, config_id: &str) -> Vec<(String, Vec<PickerOption>)> {
        let Some(option) = self.config_option(config_id) else {
            return Vec::new();
        };
        let mut groups: Vec<(String, Vec<PickerOption>)> = Vec::new();
        let mut ungrouped = None;
        for item in option
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(nested) = item.get("options").and_then(Value::as_array) {
                let name = item
                    .get("name")
                    .or_else(|| item.get("group"))
                    .and_then(Value::as_str)
                    .unwrap_or("模型")
                    .to_string();
                let choices = nested.iter().filter_map(picker_choice).collect::<Vec<_>>();
                if !choices.is_empty() {
                    groups.push((name, choices));
                }
            } else if let Some(choice) = picker_choice(item) {
                let index = *ungrouped.get_or_insert_with(|| {
                    groups.push(("模型".to_string(), Vec::new()));
                    groups.len() - 1
                });
                groups[index].1.push(choice);
            }
        }
        groups
    }

    fn render_config_choices(
        &self,
        pane: ComposerMenu,
        searchable: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let config_id = super::model_menu::config_id(pane);
        let groups = self.model_menu_groups(pane, cx);
        if groups.is_empty() {
            let message = if pane == ComposerMenu::ReasoningChoices {
                "当前模型未提供思考强度。"
            } else if self.model_menu.query.is_empty() {
                "暂无可用模型"
            } else {
                "没有匹配的模型。"
            };
            return column()
                .w_full()
                .px(px(7.))
                .py(px(5.))
                .child(note(message, cx));
        }
        let current = self
            .config_option(config_id)
            .and_then(|option| option["currentValue"].as_str());
        let ungrouped = groups.len() == 1 && groups[0].0 == "模型";
        let active = self.model_menu.active.unwrap_or(0);
        let busy = self.model_menu_busy();
        let mut list = column()
            .id("model-menu-choices")
            .role(gpui::Role::Menu)
            .w_full()
            .min_h_0()
            .gap_0()
            .overflow_y_scroll()
            .track_scroll(&self.model_menu.viewport.scroll);
        let mut headings = Vec::new();
        let mut index = 0;
        for (name, choices) in groups {
            if !ungrouped {
                list = list.child(heading_space(headings.is_empty()));
                let child = index + headings.len();
                headings.push(MenuHeading {
                    child,
                    last_child: child + choices.len(),
                    label: name,
                });
            }
            for choice in choices {
                let p = dsh(cx);
                let value = choice.value.to_string();
                let label = choice.label;
                let selected = current == Some(value.as_str());
                let pending = self
                    .conversation
                    .model_menu_pending
                    .as_ref()
                    .is_some_and(|(target, requested)| *target == pane && *requested == value);
                list = list.child(
                    row()
                        .id(gpui::ElementId::Name(
                            format!("{config_id}-choice-{value}").into(),
                        ))
                        .role(gpui::Role::MenuItemRadio)
                        .aria_label(label.clone())
                        .aria_selected(selected)
                        .when(index == active && !busy, |row| {
                            row.bg(p.hover).when(!searchable, |row| {
                                row.track_focus(&self.model_menu.row).tab_stop(true)
                            })
                        })
                        .w_full()
                        .flex_none()
                        .h(px(34.))
                        .px(px(7.))
                        .gap(px(6.))
                        .rounded(px(12.))
                        .when(busy, |row| row.text_color(p.label_caption))
                        .when(!busy, |row| row.cursor_pointer())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.))
                                .line_height(px(18.))
                                .child(label.clone()),
                        )
                        .child(
                            div()
                                .w(px(14.))
                                .flex_none()
                                .flex()
                                .justify_center()
                                .when(pending, |mark| {
                                    mark.child(pending_dot(
                                        "model-option-pending",
                                        div().size(px(7.)).rounded_full().bg(p.label_caption),
                                    ))
                                })
                                .when(selected && !pending, |mark| {
                                    mark.child(icon(Glyph::Check, 14., p.ink))
                                }),
                        )
                        .on_mouse_move(cx.listener(move |this, _, w, cx| {
                            this.hover_model_choice(pane, index, w, cx)
                        }))
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.choose_model_menu(pane, value.clone(), w, cx)
                        }))
                        .when(pane == ComposerMenu::ModelChoices, |row| {
                            row.tooltip(move |_, cx| {
                                cx.new(|_| TurnActionHint(label.clone())).into()
                            })
                        }),
                );
                index += 1;
            }
        }
        self.model_menu.viewport.render(list, headings)
    }

    fn permission_option(
        &self,
        choice: &super::permission_picker::PermissionOption,
        current: &str,
        active: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let id = choice.value.clone();
        let label = choice.label().to_owned();
        let selected = current == id;
        row()
            .id(gpui::ElementId::Name(format!("permission-{id}").into()))
            .role(gpui::Role::MenuItem)
            .aria_label(if selected {
                format!("{label}，当前审批模式")
            } else if matches!(id.as_str(), "danger-full-access" | "auto") {
                format!("{label}，需要确认风险")
            } else {
                label.clone()
            })
            .aria_selected(selected)
            .when(active, |row| {
                row.track_focus(&self.permission_menu.row).bg(p.hover)
            })
            .w_full()
            .h(px(34.))
            .flex_none()
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(12.))
            .text_size(px(13.))
            .line_height(px(20.))
            .cursor_pointer()
            .hover(|style| style.bg(p.hover))
            .children(permission_icon(&id, 14., p.secondary))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when(id == "auto", |element| {
                element.child(
                    div()
                        .flex_none()
                        .text_size(px(8.))
                        .text_color(p.faint)
                        .child("EXP"),
                )
            })
            .when(selected, |element| {
                element.child(icon(Glyph::Check, 14., p.ink))
            })
            .on_click(cx.listener(move |this, _, w, cx| this.choose_permission(id.clone(), w, cx)))
    }

    fn command_row(
        &self,
        spec: &CommandSpec,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let command = spec.key;
        let title = spec.title;
        let description = spec.description;
        let availability = if spec.enabled {
            ""
        } else {
            "，ACP 暂不支持"
        };
        let mut item = row()
            .id(gpui::ElementId::Name(format!("command-{command}").into()))
            .role(gpui::Role::Button)
            .aria_label(format!("{title} {command}，{description}{availability}"))
            .tab_index(0)
            .tab_stop(spec.enabled)
            .w_full()
            .h(px(34.))
            .flex_none()
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(12.))
            .text_size(px(13.))
            .line_height(px(20.))
            .when(selected, |element| element.bg(p.hover))
            .child(icon(spec.glyph, 14., p.secondary))
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
        if spec.enabled {
            item = item
                .cursor_pointer()
                .hover(|style| style.bg(p.hover))
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.activate_command(command, w, cx);
                }));
        } else {
            item = item.opacity(0.45).cursor_default();
        }
        item
    }

    pub(super) fn current_config_label(&self, id: &str) -> Option<String> {
        let option = self.config_option(id)?;
        let current = option.get("currentValue").and_then(Value::as_str)?;
        config_choices(option)
            .into_iter()
            .find(|choice| choice.value.as_ref() == current)
            .map(|choice| choice.label.to_string())
            .or_else(|| {
                Some(if id == "model" {
                    model_choice_name(current, None)
                } else {
                    current.to_string()
                })
            })
    }

    fn render_more_menu(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let left = 12.;
        column()
            .id("sidebar-more-menu")
            .absolute()
            .left(px(left))
            .bottom(px(56.))
            .w(px(240.))
            .gap(px(2.))
            .p(px(4.))
            .rounded(px(16.))
            .bg(menu_bg(cx))
            .shadow(elevation_prominent())
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.show_more_menu = false;
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.show_more_menu = false;
                    cx.notify();
                }
            }))
            .child(account_menu_row(
                "sidebar-more-menu-settings",
                "设置",
                Glyph::Settings,
                true,
                cx,
                |this, _, cx| {
                    this.show_more_menu = false;
                    this.settings_page = SettingsPage::Credentials;
                    this.show_settings = true;
                    cx.notify();
                },
            ))
    }

    fn render_developer_tools_setting(&self, w: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let enabled = self.developer_tools;
        let busy = self.developer_tools_loading || self.developer_tools_saving;
        let target = if enabled { 16. } else { 0. };
        let mut offset = target;
        if let Some(start) = self
            .developer_tools_switched_at
            .filter(|_| !cx.reduce_motion())
        {
            let progress = ((bridge::monotonic_ms() - start) / 120.).clamp(0., 1.);
            if progress < 1. {
                // CSS ease: cubic-bezier(.25, .1, .25, 1).
                let (mut low, mut high) = (0., 1.);
                for _ in 0..16 {
                    let t = (low + high) / 2.;
                    let x = 0.75 * t * (1. - t) + t * t * t;
                    if x < progress {
                        low = t;
                    } else {
                        high = t;
                    }
                }
                let t = (low + high) / 2.;
                let y = 0.3 * t * (1. - t) * (1. - t) + t * t * (3. - 2. * t);
                offset = (16. - target) + (2. * target - 16.) * y as f32;
                w.request_animation_frame();
            }
        }
        let label = "代码工作工具";
        let switch = div()
            .id("developer-tools")
            .role(gpui::Role::Switch)
            .aria_label(label)
            .aria_toggled(enabled.into())
            .tab_stop(!busy)
            .relative()
            .flex_none()
            .w(px(36.))
            .h(px(20.))
            .rounded_full()
            .bg(if enabled {
                p.button_primary_fill
            } else {
                p.border_l3
            })
            .when(busy, |switch| switch.opacity(0.5))
            .when(!busy, |switch| {
                switch
                    .cursor_pointer()
                    .focus_visible(|style| style.border_1().border_color(p.state_business_primary))
            })
            .child(
                div()
                    .absolute()
                    .left(px(2. + offset))
                    .top(px(2.))
                    .size(px(16.))
                    .rounded_full()
                    .bg(if enabled {
                        p.label_primary_foreground
                    } else {
                        p.switch_thumb
                    }),
            );
        row()
            .w_full()
            .gap(px(24.))
            .py(px(16.))
            .border_b(px(0.5))
            .border_color(p.border_l2)
            .child(
                column()
                    .flex_1()
                    .gap(px(4.))
                    .child(div().text_size(px(14.)).line_height(px(20.)).child(label))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(p.label_secondary)
                            .child("控制界面展示与 HTML 预览权限，不影响 Host 授权或会话记录"),
                    )
                    .when(self.developer_tools_error, |content| {
                        content.child(
                            div()
                                .id("developer-tools-error")
                                .role(gpui::Role::Alert)
                                .child("保存失败，请重试"),
                        )
                    }),
            )
            .child(super::dock_controls::dock_action_button(
                switch,
                cx.listener(|this, _, _, cx| {
                    if !this.developer_tools_loading && !this.developer_tools_saving {
                        this.developer_tools_saving = true;
                        this.developer_tools_error = false;
                        invoke(
                            "save_developer_tools",
                            json!({"enabled": !this.developer_tools}),
                        );
                        cx.notify();
                    }
                }),
            ))
    }

    /// Restores workspaces hidden from the sidebar. Harness keeps session data
    /// when a group is removed, so undoing the local `hidden` preference is all
    /// that is needed — no files or history are touched.
    fn render_hidden_workspaces(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let mut hidden: Vec<String> = self.hidden_workspaces.iter().cloned().collect();
        hidden.sort();
        let mut body = column().w_full().gap(px(8.)).child(
            div()
                .text_size(px(14.))
                .line_height(px(20.))
                .text_color(p.label_primary)
                .child("已移除的工作区"),
        );
        if hidden.is_empty() {
            return body.child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(p.label_tertiary)
                    .child("暂无；从侧栏移除的工作区会出现在这里，可随时恢复，磁盘文件与会话不受影响。"),
            );
        }
        for (index, path) in hidden.iter().enumerate() {
            let restored = path.clone();
            body = body.child(
                row()
                    .w_full()
                    .h(px(36.))
                    .px(px(12.))
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .rounded(px(8.))
                    .bg(p.bg_module_platform)
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(p.label_secondary)
                            .child(path.clone()),
                    )
                    .child(
                        div()
                            .id(gpui::ElementId::Name(
                                format!("restore-workspace-{index}").into(),
                            ))
                            .role(gpui::Role::Button)
                            .px(px(10.))
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .bg(p.button_primary_fill)
                            .text_size(px(12.))
                            .text_color(p.label_primary_foreground)
                            .child("恢复")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.hidden_workspaces.remove(&restored);
                                this.save_workspace_preferences(cx);
                                this.push_toast("已恢复工作区分组", cx);
                                cx.notify();
                            })),
                    ),
            );
        }
        body
    }

    fn render_settings(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.settings_page != SettingsPage::Plugins {
            self.plugin_center.close_inventory();
        }
        let p = dsh(cx);
        let (status, detail) = match self.status {
            ServiceStatus::Starting => ("正在连接", "正在启动 ACP 服务"),
            ServiceStatus::Connected => ("服务已连接", self.agent_name.as_str()),
            ServiceStatus::Error => ("连接异常", "请检查路径后重新连接"),
        };
        let page = self.settings_page;
        let compact = w.viewport_size().width < px(720.);
        let content: gpui::Div = match page {
            SettingsPage::Credentials => {
                let controls = self.welcome.update(cx, |welcome, cx| welcome.render_inline(cx));
                settings_section("API Key", controls, cx)
            }
            SettingsPage::General => settings_section(
                "通用设置",
                column()
                    .w_full()
                    .gap(px(14.))
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
                            .disabled(self.conversation.running || self.creating_session || self.plugin_install_busy())
                            .on_click(cx.listener(|this, _, w, cx| this.start_service(w, cx)))
                            .into_themed(cx),
                    )
                    .child(note(format!("{status} · {detail}"), cx))
                    .child(self.render_shortcut_setting(cx))
                    .child(self.render_transcript_preferences(w, cx))
                    .child(self.render_developer_tools_setting(w, cx))
                    .child(self.render_hidden_workspaces(cx))
                    .child(note("当前版本 0.1.0（GPUI 客户端）", cx)),
                cx,
            ),
            SettingsPage::Appearance => settings_section(
                "外观",
                column()
                    .w_full()
                    .gap(px(14.))
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
                    .child(labeled_control(
                        "正文字号",
                        Some(
                            row()
                                .w_full()
                                .gap(px(8.))
                                .items_center()
                                .child(
                                    div()
                                        .id("font-dec")
                                        .role(gpui::Role::Button)
                                        .aria_label("减小字号")
                                        .size(px(28.))
                                        .rounded(px(8.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .bg(p.stage)
                                        .hover(|style| style.bg(p.hover))
                                        .child("−")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.set_content_font_delta(
                                                this.content_font_delta - 1,
                                                cx,
                                            );
                                        })),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(self.content_font_size(14.)))
                                        .line_height(px(self.content_font_size(14.) + 10.))
                                        .child(format!(
                                            "{}px（步进 {:+}）",
                                            self.content_font_size(14.) as u32,
                                            self.content_font_delta
                                        )),
                                )
                                .child(
                                    div()
                                        .id("font-inc")
                                        .role(gpui::Role::Button)
                                        .aria_label("增大字号")
                                        .size(px(28.))
                                        .rounded(px(8.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .bg(p.stage)
                                        .hover(|style| style.bg(p.hover))
                                        .child("+")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.set_content_font_delta(
                                                this.content_font_delta + 1,
                                                cx,
                                            );
                                        })),
                                ),
                        ),
                        cx,
                    )),
                cx,
            ),
            SettingsPage::Model => settings_section(
                "模型与推理",
                column()
                    .w_full()
                    .gap(px(14.))
                    .child(note("选择当前会话使用的模型和思考强度。", cx))
                    .child(super::permission_dialog::dialog_action(
                        "open-model-providers", "管理模型服务", false, false, 0., cx,
                        |app, w, cx| app.open_model_providers(w, cx),
                    ))
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
            SettingsPage::Providers => self.render_model_providers(compact, cx),
            SettingsPage::Plugins => {
                let inventory = self.render_plugin_inventory(w, cx);
                settings_section("内置插件", inventory, cx)
            }
            SettingsPage::AgentPresets => settings_section(
                "Agent 预设",
                column()
                    .w_full()
                    .gap(px(12.))
                    .child(wrapped_settings_note(
                        "配套 dsh-acp 的 ACP profile 未挂载 agent-preset-registry，当前无法读取或切换 Agent 预设，也无法按指定预设创建自动化会话。工具执行仍会在会话中逐项请求审批。",
                        cx,
                    )),
                cx,
            ),
            SettingsPage::SessionLog => settings_section(
                "会话日志",
                wrapped_settings_note(
                    "官方会话日志上传偏好依赖 Host；当前客户端不上传本地 transcript。",
                    cx,
                ),
                cx,
            ),
        };
        let nav = [
            (SettingsPage::Credentials, "API Key", Glyph::Lock),
            (SettingsPage::General, "通用设置", Glyph::Settings),
            (SettingsPage::Appearance, "外观", Glyph::Sun),
            (SettingsPage::Model, "模型", Glyph::Card),
            (SettingsPage::Providers, "模型服务", Glyph::Globe),
            (SettingsPage::Plugins, "内置插件", Glyph::Bulb),
            (SettingsPage::AgentPresets, "Agent 预设", Glyph::Archive),
            (SettingsPage::SessionLog, "会话日志", Glyph::Book),
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
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.transcript_preferences.clear_menu();
                        if target == SettingsPage::Providers {
                            this.open_model_providers(w, cx);
                        } else {
                            this.settings_page = target;
                            cx.notify();
                        }
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
            .p(px(if compact { 12. } else { 24. }))
            .occlude()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close_settings(cx);
                }
            }))
            .child(
                div()
                    .id("settings-mask")
                    .absolute()
                    .inset_0()
                    .bg(p.bg_mask_1)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.close_settings(cx);
                    })),
            )
            .child(
                row()
                    .id("settings-panel")
                    .relative()
                    .w_full()
                    .max_w(px(if page == SettingsPage::Providers {
                        1000.
                    } else {
                        800.
                    }))
                    .h_full()
                    .max_h(px(800.))
                    .flex_none()
                    .gap_0()
                    .overflow_hidden()
                    .rounded(px(28.))
                    .bg(composer_bg(cx))
                    .shadow(elevation_prominent())
                    .occlude()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        column()
                            .w(px(if compact { 132. } else { 188. }))
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
                                            this.close_settings(cx);
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .id("settings-content")
                                    .flex_1()
                                    .min_h_0()
                                    .w_full()
                                    .when(page != SettingsPage::Providers, |content| {
                                        content.overflow_y_scroll()
                                    })
                                    .px(px(if compact { 12. } else { 24. }))
                                    .pb(px(if compact { 12. } else { 24. }))
                                    .child(content),
                            ),
                    ),
            )
            .when(page == SettingsPage::General, |overlay| {
                overlay.children(self.render_transcript_preference_menu(w, cx))
            })
    }

    fn brand_mark(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
        size: f32,
    ) -> gpui::Stateful<gpui::Div> {
        let hover = self.brand_hover.get(w, cx).clamp(0., 1.);
        let mark = if hover > 0.5 {
            animated_brand_mark("hero-kense", size, dark(cx)).into_any_element()
        } else {
            crate::brand::mark(size, dark(cx)).into_any_element()
        };
        div()
            .id(gpui::ElementId::Name(format!("brand-{size}").into()))
            .flex_none()
            .on_hover(cx.listener(|this, inside, _, cx| {
                this.brand_hover.set(if *inside { 1. } else { 0. }, cx);
                cx.notify();
            }))
            .child(mark)
    }
}

fn account_menu_row(
    id: &'static str,
    label: &'static str,
    glyph: Glyph,
    enabled: bool,
    cx: &mut Context<DshApp>,
    action: impl Fn(&mut DshApp, &mut Window, &mut Context<DshApp>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let p = dsh(cx);
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
        .rounded(px(12.))
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
        .text_color(dsh(cx).faint)
        .child(label)
}

/// 发送按钮启动 spinner 的弧线（官方 .8s conic 等效：白弧 + 线性旋转）。
pub(super) const SPINNER_ARC_SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 20 20\"><circle cx=\"10\" cy=\"10\" r=\"8\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-dasharray=\"12 38\" stroke-linecap=\"round\"/></svg>";

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
        .text_color(dsh(cx).faint)
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
        .text_color(dsh(cx).faint)
        .child(label.to_string())
}

fn composer_label_width(label: &str, weight: FontWeight, w: &Window, cx: &App) -> gpui::Pixels {
    let mut style = super::document_fonts::interface_style(w, cx);
    style.font_weight = weight;
    w.text_system()
        .shape_line(
            label.to_owned().into(),
            px(13.),
            &[style.to_run(label.len())],
            None,
        )
        .width()
}

fn measure_composer_anchor(bounds: ComposerAnchor, open: bool) -> impl IntoElement {
    canvas(
        move |measured, w, _| {
            let previous = bounds.replace(Some(measured));
            if open && previous != Some(measured) {
                w.request_animation_frame();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

fn composer_chevron(
    motion: &mut DockSlide,
    open: bool,
    color: gpui::Hsla,
    w: &Window,
    cx: &App,
) -> gpui::Svg {
    let (turn, running) = motion.update(open, bridge::monotonic_ms(), !cx.reduce_motion());
    if running {
        w.request_animation_frame();
    }
    svg()
        .data(include_bytes!("../../assets/header-chevron.svg"))
        .size(px(12.))
        .flex_none()
        .text_color(color)
        .with_transformation(
            gpui::Transformation::default()
                .with_rotation(gpui::radians(turn * std::f32::consts::PI)),
        )
}

fn composer_focus_ring(color: gpui::Hsla) -> gpui::Div {
    div()
        .absolute()
        .top(px(-2.))
        .right(px(-2.))
        .bottom(px(-2.))
        .left(px(-2.))
        .rounded(px(10.))
        .border(px(2.))
        .border_color(color)
}

fn composer_popup_surface(id: &'static str, cx: &App) -> gpui::Stateful<gpui::Div> {
    let p = dsh(cx);
    column()
        .id(id)
        .gap_0()
        .p(px(4.))
        .rounded(px(16.))
        .child(menu_material(p, 16.))
        .shadow(menu_elevation(p))
        .occlude()
}

fn composer_overlay_card(
    id: &'static str,
    max_height: gpui::Pixels,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    composer_popup_surface(id, cx)
        .absolute()
        .bottom(px(4.))
        .max_h(max_height)
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

fn wrapped_settings_note(text: &'static str, cx: &App) -> gpui::Div {
    let mut start = 0;
    // GPUI can wrap before a CJK closing mark. Keep each Unicode line-break
    // segment intact and let GPUI lay the segments out at the available width.
    let segments = unicode_linebreak::linebreaks(text).map(|(end, _)| {
        let segment = &text[start..end];
        start = end;
        note(segment, cx).flex_none().whitespace_nowrap()
    });
    row().w_full().gap_0().flex_wrap().children(segments)
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
        .child(div().h(px(1.)).w_full().bg(dsh(cx).line))
}

fn render_entry(
    entry: &ChatEntry,
    expanded: bool,
    copied_mark: Option<&str>,
    wrapped_code: &std::collections::HashSet<String>,
    font_delta: i32,
    show_thought_preview: bool,
    cx: &mut Context<DshApp>,
) -> gpui::Div {
    let p = dsh(cx);
    match entry.role {
        EntryRole::Plan => super::plan::plan_card(entry, cx),
        EntryRole::QuestionReply => {
            super::question_panel::render_question_reply(entry, copied_mark, font_delta, cx)
        }
        EntryRole::User if entry.title == "goal" || entry.title.starts_with("trigger:") => {
            render_turn_trigger(entry, expanded, cx)
        }
        EntryRole::User => {
            // User message actions show its timestamp and copy control.
            let key = entry.key.clone();
            let group = format!("user-row-{key}");
            let copied = copied_mark == Some(key.as_str());
            let time = entry.at_ms.map(bridge::time_label);
            let for_copy = entry.text.clone();
            row().w_full().justify_end().child(
                column()
                    .id(gpui::ElementId::Name(format!("user-{key}").into()))
                    .group(group.clone())
                    .max_w(gpui::relative(0.702))
                    .items_end()
                    .gap(px(6.))
                    .child(
                        div()
                            .px(px(16.))
                            .py(px(10.))
                            .rounded(px(20.))
                            .bg(p.bubble)
                            .text_size(px(14.))
                            .line_height(px(22.))
                            .child(entry.text.clone()),
                    )
                    .child(
                        row()
                            .id(gpui::ElementId::Name(format!("user-actions-{key}").into()))
                            .h(px(28.))
                            .gap(px(2.))
                            .items_center()
                            .opacity(0.)
                            .group_hover(group.as_str(), |style| style.opacity(1.))
                            .when_some(time, |element, time| {
                                element.child(
                                    div()
                                        .flex_none()
                                        .px(px(4.))
                                        .text_size(px(13.))
                                        .line_height(px(24.))
                                        .text_color(p.faint)
                                        .child(time),
                                )
                            })
                            .child(
                                div()
                                    .id(gpui::ElementId::Name(format!("user-copy-{key}").into()))
                                    .role(gpui::Role::Button)
                                    .aria_label(if copied { "已复制" } else { "复制" })
                                    .tab_index(0)
                                    .size(px(28.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(8.))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(p.hover))
                                    .child(icon(
                                        if copied { Glyph::Check } else { Glyph::Copy },
                                        15.,
                                        if copied {
                                            p.state_business_primary
                                        } else {
                                            p.faint
                                        },
                                    ))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        bridge::copy_text(&for_copy);
                                        mark_copied(this, &key, cx);
                                        this.push_toast("已复制", cx);
                                    })),
                            ),
                    ),
            )
        }
        EntryRole::Assistant => div().w_full().child(render_markdown(
            &entry.text,
            &format!("md-{}", entry.key),
            copied_mark,
            wrapped_code,
            font_delta,
            None,
            cx,
        )),
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
                            "收起思考"
                        } else {
                            "展开思考"
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
                        .child(div().flex_none().text_size(px(13.)).child("思考"))
                        .when(
                            !expanded && show_thought_preview && !summary.is_empty(),
                            |element| {
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
                            },
                        )
                        .child(icon(
                            if expanded { Glyph::Up } else { Glyph::Down },
                            12.,
                            p.faint,
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            toggle_entry(&mut this.conversation.expanded_entries, &key);
                            cx.notify();
                        })),
                )
                .when(expanded, |element| {
                    element.child(div().pl(px(22.)).py(px(4.)).opacity(0.92).child(
                        render_markdown(
                            &entry.text,
                            &format!("thought-{}", entry.key),
                            copied_mark,
                            wrapped_code,
                            font_delta,
                            None,
                            cx,
                        ),
                    ))
                })
        }
        EntryRole::Tool => {
            // 官方工具行状态：正在准备调用 / 运行中 / 失败 / 已停止（完成只留对勾）。
            let (status_label, leading) = match entry.status.as_str() {
                "in_progress" => ("运行中", ToolLead::Spinner),
                "pending" | "preparing" => ("正在准备调用", ToolLead::Spinner),
                "completed" => ("", ToolLead::Check),
                "failed" => ("失败", ToolLead::Alert),
                "cancelled" | "stopped" => ("已停止", ToolLead::Stopped),
                "approval" => ("等待审批", ToolLead::Alert),
                _ => ("", ToolLead::None),
            };
            let title = if entry.title.is_empty() {
                "工具调用".to_string()
            } else {
                short_text(&entry.title, 52)
            };
            let summary = if entry.text.is_empty() {
                status_label.to_string()
            } else if status_label.is_empty() {
                short_text(&entry.text, 64)
            } else {
                format!("{status_label} · {}", short_text(&entry.text, 64))
            };
            let key = entry.key.clone();
            let failed = entry.status == "failed";
            let tool_glyph = match entry.title.as_str() {
                title if title.contains("edit") || title.contains("write") => Glyph::Edit,
                title if title.contains("patch") => Glyph::Edit,
                title if title.contains("read") => Glyph::Book,
                title if title.contains("image") => Glyph::Monitor,
                title if title.contains("search") || title.contains("find") => Glyph::Search,
                title if title.contains("bash") || title.contains("terminal") => Glyph::Play,
                title if title.contains("browser") || title.contains("web") => Glyph::Globe,
                title if title.contains("file") => Glyph::FileText,
                title if title.contains("folder") || title.contains("ls") => Glyph::Folder,
                _ => Glyph::Settings,
            };
            let title_color = if failed { p.danger } else { p.faint };
            let lead_element = match leading {
                ToolLead::Spinner => pending_dot(
                    gpui::ElementId::Name(format!("tool-lead-{key}").into()),
                    div()
                        .size(px(8.))
                        .rounded_full()
                        .bg(p.state_business_primary),
                )
                .into_any_element(),
                ToolLead::Check => {
                    icon(Glyph::Check, 13., p.state_business_primary).into_any_element()
                }
                ToolLead::Alert => icon(Glyph::Alert, 13., p.danger).into_any_element(),
                ToolLead::Stopped => div()
                    .size(px(9.))
                    .rounded(px(2.))
                    .bg(p.faint)
                    .into_any_element(),
                ToolLead::None => icon(tool_glyph, 14., p.faint).into_any_element(),
            };
            column()
                .w_full()
                .gap_0()
                .child(
                    row()
                        .id(gpui::ElementId::Name(format!("entry-{key}").into()))
                        .role(gpui::Role::Button)
                        .aria_label(if status_label.is_empty() {
                            format!("{title}，已完成")
                        } else {
                            format!("{title}，{status_label}")
                        })
                        .aria_expanded(expanded)
                        .tab_index(0)
                        .w_full()
                        .h(px(24.))
                        .gap(px(6.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .text_color(title_color)
                        .hover(|style| style.text_color(p.secondary))
                        .child(lead_element)
                        .child(div().flex_none().text_size(px(13.)).child(title))
                        .when(!summary.is_empty(), |element| {
                            element.child(div().size(px(2.)).mx(px(2.)).rounded_full().bg(p.faint))
                        })
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
                            toggle_entry(&mut this.conversation.expanded_entries, &key);
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
        EntryRole::Retry => render_model_retry(entry, expanded, cx),
        EntryRole::Command => render_command_card(entry, expanded, cx),
        EntryRole::Context => render_context_injection(entry, expanded, cx),
        EntryRole::ToolChange => render_tool_change(entry, expanded, cx),
        EntryRole::SystemPrompt => render_system_prompt(entry, expanded, cx),
        EntryRole::Job => render_job_row(entry, expanded, cx),
        EntryRole::System if entry.title == "turn-end" => row().w_full().justify_start().child(
            div()
                .px(px(6.))
                .rounded(px(6.))
                .bg(p.hover)
                .text_size(px(11.))
                .line_height(px(18.))
                .text_color(p.faint)
                .child(entry.text.clone()),
        ),
        EntryRole::System => row()
            .w_full()
            .justify_center()
            .child(note(entry.text.clone(), cx)),
        EntryRole::Error => render_turn_notice(entry, p.danger, true, cx),
        EntryRole::Warning => render_turn_notice(entry, p.state_warn_primary, false, cx),
    }
}

/// 官方 TurnErrorItem / TurnMaxTokensItem 共用的状态行：
/// 状态点 + 标题 + 说明文字 +（可选的错误码徽标）。
fn render_turn_notice(
    entry: &ChatEntry,
    tone: gpui::Hsla,
    show_code: bool,
    cx: &mut Context<DshApp>,
) -> gpui::Div {
    let p = dsh(cx);
    row()
        .w_full()
        .items_start()
        .gap(px(8.))
        .child(
            div()
                .flex_none()
                .mt(px(4.))
                .size(px(8.))
                .rounded_full()
                .bg(tone),
        )
        .child(
            column()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(tone)
                        .child(entry.title.clone()),
                )
                .when(!entry.text.is_empty(), |element| {
                    element.child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(tone)
                            .child(entry.text.clone()),
                    )
                }),
        )
        .when(show_code && !entry.status.is_empty(), |element| {
            element.child(
                div()
                    .flex_none()
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(6.))
                    .bg(p.hover)
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(p.faint)
                    .child(entry.status.clone()),
            )
        })
}

/// 官方 ui-jobs 行：状态点 + 类型 + 标签 +（详情或状态）+ 时长。
///
/// 官方那个组件是挂在会话头上的实时浮层（依赖 job controller 的流式 RPC）；
/// 这里渲染的是同一份数据在**对话流里的存档行**——来自持久化的
/// `job/registered` / `job/settled` 事件，所以重开旧会话也能看到。
fn render_job_row(entry: &ChatEntry, expanded: bool, cx: &mut Context<DshApp>) -> gpui::Div {
    let p = dsh(cx);
    let key = entry.key.clone();
    let live = matches!(entry.status.as_str(), "running" | "stopping");
    let status_label = match entry.status.as_str() {
        "running" => "运行中",
        "stopping" => "正在停止",
        "killed" => "已取消",
        "failed" => "已失败",
        _ => "已完成",
    };
    let tone = match entry.status.as_str() {
        "running" | "stopping" => p.state_business_primary,
        "failed" => p.danger,
        "killed" => p.state_warn_primary,
        _ => p.state_success_primary,
    };
    // 官方 `jobDetail`：进行中看 progress，结算后看 detail；没有就显示状态本身。
    let detail = if entry.text.is_empty() {
        status_label.to_owned()
    } else {
        format!("{status_label} · {}", entry.text)
    };
    let elapsed = job_elapsed_label(entry);
    let body = entry.title.clone();
    let body_id = format!("job-body-{key}");
    let toggle_key = key.clone();
    column()
        .w_full()
        .gap(px(2.))
        .child(
            row()
                .id(gpui::ElementId::Name(format!("job-{key}").into()))
                .role(gpui::Role::Button)
                .aria_label(detail.clone())
                .aria_expanded(expanded)
                .tab_index(0)
                .w_full()
                .gap(px(7.))
                .items_center()
                .rounded(px(8.))
                .px(px(2.))
                .cursor_pointer()
                .hover(|style| style.bg(p.hover))
                .text_color(p.faint)
                .child(div().flex_none().size(px(6.)).rounded_full().bg(tone))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .child(status_label),
                )
                .child(div().flex_none().size(px(2.)).rounded_full().bg(p.faint))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .child(body.clone()),
                )
                .when_some(elapsed, |row, elapsed| {
                    row.child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .child(elapsed),
                    )
                })
                .child(icon(
                    if expanded { Glyph::Up } else { Glyph::Down },
                    12.,
                    p.faint,
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    toggle_entry(&mut this.conversation.expanded_entries, &toggle_key);
                    cx.notify();
                })),
        )
        .child(
            div()
                .id(gpui::ElementId::Name(body_id.into()))
                .pl(px(21.))
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(p.faint)
                .when(!expanded, |element| {
                    element.max_h(px(18.)).overflow_hidden()
                })
                .child(if live {
                    // A running job's label is its command line; show it in full
                    // only when the reader asks for it.
                    body
                } else if entry.text.is_empty() {
                    detail
                } else {
                    entry.text.clone()
                }),
        )
}

/// 后台任务时长，对齐官方 `formatDuration`：最多两个相邻单位。
///
/// 需要在 `anim` 里算成文本，因为行本身只有起止时间戳。
fn job_elapsed_label(entry: &ChatEntry) -> Option<String> {
    let started = entry.at_ms?;
    let finished = entry
        .input
        .parse::<f64>()
        .ok()
        .unwrap_or_else(bridge::monotonic_ms);
    let total = (((finished - started).max(0.)) / 1000.).floor() as u64;
    let seconds = total % 60;
    let minutes = (total / 60) % 60;
    let hours = total / 3600;
    Some(if hours > 0 {
        format!("{hours}小时{minutes}分")
    } else if minutes > 0 {
        format!("{minutes}分{seconds}秒")
    } else {
        format!("{seconds}秒")
    })
}

/// 官方 ContextInjectionRow：折叠行，标题说明这段上下文的角色，展开看正文。
///
/// 技能目录是常驻的一类（`skill-catalog`）；它不显示为聊天气泡，也不触发一轮
/// 回复，只作为可展开的注入记录留在对话流里。
/// 官方 ContextInjectionRow 的工具变更变体：browse 图标 + 标题 + 摘要 + 展开后的
/// 新增/移除名单。单条变更不可展开（和官方一致）。
fn render_tool_change(entry: &ChatEntry, expanded: bool, cx: &mut Context<DshApp>) -> gpui::Div {
    let p = dsh(cx);
    let key = entry.key.clone();
    let body = entry.text.clone();
    let expandable = !body.trim().is_empty();
    let open = expanded && expandable;
    let toggle_key = key.clone();
    let mut row = row()
        .id(gpui::ElementId::Name(format!("tool-change-{key}").into()))
        .role(gpui::Role::Button)
        .aria_label(format!("{} {}", entry.title, entry.status))
        .aria_expanded(open)
        .tab_index(0)
        .tab_stop(expandable)
        .w_full()
        .gap(px(7.))
        .items_center()
        .rounded(px(8.))
        .h(px(24.))
        .px(px(2.))
        .when(expandable, |row| {
            row.cursor_pointer().hover(|style| style.bg(p.hover))
        })
        .text_color(p.faint)
        .child(icon(Glyph::Card, 14., p.faint))
        .child(
            div()
                .flex_none()
                .text_size(px(13.))
                .line_height(px(20.))
                .child(entry.title.clone()),
        )
        .when(!entry.status.is_empty(), |row| {
            row.child(div().flex_none().size(px(2.)).rounded_full().bg(p.faint))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .child(entry.status.clone()),
                )
        });
    if expandable {
        row = row
            .child(icon(
                if open { Glyph::Up } else { Glyph::Down },
                12.,
                p.faint,
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                toggle_entry(&mut this.conversation.expanded_entries, &toggle_key);
                cx.notify();
            }));
    }
    let mut card = column().w_full().child(row);
    if open {
        card = card.child(
            div()
                .w_full()
                .pl(px(21.))
                .pt(px(4.))
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(p.secondary)
                .child(body),
        );
    }
    card
}

fn render_context_injection(
    entry: &ChatEntry,
    expanded: bool,
    cx: &mut Context<DshApp>,
) -> gpui::Div {
    let p = dsh(cx);
    let key = entry.key.clone();
    let title = "上下文注入";
    let source = entry.title.clone();
    let body = entry.text.clone();
    let expandable = !body.trim().is_empty();
    let open = expanded && expandable;
    let toggle_key = key.clone();
    let mut row = row()
        .id(gpui::ElementId::Name(format!("context-{key}").into()))
        .role(gpui::Role::Button)
        .aria_label(format!("{title} {source}"))
        .aria_expanded(open)
        .tab_index(0)
        .tab_stop(expandable)
        .w_full()
        .gap(px(7.))
        .items_center()
        .rounded(px(8.))
        .h(px(24.))
        .px(px(2.))
        .when(expandable, |row| {
            row.cursor_pointer().hover(|style| style.bg(p.hover))
        })
        .text_color(p.faint)
        .child(
            gpui::svg()
                .data(include_bytes!("../../assets/context-injection.svg") as &[u8])
                .size(px(14.))
                .flex_none()
                .text_color(p.faint),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(13.))
                .line_height(px(20.))
                .child(title),
        )
        .child(div().flex_none().size(px(2.)).rounded_full().bg(p.faint))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .line_height(px(20.))
                .child(source),
        );
    if expandable {
        row = row
            .child(icon(
                if open { Glyph::Up } else { Glyph::Down },
                12.,
                p.faint,
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                toggle_entry(&mut this.conversation.expanded_entries, &toggle_key);
                cx.notify();
            }));
    }
    let mut card = column().w_full().child(row);
    if open {
        card = card.child(
            div()
                .id(gpui::ElementId::Name(format!("context-body-{key}").into()))
                .w_full()
                .max_h(px(260.))
                .overflow_y_scroll()
                .pl(px(21.))
                .pt(px(4.))
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(p.secondary)
                .child(body),
        );
    }
    card
}

/// 官方 GenericCommandCard：指令名 + 分隔点 + 折叠摘要，多行正文可展开。
///
/// 官方的规则是「正文含换行才可展开」，摘要取结果首行，因此 `/goal` 这类多行
/// 结果默认只显示一行、点开后看到完整的 `Status/Rounds/Activation` 文本。
fn render_command_card(entry: &ChatEntry, expanded: bool, cx: &mut Context<DshApp>) -> gpui::Div {
    let p = dsh(cx);
    let key = entry.key.clone();
    let running = entry.status.is_empty();
    let failed = entry.status == "error";
    let body = (!entry.text.is_empty()).then(|| entry.text.clone());
    // 官方：仅当结果含换行时 DisclosureRow 才可展开。
    let expandable = body.as_deref().is_some_and(|text| text.contains('\n'));
    let open = expanded && expandable;
    let summary = match (running, failed) {
        (true, _) => "正在执行指令".to_string(),
        (false, true) => "指令执行失败".to_string(),
        (false, false) => body
            .as_deref()
            .map(|text| text.lines().next().unwrap_or_default().trim().to_string())
            .filter(|line| !line.is_empty())
            .unwrap_or_else(|| "指令已执行".to_string()),
    };
    let title = if entry.title.is_empty() {
        "指令".to_string()
    } else {
        entry.title.clone()
    };
    let toggle_key = key.clone();
    let mut card = column()
        .w_full()
        .rounded(px(10.))
        .border(px(0.5))
        .border_color(p.line)
        .child(
            row()
                .id(gpui::ElementId::Name(format!("command-{key}").into()))
                .role(gpui::Role::Button)
                .aria_label(title.clone())
                .aria_expanded(open)
                .tab_index(0)
                .tab_stop(expandable)
                .w_full()
                .px(px(12.))
                .py(px(8.))
                .gap(px(8.))
                .items_center()
                .when(expandable, |row| {
                    row.cursor_pointer().hover(|style| style.bg(p.hover))
                })
                .child(icon(Glyph::Play, 13., p.faint))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(p.secondary)
                        .child(title),
                )
                .child(div().flex_none().size(px(2.)).rounded_full().bg(p.faint))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .when(failed, |element| element.text_color(p.danger))
                        .when(!failed, |element| element.text_color(p.faint))
                        .child(summary),
                )
                .children(running.then(|| {
                    pending_dot(
                        gpui::ElementId::Name(format!("command-dot-{key}").into()),
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .bg(p.state_business_primary),
                    )
                }))
                .when(expandable, |row| {
                    row.child(icon(
                        if open { Glyph::Up } else { Glyph::Down },
                        12.,
                        p.faint,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        toggle_entry(&mut this.conversation.expanded_entries, &toggle_key);
                        cx.notify();
                    }))
                }),
        );
    if open && let Some(body) = body {
        card = card.child(
            div()
                .id(gpui::ElementId::Name(format!("command-body-{key}").into()))
                .w_full()
                .max_h(px(260.))
                .overflow_y_scroll()
                .border_t(px(0.5))
                .border_color(p.line)
                .px(px(12.))
                .py(px(8.))
                .font_family("Menlo")
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(p.ink)
                .child(body),
        );
    }
    card
}

/// 官方 SystemPromptRow：本轮完整系统提示词的折叠行。
///
/// 默认收起，只占一行；展开后是模型可见的原文（真实换行、超长可滚动），
/// 与上下文注入行同一个正文外壳。
fn render_system_prompt(entry: &ChatEntry, expanded: bool, cx: &mut Context<DshApp>) -> gpui::Div {
    let p = dsh(cx);
    let key = entry.key.clone();
    let title = entry.title.clone();
    let body = entry.text.clone();
    let open = expanded && !body.trim().is_empty();
    let toggle_key = key.clone();
    let row = row()
        .id(gpui::ElementId::Name(format!("system-prompt-{key}").into()))
        .role(gpui::Role::Button)
        .aria_label(title.clone())
        .aria_expanded(open)
        .tab_index(0)
        .w_full()
        .gap(px(7.))
        .items_center()
        .rounded(px(8.))
        .h(px(24.))
        .px(px(2.))
        .cursor_pointer()
        .hover(|style| style.bg(p.hover))
        .text_color(p.faint)
        .child(icon(Glyph::Book, 14., p.faint))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .line_height(px(20.))
                .child(title),
        )
        .child(icon(
            if open { Glyph::Up } else { Glyph::Down },
            12.,
            p.faint,
        ))
        .on_click(cx.listener(move |this, _, _, cx| {
            toggle_entry(&mut this.conversation.expanded_entries, &toggle_key);
            cx.notify();
        }));
    let mut card = column().w_full().child(row);
    if open {
        card = card.child(
            div()
                .id(gpui::ElementId::Name(
                    format!("system-prompt-body-{key}").into(),
                ))
                .w_full()
                .max_h(px(141.))
                .overflow_y_scroll()
                .ml(px(21.))
                .mt(px(4.))
                .px(px(10.))
                .py(px(8.))
                .rounded(px(8.))
                .bg(p.stage)
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(p.secondary)
                .child(body),
        );
    }
    card
}

/// 官方 ModelRetryItem：`已重试模型请求（n/m） · Ns`，可展开看失败原因。
fn render_model_retry(entry: &ChatEntry, expanded: bool, cx: &mut Context<DshApp>) -> gpui::Div {
    let p = dsh(cx);
    let key = entry.key.clone();
    let active = entry.status == "active" || entry.status == "scheduled";
    let label = match entry.status.as_str() {
        "active" => "正在重试模型请求",
        "cancelled" => "模型请求重试已取消",
        "scheduled" => "等待重试模型请求",
        _ => "已重试模型请求",
    };
    let attempt = if entry.title.is_empty() {
        label.to_string()
    } else {
        format!(
            "{label}（{}） · {}s",
            entry.title,
            entry.input.parse::<u32>().unwrap_or(1)
        )
    };
    column()
        .w_full()
        .gap(px(2.))
        .child(
            row()
                .id(gpui::ElementId::Name(format!("retry-{key}").into()))
                .role(gpui::Role::Button)
                .aria_label(attempt.clone())
                .aria_expanded(expanded)
                .tab_index(0)
                .w_full()
                .gap(px(7.))
                .items_center()
                .rounded(px(8.))
                .cursor_pointer()
                .text_color(p.faint)
                .hover(|style| style.text_color(p.secondary))
                .child(div().text_size(px(13.)).line_height(px(20.)).child(attempt))
                .when(active, |element| {
                    element.child(pending_dot(
                        gpui::ElementId::Name(format!("retry-dot-{key}").into()),
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .bg(p.state_business_primary),
                    ))
                })
                .child(icon(
                    if expanded { Glyph::Down } else { Glyph::Arrow },
                    12.,
                    p.faint,
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    toggle_entry(&mut this.conversation.expanded_entries, &key);
                    cx.notify();
                })),
        )
        .when(expanded && !entry.text.is_empty(), |element| {
            element.child(
                column()
                    .w_full()
                    .pl(px(14.))
                    .gap(px(2.))
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(p.secondary)
                    .child(format!("失败原因：{}", entry.text)),
            )
        })
}

pub(super) fn toggle_entry(expanded: &mut std::collections::HashSet<String>, key: &str) {
    if !expanded.remove(key) {
        expanded.insert(key.to_string());
    }
}

/// 工具行前导状态符号。
enum ToolLead {
    Spinner,
    Check,
    Alert,
    Stopped,
    None,
}

pub(super) struct TurnActionHint(pub(super) gpui::SharedString);

impl Render for TurnActionHint {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = dsh(cx);
        div()
            .id("turn-action-hint")
            .role(gpui::Role::Tooltip)
            .px(px(7.))
            .py(px(3.))
            .rounded(px(8.))
            .bg(p.tooltip_bg)
            .text_color(p.label_primary)
            .text_size(px(13.))
            .line_height(px(20.))
            .child(self.0.clone())
    }
}

fn turn_action_button(
    id: &'static str,
    label: &'static str,
    artwork: &'static [u8],
    unavailable: Option<&'static str>,
    delta: f32,
    cx: &mut Context<DshApp>,
    action: impl Fn(&mut DshApp, &mut Window, &mut Context<DshApp>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let p = dsh(cx);
    let hint = unavailable.unwrap_or(label);
    let button = div()
        .id(id)
        .group(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .tab_index(0)
        .size(px(28. + delta))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .child(
            svg()
                .data(artwork)
                .size(px(17. + delta))
                .flex_none()
                .text_color(p.label_tertiary)
                .when(unavailable.is_none(), |icon| {
                    icon.group_hover(id, |style| style.text_color(p.label_secondary))
                }),
        )
        .tooltip(move |_, cx| cx.new(|_| TurnActionHint(hint.into())).into());
    if let Some(reason) = unavailable {
        button.opacity(0.4).aria_description(reason)
    } else {
        super::dock_action_button(
            button.cursor_pointer().hover(|style| style.bg(p.hover)),
            cx.listener(move |this, _, w, cx| action(this, w, cx)),
        )
    }
}

/// 记录“已复制”标记，1.6s 后自动还原为复制图标。
pub(super) fn mark_copied(this: &mut DshApp, mark: &str, cx: &mut Context<DshApp>) {
    let mark = mark.to_string();
    this.copied_mark = Some(mark.clone());
    cx.notify();
    cx.spawn(async move |app, cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(1600))
            .await;
        app.update(cx, |app, cx| {
            if app.copied_mark == Some(mark) {
                app.copied_mark = None;
                cx.notify();
            }
        })
        .ok();
    })
    .detach();
}

/// 官方 ContextMeter：14×14 环形进度，无阈值变色。
/// 官方 TurnTriggerNodeView：圆角条 + 图标 + 标题 + 时间 + 展开正文。
fn render_turn_trigger(entry: &ChatEntry, expanded: bool, cx: &mut Context<DshApp>) -> gpui::Div {
    let p = dsh(cx);
    let key = entry.key.clone();
    let title = crate::state::trigger_title(&entry.title, &entry.status);
    let glyph = match entry.title.as_str() {
        "goal" | "trigger:goal" => Glyph::Check,
        "trigger:schedule" => Glyph::Clock,
        "trigger:webhook" | "trigger:github" => Glyph::Globe,
        _ => Glyph::Rocket,
    };
    let time = entry.at_ms.map(bridge::time_label);
    let body = entry.text.clone();
    let body_id = format!("trigger-body-{key}");
    let toggle_key = key.clone();
    column()
        .w_full()
        .rounded(px(16.))
        .border(px(0.5))
        .border_color(p.line)
        .bg(p.selector)
        .child(
            row()
                .id(gpui::ElementId::Name(format!("trigger-{key}").into()))
                .role(gpui::Role::Button)
                .aria_label(title)
                .aria_expanded(expanded)
                .tab_index(0)
                .w_full()
                .px(px(16.))
                .py(px(12.))
                .gap(px(10.))
                .items_center()
                .cursor_pointer()
                .text_color(p.faint)
                .hover(|style| style.text_color(p.secondary).bg(p.hover))
                .child(icon(glyph, 14., p.faint))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(14.))
                        .line_height(px(22.))
                        .child(title),
                )
                .child(div().flex_1())
                .when_some(time, |element, time| {
                    element.child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .child(time),
                    )
                })
                .child(icon(
                    if expanded { Glyph::Up } else { Glyph::Down },
                    12.,
                    p.faint,
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    toggle_entry(&mut this.conversation.expanded_entries, &toggle_key);
                    cx.notify();
                })),
        )
        .when(expanded, |element| {
            element.child(
                column()
                    .px(px(16.))
                    .pb(px(12.))
                    .pl(px(40.))
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(p.secondary)
                            .child("这条通知触发了本轮回复。"),
                    )
                    .when(!body.is_empty(), |element| {
                        element.child(
                            div()
                                .id(gpui::ElementId::Name(body_id.into()))
                                .max_h(px(240.))
                                .overflow_y_scroll()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(p.ink)
                                .child(body),
                        )
                    }),
            )
        })
}

/// 官方助手消息：全宽 Markdown（14/24），标题/列表/表格/引用/代码块 banner（复制+换行）。
pub(super) fn render_markdown(
    source: &str,
    block_id: &str,
    copied_mark: Option<&str>,
    wrapped_code: &std::collections::HashSet<String>,
    font_delta: i32,
    selection: Option<&super::document_selection::SelectionLayout>,
    cx: &mut Context<DshApp>,
) -> gpui::Div {
    let p = dsh(cx);
    let body = (14. + font_delta as f32).clamp(12., 20.);
    let body_lh = body + 10.;
    let mut list = column().w_full().gap(px(8.));
    let leading_spans = selection.map_or(0, |layout| layout.span_count());
    let mut previous_list = false;
    for (index, block) in parse_markdown(source).into_iter().enumerate() {
        let is_list = matches!(block, Block::ListItem(..));
        let first_after_metadata = leading_spans > 0
            && selection.is_some_and(|layout| layout.span_count() == leading_spans)
            && !matches!(block, Block::Paragraph(_));
        let separator = if is_list && previous_list || first_after_metadata {
            "\n"
        } else {
            "\n\n"
        };
        previous_list = is_list;
        let selected = selection.map(|layout| (layout, separator));
        let code_id = format!("{block_id}-{index}");
        list = match block {
            Block::Paragraph(text) => list.child(render_inline_rich(
                &text, p, false, body, body_lh, selected, cx,
            )),
            Block::Heading(level, text) => {
                let (size, line_height, weight, top) = if selection.is_some() {
                    let delta = font_delta as f32;
                    let (size, height, weight) = match level {
                        1 => (21. + delta, 30. + delta, FontWeight::BOLD),
                        2 => (19. + delta, 28. + delta, FontWeight::BOLD),
                        3 => (18. + delta, 26. + delta, FontWeight::BOLD),
                        _ => (body, body_lh, FontWeight::SEMIBOLD),
                    };
                    (size, height, weight, 0.)
                } else {
                    let size = match level {
                        1 | 2 => body + 4.,
                        3 => body + 2.,
                        _ => body + 1.,
                    };
                    (size, size + 8., FontWeight::SEMIBOLD, 4.)
                };
                let family = selection.and_then(|_| super::document_fonts::family(size, cx));
                list.child(
                    div()
                        .w_full()
                        .pt(px(top))
                        .when_some(family, |heading, family| heading.font_family(family))
                        .text_size(px(size))
                        .line_height(px(line_height))
                        .font_weight(weight)
                        .text_color(p.ink)
                        .child(markdown_leaf(
                            if selection.is_some() {
                                strip_md_markers(&text)
                            } else {
                                text
                            },
                            selected,
                        )),
                )
            }
            Block::Quote(text) => list.child(
                row()
                    .w_full()
                    .gap(px(10.))
                    .items_stretch()
                    .child(div().w(px(3.)).rounded_full().bg(p.border_l3).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(body))
                            .line_height(px(body_lh))
                            .text_color(p.secondary)
                            .child(render_inline_rich(
                                &text, p, true, body, body_lh, selected, cx,
                            )),
                    ),
            ),
            Block::Rule => list.child(div().w_full().h(px(0.5)).my(px(4.)).bg(p.line)),
            Block::ListItem(index, text) => list.child(
                row()
                    .w_full()
                    .gap(px(8.))
                    .pl(px(4.))
                    .items_start()
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(body))
                            .line_height(px(body_lh))
                            .text_color(p.secondary)
                            .child(if index == 0 {
                                "•".to_string()
                            } else {
                                format!("{index}.")
                            }),
                    )
                    .child(div().flex_1().min_w_0().child(render_inline_rich(
                        &text, p, false, body, body_lh, selected, cx,
                    ))),
            ),
            Block::Table { headers, rows } => {
                let mut table = column()
                    .w_full()
                    .rounded(px(crate::tokens::Tokens::RADIUS_LG))
                    .border(px(0.5))
                    .border_color(p.line)
                    .overflow_hidden();
                table = table.child(render_table_row(
                    &headers, true, p, body, body_lh, selected, cx,
                ));
                for (row_index, row_cells) in rows.iter().enumerate() {
                    table = table.child(
                        div()
                            .w_full()
                            .border_t(px(0.5))
                            .border_color(p.line)
                            .when(row_index % 2 == 1, |element| element.bg(p.stage))
                            .child(render_table_row(
                                row_cells,
                                false,
                                p,
                                body,
                                body_lh,
                                selection.map(|layout| (layout, "\n")),
                                cx,
                            )),
                    );
                }
                list.child(table)
            }
            Block::Image { alt, url } => list.child(
                column()
                    .w_full()
                    .gap(px(4.))
                    .rounded(px(crate::tokens::Tokens::RADIUS_LG))
                    .border(px(0.5))
                    .border_color(p.line)
                    .bg(p.md_code_block)
                    .px(px(12.))
                    .py(px(10.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .text_color(p.ink)
                            .child(markdown_leaf(
                                if alt.is_empty() {
                                    "图片".to_owned()
                                } else {
                                    alt.clone()
                                },
                                selected,
                            )),
                    )
                    .child({
                        let open = url.clone();
                        div()
                            .id(gpui::ElementId::Name(format!("img-link-{code_id}").into()))
                            .role(gpui::Role::Link)
                            .cursor_pointer()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(p.link)
                            .child(markdown_leaf(url, selection.map(|layout| (layout, "\n"))))
                            .on_click(cx.listener({
                                let selection = selection.cloned();
                                move |_, event, _, _| {
                                    if selection
                                        .as_ref()
                                        .is_none_or(|layout| layout.clicked_link(event))
                                    {
                                        bridge::open_url(&open);
                                    }
                                }
                            }))
                    }),
            ),
            Block::Code { lang, text } => {
                // 官方：diff / patch 围栏以统一 diff 行代替普通代码内容。
                let is_diff = matches!(lang.to_lowercase().as_str(), "diff" | "patch");
                let color_of = |kind: crate::markdown::CodeToken| match kind {
                    crate::markdown::CodeToken::Keyword => p.label_primary_bluish,
                    crate::markdown::CodeToken::Str => p.success,
                    crate::markdown::CodeToken::Number => p.state_warn_label,
                    crate::markdown::CodeToken::Comment => p.faint,
                };
                let banner = if lang.is_empty() {
                    "代码".to_string()
                } else {
                    lang.clone()
                };
                let source = text.clone();
                let highlight_lang = lang.clone();
                let copy_key = format!("code-{code_id}");
                let wrap_key = code_id.clone();
                let copied = copied_mark == Some(copy_key.as_str());
                let wrapped = wrapped_code.contains(&wrap_key);
                let for_copy = text.clone();
                list.child(
                    column()
                        .w_full()
                        .rounded(px(crate::tokens::Tokens::RADIUS_LG))
                        .border(px(0.5))
                        .border_color(p.line)
                        .bg(p.md_code_block)
                        .overflow_hidden()
                        .child(
                            row()
                                .w_full()
                                .h(px(33.))
                                .flex_none()
                                .px(px(12.))
                                .items_center()
                                .gap(px(8.))
                                .bg(p.md_code_block_banner)
                                .border_b(px(0.5))
                                .border_color(p.line)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(px(11.))
                                        .line_height(px(18.))
                                        .text_color(p.faint)
                                        .child(banner),
                                )
                                .child({
                                    let wrap_key = wrap_key.clone();
                                    div()
                                        .id(gpui::ElementId::Name(
                                            format!("wrap-{wrap_key}").into(),
                                        ))
                                        .role(gpui::Role::Button)
                                        .aria_label(if wrapped { "取消换行" } else { "换行" })
                                        .size(px(24.))
                                        .rounded(px(8.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(|style| style.bg(p.hover))
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(if wrapped {
                                                    p.state_business_primary
                                                } else {
                                                    p.faint
                                                })
                                                .child(if wrapped { "↩" } else { "↪" }),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.conversation.wrapped_code.contains(&wrap_key) {
                                                this.conversation.wrapped_code.remove(&wrap_key);
                                            } else {
                                                this.conversation
                                                    .wrapped_code
                                                    .insert(wrap_key.clone());
                                            }
                                            cx.notify();
                                        }))
                                })
                                .child(
                                    div()
                                        .id(gpui::ElementId::Name(
                                            format!("copy-{copy_key}").into(),
                                        ))
                                        .role(gpui::Role::Button)
                                        .aria_label(if copied { "已复制" } else { "复制" })
                                        .size(px(24.))
                                        .rounded(px(8.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(|style| style.bg(p.hover))
                                        .child(icon(
                                            if copied { Glyph::Check } else { Glyph::Copy },
                                            14.,
                                            if copied {
                                                p.state_business_primary
                                            } else {
                                                p.faint
                                            },
                                        ))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            bridge::copy_text(&for_copy);
                                            mark_copied(this, &copy_key, cx);
                                            this.push_toast("已复制", cx);
                                        })),
                                ),
                        )
                        .child({
                            let mut body = div()
                                .w_full()
                                .px(px(16.))
                                .py(px(12.))
                                .font_family("monospace")
                                .text_size(px(12.))
                                .line_height(px(20.))
                                .text_color(p.secondary)
                                .overflow_x_hidden();
                            if is_diff {
                                let mut rows = column().w_full();
                                for line in text.lines() {
                                    let (color, fill) =
                                        if line.starts_with("+++") || line.starts_with('+') {
                                            (p.success, Some(p.state_success_tertiary))
                                        } else if line.starts_with("---") || line.starts_with('-') {
                                            (p.danger, Some(p.hover_danger))
                                        } else if line.starts_with("@@") {
                                            (p.state_business_primary, None)
                                        } else {
                                            (p.secondary, None)
                                        };
                                    let row = div()
                                        .w_full()
                                        .px(px(6.))
                                        .text_color(color)
                                        .when_some(fill, |row, fill| row.bg(fill))
                                        .child(line.to_owned());
                                    rows = rows.child(row);
                                }
                                body = body.child(rows);
                            } else {
                                body = body
                                    .when(wrapped, |element| element.overflow_x_hidden())
                                    .child(super::document_selection::SelectionLayout::text(
                                        selection,
                                        source,
                                        separator,
                                        |source| {
                                            gpui::StyledText::new(source).with_highlights(
                                                code_spans(&text, &highlight_lang).into_iter().map(
                                                    |span| {
                                                        (
                                                            span.start..span.end,
                                                            gpui::HighlightStyle {
                                                                color: Some(color_of(span.kind)),
                                                                ..Default::default()
                                                            },
                                                        )
                                                    },
                                                ),
                                            )
                                        },
                                    ));
                            }
                            body
                        }),
                )
            }
        };
    }
    list
}

type MarkdownSelection<'a> = Option<(&'a super::document_selection::SelectionLayout, &'static str)>;

fn markdown_leaf(text: String, selection: MarkdownSelection<'_>) -> gpui::AnyElement {
    let (layout, prefix) = selection.map_or((None, ""), |(layout, prefix)| (Some(layout), prefix));
    super::document_selection::SelectionLayout::text(layout, text, prefix, gpui::StyledText::new)
}

fn render_table_row(
    cells: &[String],
    header: bool,
    p: crate::tokens::Tokens,
    body: f32,
    body_lh: f32,
    selected: MarkdownSelection<'_>,
    cx: &mut Context<DshApp>,
) -> gpui::Div {
    let mut line = row().w_full().items_stretch();
    for (index, cell) in cells.iter().enumerate() {
        line = line.child(
            div()
                .flex_1()
                .min_w_0()
                .px(px(12.))
                .py(px(8.))
                .when(index + 1 < cells.len(), |element| {
                    element.border_r(px(0.5)).border_color(p.line)
                })
                .when(header, |element| {
                    element
                        .bg(p.md_code_block_banner)
                        .font_weight(FontWeight::MEDIUM)
                })
                .child(render_inline_rich(
                    cell,
                    p,
                    false,
                    body,
                    body_lh,
                    selected.map(|(layout, separator)| {
                        (layout, if index == 0 { separator } else { "\t" })
                    }),
                    cx,
                )),
        );
    }
    line
}

fn render_inline_rich(
    text: &str,
    p: crate::tokens::Tokens,
    muted: bool,
    body: f32,
    body_lh: f32,
    selection: MarkdownSelection<'_>,
    cx: &mut Context<DshApp>,
) -> gpui::Div {
    let parts = inline_parts(text);
    if parts
        .iter()
        .any(|part| matches!(part, InlinePart::Link { .. }))
    {
        let mut line = row().w_full().flex_wrap().gap(px(0.));
        for (index, part) in parts.into_iter().enumerate() {
            let selected =
                selection.map(|(layout, prefix)| (layout, if index == 0 { prefix } else { "" }));
            line = match part {
                InlinePart::Text { text, bold, code } => line.child(
                    div()
                        .text_size(px(body))
                        .line_height(px(body_lh))
                        .text_color(if muted || code { p.secondary } else { p.ink })
                        .when(bold, |element| element.font_weight(FontWeight::SEMIBOLD))
                        .when(code, |element| {
                            element
                                .px(px(4.))
                                .rounded(px(4.))
                                .bg(p.md_inline_code)
                                .font_family("monospace")
                                .text_size(px((body - 2.).max(11.)))
                        })
                        .child(markdown_leaf(text, selected)),
                ),
                InlinePart::Link { label, url } => {
                    let open = url.clone();
                    line.child(
                        div()
                            .id(gpui::ElementId::Name(
                                format!("md-link-{index}-{}", url.len()).into(),
                            ))
                            .role(gpui::Role::Link)
                            .cursor_pointer()
                            .text_size(px(body))
                            .line_height(px(body_lh))
                            .when(selection.is_some(), |link| {
                                link.font_weight(FontWeight::MEDIUM)
                            })
                            .text_color(p.link)
                            .underline()
                            .child(markdown_leaf(label, selected))
                            .on_click(cx.listener({
                                let selection = selected.map(|(layout, _)| layout.clone());
                                move |_, event, _, _| {
                                    if selection
                                        .as_ref()
                                        .is_none_or(|layout| layout.clicked_link(event))
                                    {
                                        bridge::open_url(&open);
                                    }
                                }
                            })),
                    )
                }
            };
        }
        return line;
    }

    let display = strip_md_markers(text);
    let spans = remap_inline_spans(text, &display);
    let (layout, prefix) = selection.map_or((None, ""), |(layout, prefix)| (Some(layout), prefix));
    div()
        .w_full()
        .text_size(px(body))
        .line_height(px(body_lh))
        .text_color(if muted { p.secondary } else { p.ink })
        .child(super::document_selection::SelectionLayout::text(
            layout,
            display,
            prefix,
            |display| {
                gpui::StyledText::new(display).with_highlights(spans.into_iter().map(|span| {
                    (
                        span.start..span.end,
                        gpui::HighlightStyle {
                            color: if span.code { Some(p.secondary) } else { None },
                            background_color: span.code.then_some(p.md_inline_code),
                            font_weight: (!span.code).then_some(gpui::FontWeight::SEMIBOLD),
                            ..Default::default()
                        },
                    )
                }))
            },
        ))
}

pub(super) fn strip_md_markers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while cursor < text.len() {
        if text[cursor..].starts_with('[')
            && let Some((label_end, url_end)) = {
                let slice = &text[cursor..];
                slice.find("](").and_then(|label_close| {
                    let url_close = slice[label_close + 2..].find(')')?;
                    Some((label_close, label_close + 2 + url_close + 1))
                })
            }
        {
            out.push_str(&text[cursor + 1..cursor + label_end]);
            cursor += url_end;
            continue;
        }
        if text[cursor..].starts_with("**")
            && let Some(offset) = text[cursor + 2..].find("**")
        {
            out.push_str(&text[cursor + 2..cursor + 2 + offset]);
            cursor += offset + 4;
            continue;
        }
        if text[cursor..].starts_with('`')
            && let Some(offset) = text[cursor + 1..].find('`')
        {
            out.push_str(&text[cursor + 1..cursor + 1 + offset]);
            cursor += offset + 2;
            continue;
        }
        let ch = text[cursor..].chars().next().unwrap();
        out.push(ch);
        cursor += ch.len_utf8();
    }
    out
}

fn remap_inline_spans(source: &str, display: &str) -> Vec<crate::markdown::InlineSpan> {
    // 在去掉标记后的展示串上重建粗体/代码区间。
    let _ = (source, display);
    let mut spans = Vec::new();
    let mut source_cursor = 0usize;
    let mut display_cursor = 0usize;
    while source_cursor < source.len() {
        if source[source_cursor..].starts_with('[')
            && let Some((label_end, url_end)) = {
                let slice = &source[source_cursor..];
                slice.find("](").and_then(|label_close| {
                    let url_close = slice[label_close + 2..].find(')')?;
                    Some((label_close, label_close + 2 + url_close + 1))
                })
            }
        {
            let label_len = label_end - 1;
            display_cursor += label_len;
            source_cursor += url_end;
            continue;
        }
        if source[source_cursor..].starts_with("**")
            && let Some(offset) = source[source_cursor + 2..].find("**")
        {
            let start = display_cursor;
            let end = start + offset;
            spans.push(crate::markdown::InlineSpan {
                start,
                end,
                code: false,
            });
            display_cursor = end;
            source_cursor += offset + 4;
            continue;
        }
        if source[source_cursor..].starts_with('`')
            && let Some(offset) = source[source_cursor + 1..].find('`')
        {
            let start = display_cursor;
            let end = start + offset;
            spans.push(crate::markdown::InlineSpan {
                start,
                end,
                code: true,
            });
            display_cursor = end;
            source_cursor += offset + 2;
            continue;
        }
        let len = source[source_cursor..]
            .chars()
            .next()
            .map_or(1, char::len_utf8);
        source_cursor += len;
        display_cursor += len;
    }
    spans
}

fn code_text(text: &str, p: crate::tokens::Tokens) -> gpui::Div {
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

/// 官方侧栏折叠后的图标栏宽度（--dsh 布局：collapsed rail 56px）。
pub(super) const SIDEBAR_RAIL_WIDTH: f32 = 56.;

pub(super) fn collapsed_sidebar_width() -> f32 {
    if bridge::is_mac() {
        0.
    } else {
        SIDEBAR_RAIL_WIDTH
    }
}

fn permission_icon(id: &str, size: f32, color: gpui::Hsla) -> Option<gpui::Svg> {
    let data: &'static [u8] = match id {
        "read-only" => include_bytes!("../../assets/permission-read-only.svg"),
        "workspace-write" => include_bytes!("../../assets/permission-workspace-write.svg"),
        "danger-full-access" => include_bytes!("../../assets/permission-full-access.svg"),
        "auto" => include_bytes!("../../assets/permission-auto-review.svg"),
        _ => return None,
    };
    Some(
        svg()
            .data(data)
            .size(px(size))
            .flex_none()
            .text_color(color),
    )
}

fn settings_mark(size: f32, cx: &App) -> gpui::Div {
    let p = dsh(cx);
    row()
        .size(px(size))
        .flex_none()
        .justify_center()
        .rounded_full()
        .bg(p.hover)
        .child(icon(Glyph::Settings, size * 0.62, p.secondary))
}

fn animated_brand_mark(id: &'static str, size: f32, dark: bool) -> impl IntoElement {
    div()
        .relative()
        .flex_none()
        .child(crate::brand::mark(size, dark))
        .with_animation(
            id,
            gpui::Animation::new(FISH_SWIM)
                .repeat()
                .with_easing(gpui::ease_in_out),
            |el, t| {
                let (_, dx, dy) = anim::fish_swim_phase(t);
                el.left(px(dx)).top(px(dy))
            },
        )
}

pub(super) fn begin_window_drag() {
    invoke("start_window_dragging", json!({}));
}

pub(super) fn dark(cx: &App) -> bool {
    Theme::global(cx).mode == ThemeMode::Dark
}

pub(super) fn conversation_bg(cx: &App) -> gpui::Hsla {
    dsh(cx).bg_base
}

pub(super) fn composer_bg(cx: &App) -> gpui::Hsla {
    dsh(cx).input_major
}

/// 官方 5px 胶囊滚动条叠加层（--dsh-scrollbar-width:5px、无轨道、hover 加深）。
/// 放在滚动容器的 relative 父层：absolute 贴右缘。
fn scrollbar_overlay(id: &'static str, handle: &ScrollHandle, cx: &App) -> impl IntoElement {
    let colors = dsh(cx);
    let handle = handle.clone();
    div()
        .id(id)
        .absolute()
        .top_0()
        .right_0()
        .h_full()
        .w(px(5.))
        .on_hover(|_, w, _| w.refresh())
        .child(
            canvas(
                move |bounds, w, _| {
                    let height = handle.bounds().size.height.as_f32();
                    let max = handle.max_offset().y.as_f32();
                    if max <= 0. || height <= 0. {
                        return None;
                    }
                    let thumb_height = (height * height / (height + max))
                        .clamp(0., height)
                        .max(24.0_f32.min(height));
                    let ratio = (-handle.offset().y.as_f32() / max).clamp(0., 1.);
                    let y = bounds.top() + px(ratio * (height - thumb_height));
                    let thumb = gpui::Bounds::new(
                        point(bounds.left(), y),
                        gpui::size(px(5.), px(thumb_height)),
                    );
                    let color = if bounds.contains(&w.mouse_position()) {
                        colors.scrollbar_l1_hover
                    } else {
                        colors.scrollbar_l1
                    };
                    Some(gpui::fill(thumb, color).corner_radii(px(2.5)))
                },
                |_, thumb, w, _| {
                    if let Some(thumb) = thumb {
                        w.paint_quad(thumb);
                    }
                },
            )
            .size_full(),
        )
}

pub(super) fn menu_bg(cx: &App) -> gpui::Hsla {
    dsh(cx).menu_surface
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
