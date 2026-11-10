//! Full-access acknowledgement dialog and shared modal actions.

use super::workspace::{composer_bg, conversation_bg};
use super::*;

impl DshApp {
    pub(super) fn close_permission_confirmation(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.confirm_permission = None;
        self.full_access_acknowledged = false;
        if self.composer_menu == Some(ComposerMenu::PermissionChoices) {
            self.focus_command_picker(w, cx);
        } else {
            self.permission_menu.trigger.focus(w, cx);
        }
        cx.notify();
    }

    pub(super) fn render_full_access_dialog(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let width = (f32::from(w.viewport_size().width) - 48.).clamp(0., 440.);
        let acknowledged = self.full_access_acknowledged;
        let (title, description, acknowledgement, action) = if self.confirm_permission.as_deref()
            == Some("auto")
        {
            (
                "确认启用自动审查（实验）？",
                "自动审查不使用沙箱。每次原生工具调用和 PTC 内层调用前，都会由与当前 agent 相同的模型进行审查；审查拒绝的调用由你批准或拒绝。此功能仍属实验性，可能误放行或误拒绝，并会消耗额外 token。",
                "我已了解这些风险，并愿意继续",
                "启用自动审查",
            )
        } else {
            (
                "确认启用完全权限？",
                "启用完全权限后，智能体将减少确认步骤，并且可以直接执行更多操作，包括敏感操作、文件修改或外部命令。仅建议在你信任当前任务时使用。",
                "我已了解风险，并愿意继续",
                "启用完全权限",
            )
        };
        div()
            .id("full-access-dialog")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.bg_mask_1)
            .occlude()
            .on_click(cx.listener(|this, _, w, cx| this.close_permission_confirmation(w, cx)))
            .child(
                column()
                    .id("full-access-dialog-panel")
                    .w(px(width))
                    .pb(px(24.))
                    .rounded(px(28.))
                    .bg(composer_bg(cx))
                    .shadow(elevation_prominent())
                    .occlude()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                    .child(dialog_header(
                        title,
                        "close-full-access-dialog",
                        None,
                        cx,
                        |this, w, cx| this.close_permission_confirmation(w, cx),
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
                                    .child(div().pt(px(2.)).flex_none().child(icon(
                                        Glyph::Alert,
                                        16.,
                                        p.danger,
                                    )))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(14.))
                                            .line_height(px(22.))
                                            .text_color(p.secondary)
                                            .child(description),
                                    ),
                            )
                            .child(
                                row()
                                    .id("full-access-acknowledgement")
                                    .role(gpui::Role::CheckBox)
                                    .aria_label(acknowledgement)
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
                                            .child(acknowledgement),
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
                                |this, w, cx| this.close_permission_confirmation(w, cx),
                            ))
                            .child(dialog_action(
                                "confirm-full-access",
                                action,
                                true,
                                !acknowledged,
                                136.,
                                cx,
                                |this, w, cx| {
                                    if this.full_access_acknowledged
                                        && let Some(preset) = this.confirm_permission.take()
                                    {
                                        this.close_permission_confirmation(w, cx);
                                        this.apply_permission(preset, true);
                                    }
                                },
                            )),
                    ),
            )
    }
}

pub(super) fn dialog_header(
    title: &'static str,
    id: &'static str,
    focus: Option<&gpui::FocusHandle>,
    cx: &mut Context<DshApp>,
    action: impl Fn(&mut DshApp, &mut Window, &mut Context<DshApp>) + 'static,
) -> gpui::Div {
    let p = dsh(cx);
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
        .child(super::dock_action_button(
            div()
                .id(id)
                .role(gpui::Role::Button)
                .aria_label("关闭")
                .when_some(focus, |button, focus| button.track_focus(focus))
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
                    gpui::svg()
                        .data(include_bytes!("../../assets/session-log-close.svg"))
                        .size(px(14.))
                        .text_color(p.label_secondary),
                ),
            cx.listener(move |this, _, w, cx| action(this, w, cx)),
        ))
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
    let p = dsh(cx);
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
            p.border_l3
        })
        .text_size(px(14.))
        .line_height(px(22.))
        .text_color(if primary {
            p.label_primary_foreground
        } else {
            p.label_primary
        })
        .child(label);
    if primary {
        button = button.bg(p.button_primary_fill);
    }
    if disabled {
        button
            .opacity(0.4)
            .cursor(gpui::CursorStyle::OperationNotAllowed)
    } else {
        super::dock_action_button(
            button.cursor_pointer().hover(move |style| {
                style.bg(if primary {
                    p.button_primary_hover
                } else {
                    p.hover
                })
            }),
            cx.listener(move |this, _, w, cx| action(this, w, cx)),
        )
    }
}
