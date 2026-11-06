//! Right-hand start workspace with desktop tool entry points.

use super::workspace::{begin_window_drag, conversation_bg, dark};
use super::*;
use gpui::svg;

impl DshApp {
    pub(super) fn render_start_panel(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = Paint::get(cx);
        let expanded = self.start_panel_expanded;
        let width = start_panel_width(w, self.sidebar_collapsed, self.sidebar_width);
        let mut panel = column()
            .relative()
            .h_full()
            .min_w_0()
            .gap_0()
            .border_l_1()
            .border_color(if dark(cx) {
                gpui::rgb(0x232324).into()
            } else {
                p.line
            })
            .bg(conversation_bg(cx));
        panel = if expanded {
            panel.flex_1().w_full()
        } else {
            panel.w(px(width)).flex_none()
        };

        let mut title = row()
            .min_w_0()
            .gap(px(8.))
            .child(
                div()
                    .size(px(16.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_color(p.faint)
                    .child(icon(Glyph::Check, 10., p.secondary)),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("开始"),
            );
        if self.sidebar_collapsed {
            title = title.child(
                div()
                    .id("start-expand-sidebar")
                    .role(gpui::Role::Button)
                    .aria_label("展开侧栏")
                    .tab_index(0)
                    .size(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .ml(px(4.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|style| style.bg(p.hover))
                    .child(
                        svg()
                            .data(include_bytes!("../../assets/sidebar-panel.svg"))
                            .size(px(16.))
                            .text_color(p.secondary),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sidebar_collapsed = false;
                        cx.notify();
                    })),
            );
        }

        let cards = column()
            .w_full()
            .max_w(px(440.))
            .gap(px(16.))
            .child(
                self.start_card(
                    "files",
                    "工作区文件",
                    Some("浏览会话工作区的文件"),
                    svg()
                        .data(include_bytes!("../../assets/start-files.svg"))
                        .size(px(26.))
                        .flex_none(),
                    Some("⌘ P"),
                    cx,
                ),
            )
            .child(
                self.start_card(
                    "terminal",
                    "新建终端",
                    Some("在会话工作区运行命令"),
                    svg()
                        .data(include_bytes!("../../assets/start-terminal.svg"))
                        .size(px(26.))
                        .flex_none(),
                    Some("⌘ `"),
                    cx,
                ),
            )
            .child(self.start_card(
                "browser",
                "浏览器",
                Some("浏览网页"),
                icon(Glyph::Globe, 26., gpui::rgb(0x65a7ff).into()),
                Some("⌘ T"),
                cx,
            ))
            .child(
                self.start_card(
                    "tools",
                    "更多工具",
                    None,
                    svg()
                        .data(include_bytes!("../../assets/start-tools.svg"))
                        .size(px(24.))
                        .flex_none()
                        .text_color(p.secondary),
                    None,
                    cx,
                ),
            );

        panel
            .child(
                row()
                    .w_full()
                    .h(px(50.))
                    .flex_none()
                    .pl(if self.sidebar_collapsed && !w.is_fullscreen() {
                        px(76.)
                    } else {
                        px(20.)
                    })
                    .pr(px(12.))
                    .child(title)
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, _| begin_window_drag()),
                    )
                    .child(self.start_layout_button("start-split", "分栏显示", false, cx))
                    .child(
                        div()
                            .id("start-expand")
                            .role(gpui::Role::Button)
                            .aria_label(if expanded {
                                "恢复分栏"
                            } else {
                                "展开工作区"
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
                            .child(icon(Glyph::Fit, 15., p.secondary))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_panel_expanded = !this.start_panel_expanded;
                                cx.notify();
                            })),
                    )
                    .child(self.render_start_panel_toggle(cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(40.))
                    .pb(px(56.))
                    .child(
                        column()
                            .w_full()
                            .max_w(px(440.))
                            .gap(px(48.))
                            .child(
                                row().w_full().justify_center().child(
                                    svg()
                                        .data(include_bytes!("../../assets/start-compass.svg"))
                                        .size(px(52.))
                                        .text_color(p.faint)
                                        .opacity(0.5),
                                ),
                            )
                            .child(cards)
                            .when_some(self.error.clone(), |element, error| {
                                element.child(
                                    div()
                                        .w_full()
                                        .text_size(px(12.))
                                        .line_height(px(18.))
                                        .text_color(p.danger)
                                        .child(error),
                                )
                            }),
                    ),
            )
    }

    pub(super) fn render_start_panel_toggle(
        &self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        div()
            .id("toggle-start-panel")
            .role(gpui::Role::Button)
            .aria_label(if self.show_start_panel {
                "收起右侧工作区"
            } else {
                "打开右侧工作区"
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
                    .data(include_bytes!("../../assets/sidebar-right.svg"))
                    .size(px(16.))
                    .text_color(p.secondary),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_start_panel = !this.show_start_panel;
                if !this.show_start_panel {
                    this.start_panel_expanded = false;
                }
                cx.notify();
            }))
    }

    fn start_layout_button(
        &self,
        id: &'static str,
        label: &'static str,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        div()
            .id(id)
            .role(gpui::Role::Button)
            .aria_label(label)
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
                this.start_panel_expanded = expanded;
                this.show_start_panel = true;
                cx.notify();
            }))
    }

    fn start_card(
        &self,
        id: &'static str,
        title: &'static str,
        description: Option<&'static str>,
        artwork: impl IntoElement,
        shortcut: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        row()
            .id(gpui::ElementId::Name(format!("start-card-{id}").into()))
            .role(gpui::Role::Button)
            .aria_label(description.map_or(title, |description| {
                if description.is_empty() {
                    title
                } else {
                    description
                }
            }))
            .tab_index(0)
            .w_full()
            .h(px(if description.is_some() { 78. } else { 64. }))
            .flex_none()
            .px(px(20.))
            .gap(px(14.))
            .rounded(px(12.))
            .border(px(0.5))
            .border_color(if dark(cx) {
                gpui::rgb(0x2c2c2e).into()
            } else {
                p.line
            })
            .bg(if dark(cx) {
                gpui::rgb(0x202022).into()
            } else {
                p.stage
            })
            .cursor_pointer()
            .hover(|style| style.bg(p.hover))
            .child(artwork)
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        row()
                            .w_full()
                            .gap(px(5.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .line_height(px(20.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(title),
                            )
                            .when(id == "terminal", |element| {
                                element.child(icon(Glyph::Down, 11., p.faint))
                            }),
                    )
                    .children(description.map(|description| {
                        div()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(p.faint)
                            .child(description)
                    })),
            )
            .children(shortcut.map(|shortcut| {
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(p.faint)
                    .child(shortcut)
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.error = None;
                if id == "tools" {
                    this.open_page(WorkspacePage::More, cx);
                } else {
                    invoke(
                        "open_start_tool",
                        json!({"workspace": this.workspace, "tool": id}),
                    );
                }
                cx.notify();
            }))
    }
}

pub(super) fn start_panel_width(w: &Window, collapsed: bool, sidebar_width: f32) -> f32 {
    let main_width =
        f32::from(w.viewport_size().width) - if collapsed { 0. } else { sidebar_width };
    (main_width * 0.56)
        .clamp(380., 650.)
        .min((main_width - 360.).max(300.))
}
