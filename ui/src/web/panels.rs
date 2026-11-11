//! Plugin, automation, and utility pages exposed by the workspace navigation.

use super::*;

impl DshApp {
    pub(super) fn render_more_panel(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let connection = if self.workspace.is_empty() {
            "选择工作区并配置 dsh-acp 服务路径".to_string()
        } else {
            self.workspace.clone()
        };
        page_scroll("more-page", cx)
            .child(
                row()
                    .w_full()
                    .min_h(px(32.))
                    .mb(px(24.))
                    .child(page_title("更多")),
            )
            .child(
                column()
                    .w_full()
                    .gap(px(2.))
                    .child(self.settings_link(
                        "more-connection",
                        "工作区与连接",
                        &connection,
                        Glyph::Folder,
                        SettingsPage::General,
                        cx,
                    ))
                    .child(self.settings_link(
                        "more-api-key",
                        "API Key",
                        "配置本机 DeepSeek API Key",
                        Glyph::Lock,
                        SettingsPage::Credentials,
                        cx,
                    ))
                    .child(self.settings_link(
                        "more-model",
                        "模型与推理",
                        "选择当前会话使用的模型和思考强度",
                        Glyph::Bulb,
                        SettingsPage::Model,
                        cx,
                    ))
                    .child(self.settings_link(
                        "more-appearance",
                        "外观",
                        if super::workspace::dark(cx) {
                            "当前为深色主题"
                        } else {
                            "当前为浅色主题"
                        },
                        Glyph::Sun,
                        SettingsPage::General,
                        cx,
                    )),
            )
            .child(
                row()
                    .w_full()
                    .mt(px(20.))
                    .px(px(8.))
                    .gap(px(7.))
                    .child(icon(
                        if self.connected() {
                            Glyph::Check
                        } else {
                            Glyph::Alert
                        },
                        14.,
                        if self.connected() { p.success } else { p.faint },
                    ))
                    .child(note(
                        if self.connected() {
                            format!("ACP 已连接 · {}", self.agent_name)
                        } else {
                            "ACP 尚未连接，可在“工作区与连接”中检查配置".to_string()
                        },
                        cx,
                    )),
            )
    }

    fn settings_link(
        &self,
        id: &'static str,
        title: &'static str,
        description: &str,
        glyph: Glyph,
        target: SettingsPage,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        row()
            .id(id)
            .role(gpui::Role::Button)
            .aria_label(title)
            .tab_index(0)
            .w_full()
            .min_h(px(64.))
            .px(px(12.))
            .py(px(10.))
            .gap(px(12.))
            .rounded(px(12.))
            .cursor_pointer()
            .hover(|style| style.bg(p.hover))
            .child(icon(glyph, 20., p.secondary))
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .line_height(px(22.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(p.secondary)
                            .child(description.to_string()),
                    ),
            )
            .child(icon(Glyph::Arrow, 15., p.faint))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.settings_page = target;
                this.show_more_menu = false;
                this.show_settings = true;
                cx.notify();
            }))
    }
}

fn page_scroll(id: &'static str, cx: &App) -> gpui::Stateful<gpui::Div> {
    column()
        .id(id)
        .flex_1()
        .min_h_0()
        .w_full()
        .max_w(px(1056.))
        .mx_auto()
        .overflow_y_scroll()
        .px(px(48.))
        .pt(px(76.))
        .pb(px(48.))
        .gap_0()
        .text_color(dsh(cx).ink)
}

fn page_title(title: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .text_size(px(20.))
        .line_height(px(28.))
        .font_weight(FontWeight::MEDIUM)
        .child(title)
}
