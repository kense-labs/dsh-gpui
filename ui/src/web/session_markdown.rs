//! Session Markdown copy status and the workspace-level announcement.

use super::*;
use gpui::{deferred, svg};
use std::time::Duration;
use wasm_bindgen_futures::JsFuture;

const HOLD_MS: f64 = 3000.;
const END_MS: f64 = 4000.;

#[derive(Default)]
pub(super) struct SessionMarkdownCopy {
    pub(super) pending: bool,
    notice: Option<(f64, Notice)>,
}

#[derive(Clone, Copy)]
enum Notice {
    Success,
    PreparationFailed,
    ClipboardFailed,
}

impl DshApp {
    pub(super) fn copy_session_markdown(&mut self, cx: &mut Context<Self>) {
        if self.session_markdown.pending {
            return;
        }
        let Some(id) = self.active_session.as_deref() else {
            return;
        };
        let cwd = self
            .sessions
            .iter()
            .find(|session| session.id == id)
            .map_or("", |session| session.cwd.as_str());
        let request = bridge::copy_session_markdown(id, cwd);
        self.session_markdown.pending = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = JsFuture::from(request).await;
            let notice = match result.ok().and_then(|value| value.as_string()).as_deref() {
                Some("success") => Notice::Success,
                Some("clipboardFailed") => Notice::ClipboardFailed,
                _ => Notice::PreparationFailed,
            };
            let at = bridge::monotonic_ms();
            let _ = this.update(cx, |app, cx| {
                app.session_markdown.pending = false;
                app.session_markdown.notice = Some((at, notice));
                cx.notify();
            });
            cx.background_executor()
                .timer(Duration::from_millis(HOLD_MS as u64))
                .await;
            let _ = this.update(cx, |_, cx| cx.notify());
            cx.background_executor()
                .timer(Duration::from_millis((END_MS - HOLD_MS) as u64))
                .await;
            let _ = this.update(cx, |app, cx| {
                if app
                    .session_markdown
                    .notice
                    .is_some_and(|(shown, _)| shown == at)
                {
                    app.session_markdown.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn render_session_markdown_notice(
        &self,
        w: &mut Window,
        cx: &App,
    ) -> Option<impl IntoElement> {
        let (at, notice) = self.session_markdown.notice?;
        let (text, success) = match notice {
            Notice::Success => ("Session 已复制为 Markdown", true),
            Notice::PreparationFailed => ("无法读取 Session 进行复制，请重试", false),
            Notice::ClipboardFailed => ("无法写入剪贴板，请检查剪贴板权限后重试", false),
        };
        SessionNotice {
            text,
            success,
            at,
            hold: HOLD_MS,
        }
        .render(0., 0., w, cx)
    }
}

#[derive(Clone, Copy)]
pub(super) struct SessionNotice {
    pub(super) text: &'static str,
    pub(super) success: bool,
    pub(super) at: f64,
    pub(super) hold: f64,
}

impl SessionNotice {
    pub(super) fn render(
        self,
        left: f32,
        right: f32,
        w: &mut Window,
        cx: &App,
    ) -> Option<impl IntoElement> {
        let age = bridge::monotonic_ms() - self.at;
        if age >= self.hold + 1000. {
            return None;
        }
        let p = dsh(cx);
        let (icon, color) = if self.success {
            (
                include_bytes!("../../assets/session-export-success.svg").as_slice(),
                p.state_success_primary,
            )
        } else {
            (
                include_bytes!("../../assets/session-export-warning.svg").as_slice(),
                p.state_warn_label,
            )
        };
        let enter = if cx.reduce_motion() {
            1.
        } else {
            crate::right_dock::dock_easing((age / 160.).clamp(0., 1.), 0., 0.58)
        };
        let opacity = if age >= self.hold {
            1. - crate::right_dock::cubic_bezier_easing(
                (age - self.hold) / 1000.,
                [0.25, 0.1, 0.25, 1.],
            )
        } else {
            enter
        };
        if enter < 1. || age >= self.hold {
            w.request_animation_frame();
        }
        Some(
            deferred(
                row()
                    .absolute()
                    .top(px(40. - 6. * (1. - enter) as f32))
                    .left(px(left))
                    .right(px(right))
                    .justify_center()
                    .child(
                        row()
                            .id(("session-notice", self.at.to_bits()))
                            .role(gpui::Role::Alert)
                            .max_w(px(640.).min((w.viewport_size().width - px(48.)).max(px(0.))))
                            .gap(px(10.))
                            .px(px(16.))
                            .py(px(12.))
                            .rounded(px(12.))
                            .bg(p.toast_bg)
                            .shadow(crate::tokens::shadow_lv3())
                            .text_size(px(14.))
                            .line_height(px(22.))
                            .text_color(gpui::white())
                            .opacity(opacity as f32)
                            .child(svg().data(icon).size(px(16.)).flex_none().text_color(color))
                            .child(div().min_w_0().child(self.text)),
                    ),
            )
            .with_priority(8),
        )
    }
}
