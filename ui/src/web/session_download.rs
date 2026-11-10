//! Session log download queue and official export dialog in GPUI.

use super::{dock_controls::dock_action_button, *};
use gpui::{Animation, AnimationExt, FocusHandle, deferred, svg};

const CLOSE: &[u8] = include_bytes!("../../assets/session-log-close.svg");

enum DownloadStatus {
    Preparing,
    Started,
    Error(String),
}

struct Download {
    request: u64,
    session: String,
    active: bool,
    open: bool,
    status: DownloadStatus,
}

pub(super) struct SessionDownloads {
    entries: Vec<Download>,
    next: u64,
    dialog: FocusHandle,
    close: [FocusHandle; 2],
    return_focus: Option<FocusHandle>,
}

impl SessionDownloads {
    pub(super) fn new(cx: &Context<DshApp>) -> Self {
        Self {
            entries: Vec::new(),
            next: 0,
            dialog: cx.focus_handle(),
            close: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            return_focus: None,
        }
    }

    pub(super) fn pending(&self, session: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.session == session && entry.active)
    }

    pub(super) fn visible(&self) -> bool {
        self.current().is_some()
    }

    fn current(&self) -> Option<&Download> {
        self.entries.iter().find(|entry| entry.open)
    }

    pub(super) fn started(&mut self, event: &Value) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.active
                && event["requestId"].as_u64() == Some(entry.request)
                && event["sessionId"].as_str() == Some(entry.session.as_str())
        }) {
            entry.status = DownloadStatus::Started;
        }
    }
}

impl DshApp {
    pub(super) fn start_session_download(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.active_session.clone() else {
            return;
        };
        if self.session_downloads.pending(&session) {
            return;
        }
        let cwd = self
            .sessions
            .iter()
            .find(|entry| entry.id == session)
            .map_or("", |entry| entry.cwd.as_str());
        let downloads = &mut self.session_downloads;
        downloads.next += 1;
        let request = downloads.next;
        if !downloads.visible() {
            downloads.return_focus = w.focused(cx);
            document_html::restore_shell_focus();
            downloads.dialog.focus(w, cx);
        }
        let args = json!({ "requestId": request, "sessionId": session, "cwd": cwd });
        downloads.entries.push(Download {
            request,
            session,
            active: true,
            open: true,
            status: DownloadStatus::Preparing,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let reply = bridge::invoke("download_session_log", args).await;
            let _ = this.update(cx, |app, cx| {
                let downloads = &mut app.session_downloads;
                if let Some(entry) = downloads
                    .entries
                    .iter_mut()
                    .find(|entry| entry.request == request)
                {
                    entry.active = false;
                    entry.status = match reply {
                        Ok(_) => DownloadStatus::Started,
                        Err(error) => DownloadStatus::Error(error),
                    };
                }
                downloads.entries.retain(|entry| entry.active || entry.open);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn dismiss_session_download(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let downloads = &mut self.session_downloads;
        let Some(entry) = downloads.entries.iter_mut().find(|entry| entry.open) else {
            return;
        };
        entry.open = false;
        downloads.entries.retain(|entry| entry.active || entry.open);
        if downloads.visible() {
            downloads.dialog.focus(w, cx);
        } else if let Some(focus) = downloads.return_focus.take() {
            focus.focus(w, cx);
        }
        cx.notify();
    }

    pub(super) fn render_session_download(
        &self,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let entry = self.session_downloads.current()?;
        let p = dsh(cx);
        let (title, description) = match &entry.status {
            DownloadStatus::Preparing => (
                "正在导出 Session",
                "正在准备包含指定 Session、子 Session 和附件的 ZIP 文件",
            ),
            DownloadStatus::Started => (
                "Session 导出已开始下载",
                "浏览器正在下载 Session ZIP 文件。",
            ),
            DownloadStatus::Error(error) => ("Session 导出失败", error.as_str()),
        };
        let caption = if bridge::is_mac() && !self.shell_fullscreen(w) {
            40.
        } else {
            0.
        };
        let width = (f32::from(w.viewport_size().width) - 48.).clamp(0., 380.);
        let [header_close, footer_close] = &self.session_downloads.close;
        let dismiss = |app: &mut DshApp, _: &(), w: &mut Window, cx: &mut Context<DshApp>| {
            app.dismiss_session_download(w, cx);
            cx.stop_propagation();
        };
        let modal = div()
            .id("session-log-download-layer")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_center()
            .occlude()
            .on_click(cx.listener(|app, _, w, cx| app.dismiss_session_download(w, cx)))
            .child(div().absolute().inset_0().top(px(caption)).bg(p.bg_mask_1))
            .child(
                column()
                    .id("session-log-download-dialog")
                    .role(gpui::Role::Dialog)
                    .aria_label(title)
                    .track_focus(&self.session_downloads.dialog)
                    .w(px(width))
                    .pb(px(24.))
                    .gap(px(20.))
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
                            let [first, last] = &app.session_downloads.close;
                            let to_last = if stroke.modifiers.shift {
                                !last.is_focused(w)
                            } else {
                                first.is_focused(w)
                            };
                            if to_last {
                                last.focus(w, cx);
                            } else {
                                first.focus(w, cx);
                            }
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
                                            .min_w_0()
                                            .text_size(px(16.))
                                            .line_height(px(24.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(p.label_primary)
                                            .child(title),
                                    )
                                    .child(dock_action_button(
                                        row()
                                            .id("session-log-download-x")
                                            .role(gpui::Role::Button)
                                            .aria_label("关闭")
                                            .track_focus(header_close)
                                            .size(px(28.))
                                            .flex_none()
                                            .justify_center()
                                            .rounded(px(8.))
                                            .cursor_pointer()
                                            .hover(|style| style.bg(p.hover))
                                            .child(
                                                svg()
                                                    .data(CLOSE)
                                                    .size(px(14.))
                                                    .text_color(p.label_secondary),
                                            ),
                                        cx.listener(dismiss),
                                    )),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .px(px(24.))
                                    .text_size(px(14.))
                                    .line_height(px(22.))
                                    .text_color(p.label_primary)
                                    .child(format!("Session：{} {description}", entry.session)),
                            ),
                    )
                    .child(
                        row()
                            .w_full()
                            .px(px(24.))
                            .justify_end()
                            .child(dock_action_button(
                                row()
                                    .id("session-log-download-close")
                                    .role(gpui::Role::Button)
                                    .track_focus(footer_close)
                                    .h(px(36.))
                                    .px(px(14.))
                                    .rounded(px(12.))
                                    .bg(p.button_primary_fill)
                                    .hover(|style| style.bg(p.button_primary_hover))
                                    .cursor_pointer()
                                    .justify_center()
                                    .text_size(px(14.))
                                    .line_height(px(22.))
                                    .text_color(p.label_primary_foreground)
                                    .child("关闭"),
                                cx.listener(dismiss),
                            )),
                    ),
            );
        Some(
            deferred(modal.with_animation(
                ("session-log-download-in", entry.request),
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
