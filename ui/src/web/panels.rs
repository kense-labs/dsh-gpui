//! Plugin, automation, and utility pages exposed by the workspace navigation.

use super::*;

const INSPECT_PLUGINS_PROMPT: &str = "请检查当前 DeepSeek Harness 环境中的插件配置和已安装插件，列出名称、启用状态与可用的配置入口。只读取并报告实际结果，不要修改配置。";
const CREATE_PLUGIN_PROMPT: &str = "我想为当前 DeepSeek Harness 环境添加或开发一个插件。请先了解我的需求，检查当前环境支持的插件安装方式，再给出可执行的方案；修改文件或安装软件包之前请先征得我同意。";
const CREATE_AUTOMATION_PROMPT: &str = "我想创建一个 DeepSeek Harness 自动化任务。请询问我任务要执行的内容、运行频率和时间，并在确认后使用当前环境实际支持的调度能力创建。不要声称任务已创建，除非已收到成功的执行结果。";

impl DshApp {
    pub(super) fn render_plugins_panel(
        &mut self,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        page_scroll("plugins-page", cx)
            .child(
                row()
                    .w_full()
                    .min_h(px(32.))
                    .mb(px(20.))
                    .gap(px(16.))
                    .child(page_title("插件"))
                    .child(div().flex_1())
                    .child(self.assisted_button(
                        "inspect-plugins",
                        "检查当前插件",
                        Glyph::Search,
                        INSPECT_PLUGINS_PROMPT,
                        false,
                        cx,
                    ))
                    .child(self.assisted_button(
                        "add-plugin",
                        "添加插件",
                        Glyph::Plus,
                        CREATE_PLUGIN_PROMPT,
                        true,
                        cx,
                    )),
            )
            .child(
                div()
                    .w_full()
                    .pb(px(24.))
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .text_color(p.secondary)
                    .child("安装、启用和配置 DeepSeek Harness 插件"),
            )
            .child(capability_notice(
                "当前 ACP 连接未提供插件清单接口",
                "这里不会显示未经确认的插件、版本或开关。可以通过真实会话检查当前环境，或让 Agent 协助添加插件；所有修改仍会经过工具审批。",
                Glyph::Bulb,
                cx,
            ))
            .child(
                column()
                    .w_full()
                    .pt(px(24.))
                    .gap(px(8.))
                    .child(section_title("通过会话管理"))
                    .child(
                        div()
                            .w_full()
                            .text_size(px(13.))
                            .line_height(px(21.))
                            .text_color(p.secondary)
                            .child("检查插件会新建会话并预填只读检查请求；添加插件会预填需求说明。草稿不会自动发送，你可以先修改再执行。"),
                    ),
            )
    }

    pub(super) fn render_automations_panel(
        &mut self,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let mut filters = row().w_full().gap(px(8.)).mb(px(14.));
        for (index, label) in ["已开启", "已暂停", "已结束", "全部"]
            .into_iter()
            .enumerate()
        {
            filters = filters.child(
                div()
                    .id(gpui::ElementId::Name(
                        format!("automation-filter-{index}").into(),
                    ))
                    .role(gpui::Role::Button)
                    .aria_label(format!("{label}（任务列表不可用）"))
                    .h(px(28.))
                    .px(px(10.))
                    .rounded(px(14.))
                    .cursor_default()
                    .opacity(if index == 0 { 1. } else { 0.48 })
                    .when(index == 0, |element| element.bg(p.hover))
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .text_color(if index == 0 { p.ink } else { p.faint })
                    .child(label),
            );
        }
        page_scroll("automations-page", cx)
            .child(
                row()
                    .w_full()
                    .min_h(px(32.))
                    .mb(px(24.))
                    .gap(px(16.))
                    .child(page_title("自动化任务"))
                    .child(div().flex_1())
                    .child(self.assisted_button(
                        "new-automation",
                        "新建",
                        Glyph::Plus,
                        CREATE_AUTOMATION_PROMPT,
                        true,
                        cx,
                    )),
            )
            .child(filters)
            .child(
                row()
                    .id("automation-search-unavailable")
                    .w_full()
                    .h(px(36.))
                    .mb(px(16.))
                    .px(px(10.))
                    .gap(px(8.))
                    .rounded(px(12.))
                    .border(px(0.5))
                    .border_color(p.line)
                    .opacity(0.5)
                    .cursor_default()
                    .child(icon(Glyph::Search, 14., p.faint))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(p.faint)
                            .child("搜索自动化任务（任务列表不可用）"),
                    ),
            )
            .child(
                column()
                    .w_full()
                    .items_center()
                    .px(px(20.))
                    .py(px(48.))
                    .gap(px(12.))
                    .child(icon(Glyph::Clock, 30., p.faint))
                    .child(
                        div()
                            .text_size(px(14.))
                            .line_height(px(22.))
                            .text_color(p.secondary)
                            .child("当前 ACP 服务无法读取自动化任务"),
                    )
                    .child(
                        div()
                            .max_w(px(520.))
                            .text_size(px(12.))
                            .line_height(px(19.))
                            .text_color(p.faint)
                            .child("这不表示没有任务。筛选和搜索已停用，避免展示错误的空列表；你仍可通过对话创建任务，并在会话中查看真实执行结果。"),
                    )
                    .child(self.assisted_button(
                        "empty-new-automation",
                        "在会话中创建",
                        Glyph::Comment,
                        CREATE_AUTOMATION_PROMPT,
                        false,
                        cx,
                    )),
            )
    }

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

    fn assisted_button(
        &self,
        id: &'static str,
        label: &'static str,
        glyph: Glyph,
        prompt: &'static str,
        primary: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let enabled = self.connected() && !self.creating_session;
        let blue = if super::workspace::dark(cx) {
            p.button_info_fill
        } else {
            p.button_info_hover
        };
        let button = row()
            .id(id)
            .role(gpui::Role::Button)
            .aria_label(label)
            .tab_index(0)
            .tab_stop(enabled)
            .h(px(32.))
            .flex_none()
            .px(px(12.))
            .gap(px(6.))
            .rounded(px(16.))
            .border(px(0.5))
            .border_color(if primary { blue } else { p.line })
            .bg(if primary { blue } else { p.panel })
            .text_size(px(13.))
            .line_height(px(20.))
            .text_color(if primary {
                p.label_primary_foreground
            } else {
                p.ink
            })
            .child(icon(
                glyph,
                13.,
                if primary {
                    p.label_primary_foreground
                } else {
                    p.secondary
                },
            ))
            .child(label);
        if enabled {
            button
                .cursor_pointer()
                .hover(move |style| {
                    if primary {
                        style.bg(blue.opacity(0.86))
                    } else {
                        style.bg(p.hover)
                    }
                })
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.start_assisted_session(prompt, w, cx);
                }))
        } else {
            button.opacity(0.45).cursor_default()
        }
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

fn section_title(title: &'static str) -> impl IntoElement {
    div()
        .text_size(px(14.))
        .line_height(px(22.))
        .font_weight(FontWeight::MEDIUM)
        .child(title)
}

fn capability_notice(
    title: &'static str,
    detail: &'static str,
    glyph: Glyph,
    cx: &App,
) -> impl IntoElement {
    let p = dsh(cx);
    row()
        .w_full()
        .p(px(16.))
        .gap(px(12.))
        .rounded(px(12.))
        .border(px(0.5))
        .border_color(p.line)
        .bg(p.stage)
        .child(icon(glyph, 24., p.secondary))
        .child(
            column()
                .flex_1()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(14.))
                        .line_height(px(22.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(20.))
                        .text_color(p.secondary)
                        .child(detail),
                ),
        )
}
