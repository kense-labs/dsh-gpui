//! Expanding workspace search with coordinated width and opacity motion.

use super::*;
use gpui::svg;

impl DshApp {
    pub(super) fn render_workspace_search(
        &mut self,
        content_width: f32,
        filtered_count: usize,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = Paint::get(cx);
        self.search_slide
            .duration(if self.search_expanded { 1. } else { 0. }, 0.18, cx);
        let expanded = self.search_slide.get(w, cx).clamp(0., 1.);
        let collapsed = 1. - expanded;
        let fade_in = (expanded * 1.5).min(1.);
        let fade_out = (collapsed * 1.5).min(1.);
        let query = self.search_editor.read(cx).value().trim().to_string();
        let label = if !query.is_empty() {
            format!("{filtered_count} 个匹配会话")
        } else if self.workspace_group_by == WorkspaceGroupBy::Flat {
            "会话".to_string()
        } else {
            "工作区".to_string()
        };
        let label_width = if query.is_empty() {
            48.
        } else {
            content_width * 0.45
        };
        let spacer_width = (content_width - label_width - 112.).max(0.) * collapsed;
        let search_width = 28. + (content_width - 36.).max(0.) * expanded;

        row()
            .w(px(content_width))
            .h(px(36.))
            .flex_none()
            .mt(px(10.))
            .pl(px(4. + 8. * collapsed))
            .pr(px(4.))
            .gap(px(4. * collapsed))
            .overflow_hidden()
            .child(
                div()
                    .w(px(label_width * collapsed))
                    .flex_none()
                    .overflow_hidden()
                    .opacity(fade_out)
                    .child(
                        div()
                            .w(px(label_width))
                            .truncate()
                            .text_size(px(12.))
                            .line_height(px(20.))
                            .text_color(p.faint)
                            .child(label),
                    ),
            )
            .child(div().w(px(spacer_width)).flex_none())
            .child(
                row()
                    .id("session-search")
                    .w(px(search_width))
                    .h(px(28. + 4. * expanded))
                    .flex_none()
                    .overflow_hidden()
                    .rounded(px(6. + 6. * expanded))
                    .border(px(0.5))
                    .border_color(p.line.opacity(expanded))
                    .child(
                        div()
                            .id("expand-session-search")
                            .role(gpui::Role::Button)
                            .aria_label("搜索会话")
                            .aria_expanded(self.search_expanded)
                            .tab_index(0)
                            .w(px(28.))
                            .h(px(28. + 4. * expanded))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .when(!self.search_expanded, |element| {
                                element.hover(|style| style.bg(p.hover))
                            })
                            .child(
                                svg()
                                    .data(include_bytes!("../../assets/workspace-search.svg"))
                                    .size(px(14. - 3. * expanded))
                                    .text_color(p.secondary),
                            )
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.search_expanded = true;
                                this.show_workspace_view_options = false;
                                this.search_editor.update(cx, |editor, cx| {
                                    editor.focus(w, cx);
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(32.))
                            .ml(px(-2. * expanded))
                            .overflow_hidden()
                            .opacity(fade_in)
                            .child(self.search_editor.clone()),
                    )
                    .child(
                        div()
                            .id("close-session-search")
                            .role(gpui::Role::Button)
                            .aria_label("关闭搜索")
                            .tab_index(0)
                            .tab_stop(self.search_expanded)
                            .w(px(24. * expanded))
                            .h(px(24.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .rounded(px(6.))
                            .opacity(fade_in)
                            .cursor_pointer()
                            .hover(|style| style.bg(p.hover))
                            .child(icon(Glyph::Close, 12., p.secondary))
                            .on_click(cx.listener(|this, _, w, cx| {
                                replace(&this.search_editor, "", w, cx);
                                this.search_expanded = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                row()
                    .w(px(60. * collapsed))
                    .h(px(28.))
                    .flex_none()
                    .gap(px(4.))
                    .overflow_hidden()
                    .opacity(fade_out)
                    .child(
                        div()
                            .id("workspace-view-options")
                            .role(gpui::Role::Button)
                            .aria_label("视图选项")
                            .aria_expanded(self.show_workspace_view_options)
                            .tab_index(0)
                            .tab_stop(!self.search_expanded)
                            .size(px(28.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.))
                            .cursor_pointer()
                            .when(self.show_workspace_view_options, |element| {
                                element.bg(p.hover)
                            })
                            .hover(|style| style.bg(p.hover))
                            .child(
                                svg()
                                    .data(include_bytes!("../../assets/view-options.svg"))
                                    .size(px(16.))
                                    .text_color(p.secondary),
                            )
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if this.search_expanded {
                                        return;
                                    }
                                    this.show_workspace_view_options =
                                        !this.show_workspace_view_options;
                                    this.show_workspace_menu = false;
                                    this.show_account_menu = false;
                                    this.composer_menu = None;
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("add-workspace")
                            .role(gpui::Role::Button)
                            .aria_label("添加工作区")
                            .tab_index(0)
                            .tab_stop(!self.search_expanded)
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
                                    .data(include_bytes!("../../assets/workspace-add.svg"))
                                    .size(px(16.))
                                    .text_color(p.secondary),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.search_expanded {
                                    return;
                                }
                                this.show_workspace_view_options = false;
                                this.show_workspace_menu = false;
                                this.composer_menu = None;
                                this.settings_page = SettingsPage::General;
                                this.show_settings = true;
                                cx.notify();
                            })),
                    ),
            )
    }
}
