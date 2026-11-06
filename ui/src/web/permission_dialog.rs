//! Full-access acknowledgement dialog and shared modal actions.

use super::workspace::{composer_bg, conversation_bg};
use super::*;

impl DshApp {
    pub(super) fn close_full_access_confirmation(&mut self) {
        self.confirm_full_access = false;
        self.full_access_acknowledged = false;
    }

    pub(super) fn render_full_access_dialog(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        let width = (f32::from(w.viewport_size().width) - 48.).clamp(0., 440.);
        let acknowledged = self.full_access_acknowledged;
        div()
            .id("full-access-dialog")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000066))
            .occlude()
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_full_access_confirmation();
                cx.notify();
            }))
            .child(
                column()
                    .id("full-access-dialog-panel")
                    .w(px(width))
                    .pb(px(24.))
                    .rounded(px(28.))
                    .bg(composer_bg(cx))
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation()
                    })
                    .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                    .child(dialog_header(
                        "确认启用完全权限？",
                        "close-full-access-dialog",
                        cx,
                        |this, cx| {
                            this.close_full_access_confirmation();
                            cx.notify();
                        },
                    ))
                    .child(
                        column()
                            .w_full()
                            .px(px(24.))
                            .child(
                                row()
                                    .w_full()
                                    .items_start()
                                    .gap(px(10.))
                                    .child(
                                        div().pt(px(2.)).flex_none().child(icon(
                                            Glyph::Alert,
                                            16.,
                                            p.danger,
                                        )),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(14.))
                                            .line_height(px(22.))
                                            .text_color(p.secondary)
                                            .child("启用完全权限后，智能体将减少确认步骤，并且可以直接执行更多操作，包括敏感操作、文件修改或外部命令。仅建议在你信任当前任务时使用。"),
                                    ),
                            )
                            .child(
                                row()
                                    .id("full-access-acknowledgement")
                                    .role(gpui::Role::CheckBox)
                                    .aria_label("我已了解风险，并愿意继续")
                                    .aria_selected(acknowledged)
                                    .tab_index(0)
                                    .w_full()
                                    .mt(px(16.))
                                    .items_start()
                                    .gap(px(8.))
                                    .cursor_pointer()
                                    .child(
                                        div()
                                            .size(px(14.))
                                            .mt(px(3.))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(3.))
                                            .border(px(1.))
                                            .border_color(if acknowledged {
                                                p.ink
                                            } else {
                                                p.faint
                                            })
                                            .when(acknowledged, |element| {
                                                element.bg(p.ink).child(icon(
                                                    Glyph::Check,
                                                    11.,
                                                    conversation_bg(cx),
                                                ))
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(14.))
                                            .line_height(px(20.))
                                            .text_color(p.secondary)
                                            .child("我已了解风险，并愿意继续"),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.full_access_acknowledged =
                                            !this.full_access_acknowledged;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        row()
                            .w_full()
                            .mt(px(20.))
                            .px(px(24.))
                            .justify_end()
                            .gap(px(8.))
                            .child(dialog_action(
                                "cancel-full-access",
                                "取消",
                                false,
                                false,
                                72.,
                                cx,
                                |this, _, cx| {
                                    this.close_full_access_confirmation();
                                    cx.notify();
                                },
                            ))
                            .child(dialog_action(
                                "confirm-full-access",
                                "启用完全权限",
                                true,
                                !acknowledged,
                                136.,
                                cx,
                                |this, _, cx| {
                                    this.close_full_access_confirmation();
                                    this.error = Some(
                                        "当前 ACP 服务暂未提供权限模式切换，已保持“工作区内修改”。"
                                            .to_string(),
                                    );
                                    cx.notify();
                                },
                            )),
                    ),
            )
    }
}

pub(super) fn dialog_header(
    title: &'static str,
    id: &'static str,
    cx: &mut Context<DshApp>,
    action: impl Fn(&mut DshApp, &mut Context<DshApp>) + 'static,
) -> gpui::Div {
    let p = Paint::get(cx);
    row()
        .w_full()
        .pl(px(24.))
        .pr(px(14.))
        .pt(px(22.))
        .pb(px(12.))
        .gap(px(8.))
        .items_center()
        .justify_between()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(16.))
                .line_height(px(24.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.ink)
                .child(title),
        )
        .child(
            div()
                .id(id)
                .role(gpui::Role::Button)
                .aria_label("关闭")
                .tab_index(0)
                .size(px(28.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|style| style.bg(p.hover))
                .child(icon(Glyph::Close, 14., p.secondary))
                .on_click(cx.listener(move |this, _, _, cx| action(this, cx))),
        )
}

pub(super) fn dialog_action(
    id: &'static str,
    label: &'static str,
    primary: bool,
    disabled: bool,
    min_width: f32,
    cx: &mut Context<DshApp>,
    action: impl Fn(&mut DshApp, &mut Window, &mut Context<DshApp>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let p = Paint::get(cx);
    let mut button = div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .tab_index(0)
        .tab_stop(!disabled)
        .h(px(36.))
        .min_w(px(min_width))
        .px(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(12.))
        .border(px(0.5))
        .border_color(if primary {
            gpui::transparent_black()
        } else {
            p.line
        })
        .text_size(px(14.))
        .line_height(px(22.))
        .text_color(if primary { conversation_bg(cx) } else { p.ink });
    if primary {
        button = button.bg(p.ink);
    }
    if disabled {
        button.opacity(0.4).cursor_default()
    } else {
        button
            .cursor_pointer()
            .hover(move |style| {
                if primary {
                    style.bg(p.ink.opacity(0.86))
                } else {
                    style.bg(p.hover)
                }
            })
            .on_click(cx.listener(move |this, _, w, cx| action(this, w, cx)))
    }
}
