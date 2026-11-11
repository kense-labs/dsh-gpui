use super::permission_dialog::{dialog_action, dialog_header};
use super::*;

#[derive(Default)]
pub(super) struct ArchiveUi {
    pub(super) available: bool,
    pending: bool,
    confirm: Option<ArchiveRequest>,
}

struct ArchiveRequest {
    session_id: String,
    activity: Vec<Value>,
    close: gpui::FocusHandle,
    return_focus: Option<gpui::FocusHandle>,
    error: Option<String>,
}

impl DshApp {
    pub(super) fn apply_archive_snapshot(&mut self, reply: &Value) {
        if reply["available"].as_bool() != Some(true) {
            return;
        }
        self.session_archival.available = true;
        if let Some(archived) = reply["archived"].as_array() {
            self.archived_sessions = archived
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            self.pinned_sessions
                .retain(|id| !self.archived_sessions.contains(id));
        }
    }

    pub(super) fn set_session_archived(
        &mut self,
        id: String,
        archived: bool,
        stop_activity: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session_archival.pending {
            return;
        }
        if !self.connected() {
            self.push_toast("连接服务后可归档会话", cx);
            return;
        }
        let return_focus = w.focused(cx);
        self.session_archival.pending = true;
        if let Some(confirm) = &mut self.session_archival.confirm {
            confirm.error = None;
        }
        cx.spawn(async move |this, cx| {
            let result = bridge::invoke(
                "set_session_archived",
                json!({"sessionId":id,"archived":archived,"stopActivity":stop_activity}),
            )
            .await;
            let _ = this.update_in(cx, |app, w, cx| {
                app.session_archival.pending = false;
                match result {
                    Ok(reply) => {
                        if let Some(activity) = reply["activity"].as_array() {
                            let close = cx.focus_handle();
                            close.focus(w, cx);
                            app.session_archival.confirm = Some(ArchiveRequest {
                                session_id: id,
                                activity: activity.clone(),
                                close,
                                return_focus,
                                error: None,
                            });
                        } else {
                            app.apply_archive_snapshot(&reply);
                            app.save_workspace_preferences(cx);
                            app.close_archive_confirmation(w, cx);
                            app.push_toast(
                                if !archived {
                                    "已取消归档"
                                } else if stop_activity {
                                    "已停止并归档"
                                } else {
                                    "会话已归档"
                                },
                                cx,
                            );
                        }
                    }
                    Err(error) => {
                        if let Some(confirm) = &mut app.session_archival.confirm {
                            confirm.error = Some(error);
                        } else {
                            app.push_toast(error, cx);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn close_archive_confirmation(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.session_archival.pending {
            return;
        }
        if let Some(request) = self.session_archival.confirm.take()
            && let Some(focus) = request.return_focus
        {
            focus.focus(w, cx);
        }
        cx.notify();
    }

    pub(super) fn render_archive_confirmation(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        let request = self.session_archival.confirm.as_ref()?;
        let title = self
            .sessions
            .iter()
            .find(|session| session.id == request.session_id)
            .map_or("新会话", SessionSummary::title);
        let description = format!(
            "“{title}”仍有正在进行的工作。归档会先停止这些工作；之后可在侧栏筛选“全部对话（显示已归档）”中恢复会话，被停止的工作不会自动继续。"
        );
        let id = request.session_id.clone();
        let busy = self.session_archival.pending;
        let p = dsh(cx);
        Some(
            row()
                .id("archive-confirm-overlay")
                .absolute()
                .inset_0()
                .p(px(24.))
                .justify_center()
                .items_center()
                .bg(p.bg_mask_1)
                .occlude()
                .child(
                    column()
                        .id("archive-confirm-dialog")
                        .role(gpui::Role::Dialog)
                        .aria_label("停止并归档此会话？")
                        .tab_group()
                        .w(px(420.))
                        .max_w_full()
                        .rounded(px(24.))
                        .bg(p.bg_layer_2)
                        .shadow(crate::tokens::elevation_prominent())
                        .capture_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, w, cx| {
                            if event.keystroke.key == "escape" {
                                app.close_archive_confirmation(w, cx);
                                w.prevent_default();
                                cx.stop_propagation();
                            } else {
                                super::dock_controls::traverse_dock_focus(event, w, cx);
                            }
                        }))
                        .child(dialog_header(
                            "停止并归档此会话？",
                            "archive-confirm-close",
                            Some(&request.close),
                            cx,
                            |app, w, cx| app.close_archive_confirmation(w, cx),
                        ))
                        .child(
                            column()
                                .px(px(24.))
                                .gap(px(12.))
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .line_height(px(22.))
                                        .child(description),
                                )
                                .children(
                                    request
                                        .activity
                                        .iter()
                                        .map(|entry| note(activity_line(entry), cx)),
                                )
                                .children(request.error.as_ref().map(|error| note(error, cx)))
                                .when(busy, |body| body.child(note("正在停止并归档…", cx))),
                        )
                        .child(
                            row()
                                .p(px(24.))
                                .gap(px(8.))
                                .justify_end()
                                .child(dialog_action(
                                    "archive-confirm-cancel",
                                    "取消",
                                    false,
                                    busy,
                                    72.,
                                    cx,
                                    |app, w, cx| app.close_archive_confirmation(w, cx),
                                ))
                                .child(dialog_action(
                                    "archive-confirm-run",
                                    "停止并归档",
                                    false,
                                    busy,
                                    100.,
                                    cx,
                                    move |app, w, cx| {
                                        app.set_session_archived(id.clone(), true, true, w, cx)
                                    },
                                )),
                        ),
                ),
        )
    }
}

fn activity_line(entry: &Value) -> String {
    let kind = entry["kind"].as_str().unwrap_or("");
    if kind == "turn" {
        return "进行中的回合".into();
    }
    let items = entry["items"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let names = items
        .iter()
        .filter_map(|item| item["label"].as_str().or_else(|| item["id"].as_str()))
        .collect::<Vec<_>>()
        .join("、");
    let count = items.len();
    match kind {
        "subagent" => format!("{count} 个运行中的子智能体：{names}"),
        "job" => format!("{count} 个后台任务：{names}"),
        "schedule" => format!("{count} 条定时提醒：{names}"),
        _ => format!("{count} 项其他工作（{kind}）"),
    }
}
