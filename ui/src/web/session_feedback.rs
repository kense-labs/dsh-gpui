//! GPUI session feedback form backed by the live Harness record service.

use super::{
    dock_controls::dock_action_button, message_feedback::Rating, session_markdown::SessionNotice, *,
};
use gpui::{Animation, AnimationExt, Entity, FocusHandle, deferred, svg};
use std::time::Duration;

const HINT: &str = "填写详情以帮助我们改进体验，提交内容会包括当前对话的日志";
const CATEGORIES: [(&str, &str); 7] = [
    ("task-result", "任务结果"),
    ("instruction-following", "指令理解与遵循"),
    ("product-interaction", "产品功能与交互"),
    ("service-stability", "稳定性和速度"),
    ("resource-cost", "资源使用与费用"),
    ("security-privacy-permission", "安全隐私与权限"),
    ("other", "其他"),
];

#[derive(Clone)]
enum FeedbackTarget {
    Session,
    Message { id: String, rating: Rating },
}

struct FeedbackDraft {
    id: u64,
    session: String,
    target: FeedbackTarget,
    category: Option<usize>,
    submitting: bool,
    return_focus: Option<FocusHandle>,
}

pub(super) struct SessionFeedback {
    draft: Option<FeedbackDraft>,
    notice: Option<SessionNotice>,
    editor: Entity<Editor>,
    dialog: FocusHandle,
    close: FocusHandle,
    submit: FocusHandle,
    chips: [FocusHandle; CATEGORIES.len()],
    next: u64,
    _edit: Subscription,
}

impl SessionFeedback {
    pub(super) fn new(w: &mut Window, cx: &mut Context<DshApp>) -> Self {
        let editor = cx.new(|cx| {
            let mut editor = Editor::new("反馈详情", InputMode::Multi, w, cx);
            editor.set_placeholder(HINT);
            editor.font_size = 14.;
            editor
        });
        let edit = cx.subscribe_in(&editor, w, |app, _, event: &EditEvent, w, cx| match event {
            EditEvent::Escape => app.dismiss_session_feedback(w, cx),
            EditEvent::Changed | EditEvent::Focused | EditEvent::Blurred => cx.notify(),
            _ => {}
        });
        Self {
            draft: None,
            notice: None,
            editor,
            dialog: cx.focus_handle(),
            close: cx.focus_handle().tab_stop(true),
            submit: cx.focus_handle().tab_stop(true),
            chips: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            next: 0,
            _edit: edit,
        }
    }

    pub(super) fn visible(&self) -> bool {
        self.draft.is_some()
    }

    fn cycle_focus(&self, backward: bool, w: &mut Window, cx: &mut App) {
        let mut stops = vec![self.close.clone()];
        if self.draft.as_ref().is_some_and(|draft| !draft.submitting) {
            stops.extend(self.chips.iter().cloned());
            stops.push(self.editor.read(cx).focus.clone());
            stops.push(self.submit.clone());
        }
        let current = stops.iter().position(|stop| stop.is_focused(w));
        let next = if backward {
            current
                .filter(|&i| i > 0)
                .map_or(stops.len() - 1, |i| i - 1)
        } else {
            current.map_or(0, |i| (i + 1) % stops.len())
        };
        stops[next].focus(w, cx);
    }
}

impl DshApp {
    pub(super) fn open_session_feedback(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.active_session.clone() else {
            return;
        };
        self.open_feedback(session, FeedbackTarget::Session, w, cx);
    }

    pub(super) fn open_message_feedback(
        &mut self,
        session: String,
        id: String,
        rating: Rating,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_feedback(session, FeedbackTarget::Message { id, rating }, w, cx);
    }

    fn open_feedback(
        &mut self,
        session: String,
        target: FeedbackTarget,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = &mut self.session_feedback;
        let return_focus = form
            .draft
            .as_ref()
            .and_then(|draft| draft.return_focus.clone())
            .or_else(|| w.focused(cx));
        form.next += 1;
        form.draft = Some(FeedbackDraft {
            id: form.next,
            session,
            target,
            category: None,
            submitting: false,
            return_focus,
        });
        form.editor.update(cx, |editor, cx| {
            editor.disabled = false;
            editor.set_value("", w, cx);
        });
        document_html::restore_shell_focus();
        form.dialog.focus(w, cx);
        cx.notify();
    }

    pub(super) fn dismiss_session_feedback(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if let Some(draft) = self.session_feedback.draft.take() {
            if let Some(focus) = draft.return_focus {
                focus.focus(w, cx);
            }
            cx.notify();
        }
    }

    pub(super) fn clear_session_feedback(&mut self) {
        self.session_feedback.draft = None;
        self.session_feedback.notice = None;
    }

    fn submit_session_feedback(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self
            .session_feedback
            .draft
            .as_mut()
            .filter(|draft| !draft.submitting)
        else {
            return;
        };
        let form = &self.session_feedback.editor;
        let session = draft.session.clone();
        let id = draft.id;
        let target = draft.target.clone();
        let category = draft.category.map(|index| CATEGORIES[index].0);
        let text = trim_feedback(form.read(cx).value()).to_owned();
        draft.submitting = true;
        form.update(cx, |editor, cx| {
            editor.disabled = true;
            cx.notify();
        });
        self.session_feedback.dialog.focus(w, cx);
        cx.notify();
        match target {
            FeedbackTarget::Session => {
                self.record_session_feedback(session, text, category, Some(id), w, cx)
            }
            FeedbackTarget::Message {
                id: message,
                rating,
            } => self.submit_message_feedback(message, rating, id, text, category, w, cx),
        }
    }

    pub(super) fn submit_feedback_command(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let text = self.composer.read(cx).value().trim();
        // Both official spellings of the builtin token must reach the same panel.
        // Handling only the English one sent `/反馈` down the host path instead,
        // where an empty argument is an error, so the two spellings behaved
        // differently.
        let Some(remark) = crate::commands::strip_command_token(text, "feedback") else {
            return false;
        };
        let remark = trim_feedback(remark).to_owned();
        let Some(session) = self.active_session.clone() else {
            return true;
        };
        self.composer_menu = None;
        self.replace_composer_without_trigger("", w, cx);
        if remark.is_empty() {
            self.open_session_feedback(w, cx);
        } else {
            self.record_session_feedback(session, remark, None, None, w, cx);
        }
        true
    }

    fn record_session_feedback(
        &mut self,
        session: String,
        text: String,
        category: Option<&'static str>,
        draft: Option<u64>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut args = json!({"sessionId":session});
        if !text.is_empty() {
            args["text"] = text.into();
        }
        if let Some(category) = category {
            args["category"] = category.into();
        }
        cx.spawn_in(w, async move |this, cx| {
            let reply = bridge::invoke("record_session_feedback", args)
                .await
                .map(|_| ());
            let _ = this.update_in(cx, |app, w, cx| {
                if app.active_session.as_deref() == Some(session.as_str()) {
                    app.finish_feedback(draft, reply, w, cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn finish_feedback(
        &mut self,
        draft: Option<u64>,
        reply: Result<(), String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let success = reply.is_ok();
        let same_draft = draft.is_some_and(|id| {
            self.session_feedback
                .draft
                .as_ref()
                .is_some_and(|draft| draft.id == id)
        });
        if success {
            if same_draft {
                self.dismiss_session_feedback(w, cx);
            }
        } else if same_draft {
            if let Some(draft) = self.session_feedback.draft.as_mut() {
                draft.submitting = false;
            }
            self.session_feedback.editor.update(cx, |editor, cx| {
                editor.disabled = false;
                cx.notify();
            });
        } else if draft.is_some() {
            return;
        }
        let text = match reply.as_ref().err() {
            None => "感谢你的反馈",
            Some(error) => feedback_failure(error),
        };
        let hold = if success { 3000 } else { 6000 };
        let at = bridge::monotonic_ms();
        self.session_feedback.notice = Some(SessionNotice {
            text,
            success,
            at,
            hold: hold as f64,
        });
        cx.notify();
        cx.spawn_in(w, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(hold))
                .await;
            let _ = this.update_in(cx, |_, _, cx| cx.notify());
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let _ = this.update_in(cx, |app, _, cx| {
                if app
                    .session_feedback
                    .notice
                    .as_ref()
                    .is_some_and(|notice| notice.at == at)
                {
                    app.session_feedback.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn render_session_feedback_notice(
        &self,
        w: &mut Window,
        cx: &App,
    ) -> Option<impl IntoElement> {
        let notice = self.session_feedback.notice.as_ref()?;
        let left = if self.rail_active(w) {
            super::workspace::collapsed_sidebar_width()
        } else {
            self.sidebar_width
        };
        notice.render(left, self.right_dock_frame.track, w, cx)
    }

    pub(super) fn render_session_feedback(
        &self,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let form = &self.session_feedback;
        let draft = form.draft.as_ref()?;
        let p = dsh(cx);
        let width = (f32::from(w.viewport_size().width) - 48.).clamp(0., 488.);
        let value = form.editor.read(cx).value();
        let height = detail_height(value, width, w, cx);
        let focused = form.editor.read(cx).focus.is_focused(w);
        let caption = if bridge::is_mac() && !self.shell_fullscreen(w) {
            40.
        } else {
            0.
        };
        let dismiss = cx.listener(|app, _: &(), w, cx| {
            app.dismiss_session_feedback(w, cx);
            cx.stop_propagation();
        });
        let modal = div()
            .id("session-feedback-layer")
            .absolute()
            .inset_0()
            .px(px(24.))
            .py(px(24.))
            .flex()
            .justify_center()
            .items_center()
            .occlude()
            .on_click(cx.listener(|app, _, w, cx| app.dismiss_session_feedback(w, cx)))
            .child(div().absolute().inset_0().top(px(caption)).bg(p.bg_mask_1))
            .child(
                column()
                    .id("session-feedback-dialog")
                    .role(gpui::Role::Dialog)
                    .aria_label("提交反馈")
                    .track_focus(&form.dialog)
                    .w(px(width))
                    .max_h((w.viewport_size().height - px(48.)).max(px(0.)))
                    .overflow_y_scroll()
                    .pb(px(24.))
                    .gap(px(38.))
                    .rounded(px(28.))
                    .bg(p.bg_layer_2)
                    .shadow(crate::tokens::elevation_prominent())
                    .occlude()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .capture_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, w, cx| {
                        let stroke = &event.keystroke;
                        if stroke.key == "tab"
                            && !stroke.modifiers.platform
                            && !stroke.modifiers.alt
                            && !stroke.modifiers.control
                        {
                            app.session_feedback
                                .cycle_focus(stroke.modifiers.shift, w, cx);
                            w.prevent_default();
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        column()
                            .w_full()
                            .gap_0()
                            .child(
                                row()
                                    .w_full()
                                    .pl(px(24.))
                                    .pr(px(14.))
                                    .pt(px(22.))
                                    .pb(px(12.))
                                    .gap(px(8.))
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .line_height(px(24.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(p.label_primary)
                                            .child("提交反馈"),
                                    )
                                    .child(dock_action_button(
                                        row()
                                            .id("session-feedback-close")
                                            .role(gpui::Role::Button)
                                            .aria_label("关闭")
                                            .track_focus(&form.close)
                                            .size(px(28.))
                                            .flex_none()
                                            .justify_center()
                                            .rounded(px(8.))
                                            .cursor_pointer()
                                            .hover(|style| style.bg(p.hover))
                                            .child(
                                                svg()
                                                    .data(include_bytes!(
                                                        "../../assets/session-log-close.svg"
                                                    ))
                                                    .size(px(14.))
                                                    .text_color(p.label_secondary),
                                            ),
                                        dismiss,
                                    )),
                            )
                            .child(
                                column()
                                    .w_full()
                                    .px(px(24.))
                                    .pt(px(6.))
                                    .gap(px(18.))
                                    .child(
                                        row()
                                            .id("session-feedback-categories")
                                            .role(gpui::Role::Group)
                                            .aria_label("反馈分类")
                                            .w_full()
                                            .flex_wrap()
                                            .gap(px(8.))
                                            .children(CATEGORIES.iter().enumerate().map(
                                                |(index, &(id, label))| {
                                                    let active = draft.category == Some(index);
                                                    let pending = draft.submitting;
                                                    dock_action_button(
                                                        row()
                                                            .id(id)
                                                            .role(gpui::Role::Button)
                                                            .aria_label(label)
                                                            .aria_selected(active)
                                                            .track_focus(&form.chips[index])
                                                            .tab_stop(!pending)
                                                            .h(px(28.))
                                                            .px(px(12.))
                                                            .flex_none()
                                                            .rounded(px(8.))
                                                            .border(px(0.5))
                                                            .border_color(if active {
                                                                p.button_primary_fill
                                                            } else {
                                                                p.border_l4
                                                            })
                                                            .text_color(if active {
                                                                p.label_primary_foreground
                                                            } else {
                                                                p.label_primary
                                                            })
                                                            .text_size(px(13.))
                                                            .line_height(px(18.))
                                                            .when(active, |button| {
                                                                button.bg(p.button_primary_fill)
                                                            })
                                                            .when(!pending && !active, |button| {
                                                                button.hover(|style| {
                                                                    style.bg(p.hover)
                                                                })
                                                            })
                                                            .when(!pending, |button| {
                                                                button
                                                                    .cursor_pointer()
                                                                    .focus_visible(|style| {
                                                                        style.shadow(vec![
                                                                            gpui::BoxShadow::new(
                                                                                px(0.),
                                                                                px(0.),
                                                                                p.brand_accent,
                                                                            )
                                                                            .spread_radius(px(2.)),
                                                                        ])
                                                                    })
                                                            })
                                                            .when(pending, |button| {
                                                                button.opacity(0.4).cursor_default()
                                                            })
                                                            .child(label),
                                                        cx.listener(move |app, _, _, cx| {
                                                            if let Some(draft) = app
                                                                .session_feedback
                                                                .draft
                                                                .as_mut()
                                                                .filter(|draft| !draft.submitting)
                                                            {
                                                                draft.category = (draft.category
                                                                    != Some(index))
                                                                .then_some(index);
                                                                cx.notify();
                                                            }
                                                        }),
                                                    )
                                                },
                                            )),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .h(px(height))
                                            .px(px(14.))
                                            .py(px(12.))
                                            .rounded(px(12.))
                                            .border(px(0.5))
                                            .border_color(if focused {
                                                p.border_l3
                                            } else {
                                                p.border_l4
                                            })
                                            .bg(p.bg_layer_1)
                                            .text_color(p.label_primary)
                                            .when(focused, |element| {
                                                element.shadow(vec![
                                                    gpui::BoxShadow::new(
                                                        px(0.),
                                                        px(0.),
                                                        p.state_business_primary,
                                                    )
                                                    .spread_radius(px(1.)),
                                                ])
                                            })
                                            .child(form.editor.clone()),
                                    ),
                            ),
                    )
                    .child(
                        row().w_full().px(px(24.)).child(dock_action_button(
                            row()
                                .id("session-feedback-submit")
                                .role(gpui::Role::Button)
                                .track_focus(&form.submit)
                                .tab_stop(!draft.submitting)
                                .w_full()
                                .h(px(44.))
                                .justify_center()
                                .rounded(px(12.))
                                .bg(p.button_primary_fill)
                                .text_color(p.label_primary_foreground)
                                .text_size(px(14.))
                                .font_weight(FontWeight::MEDIUM)
                                .when(!draft.submitting, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| style.bg(p.button_primary_hover))
                                })
                                .when(draft.submitting, |button| {
                                    button.opacity(0.4).cursor_not_allowed()
                                })
                                .child(if draft.submitting {
                                    "正在提交…"
                                } else {
                                    "提交"
                                }),
                            cx.listener(|app, _, w, cx| app.submit_session_feedback(w, cx)),
                        )),
                    ),
            );
        Some(
            deferred(modal.with_animation(
                ("session-feedback-in", draft.id),
                Animation::new(crate::anim::STANDARD).with_easing(|progress| {
                    crate::right_dock::cubic_bezier_easing(f64::from(progress), [0.4, 0., 0.2, 1.])
                        as f32
                }),
                |layer, progress| layer.opacity(progress),
            ))
            .with_priority(7),
        )
    }
}

fn detail_height(value: &str, width: f32, w: &Window, cx: &App) -> f32 {
    let run = gpui::TextRun {
        len: value.len(),
        font: super::document_fonts::interface_style(w, cx).font(),
        ..Default::default()
    };
    let wrap_width = px((width - 77.).max(1.));
    let rows = w
        .text_system()
        .shape_text(
            value.to_owned().into(),
            px(14.),
            &[run],
            Some(wrap_width),
            None,
        )
        .map(|lines| {
            lines
                .iter()
                .map(|line| line.wrap_boundaries().len() + 1)
                .sum::<usize>()
        })
        .unwrap_or(1);
    (rows as f32 * 22. + 25.).clamp(116., 280.)
}

pub(super) fn feedback_failure(code: &str) -> &'static str {
    match code {
        "note-too-large" => "描述太长，请缩短后再提交",
        "version-conflict" => "这条反馈已在别处改动，已显示最新状态",
        _ => "反馈保存失败",
    }
}

fn trim_feedback(text: &str) -> &str {
    // ECMAScript trim, matching the dialog and the Host producer.
    text.trim_matches(|ch: char| ch == '\u{feff}' || (ch != '\u{0085}' && ch.is_whitespace()))
}
