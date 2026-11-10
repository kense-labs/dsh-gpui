//! Plan-mode controls and the Host's blocking plan review.

use super::{
    permission_dialog::dialog_action,
    workspace::{TurnActionHint, composer_bg, dark, render_markdown, strip_md_markers},
    *,
};
use gpui::svg;

const OPEN_PLAN: &str = "在侧边栏打开计划";
const PLAN_MODE_HINT: &str = "计划模式已开启 — 点击关闭（/plan off）";

use crate::submitted_plan::PlanDocument;

pub(super) enum PlanPreview {
    Loading,
    Ready(PlanDocument),
    Missing,
    Failed(String),
}

impl PlanPreview {
    pub(super) fn title(&self) -> Option<&str> {
        match self {
            Self::Ready(plan) => Some(&plan.title),
            _ => None,
        }
    }
}

pub(super) struct PlanReview {
    id: String,
    call_id: Option<String>,
    question: String,
    title: String,
    document: PlanDocument,
    summary: String,
    approval_description: Option<String>,
    preview: Option<u64>,
    auto_opened: bool,
    busy: bool,
    error: Option<String>,
}

impl PlanReview {
    fn from_update(id: &str, update: &Value) -> Option<Self> {
        let markdown = update["markdown"].as_str()?;
        let blocks = parse_markdown(markdown);
        let title = match blocks.first() {
            Some(Block::Heading(_, text)) => strip_md_markers(text),
            _ => markdown
                .trim()
                .lines()
                .next()
                .map(|line| strip_md_markers(line.trim_start_matches('#').trim()))
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| "计划".into()),
        };
        let summary = blocks
            .iter()
            .find_map(|block| match block {
                Block::Paragraph(text) => Some(strip_md_markers(text)),
                _ => None,
            })
            .filter(|summary| *summary != title)
            .unwrap_or_default();
        let call_id = update["callId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        let document = if call_id.is_some() {
            PlanDocument::from_logged_markdown(markdown.to_owned())
        } else {
            None
        }
        .unwrap_or_else(|| PlanDocument {
            title: title.clone(),
            markdown: markdown.to_owned(),
        });
        Some(Self {
            id: id.to_owned(),
            call_id,
            question: update["question"].as_str()?.to_owned(),
            title,
            document,
            summary,
            approval_description: update["approvalDescription"].as_str().map(str::to_owned),
            preview: None,
            auto_opened: false,
            busy: false,
            error: None,
        })
    }
}

#[derive(Clone, Copy)]
enum Decision {
    Discuss,
    Approve,
}

struct PlanSubmission {
    draft: String,
    attachment_ids: Vec<String>,
    prompt: Option<QueuedPrompt>,
}

impl DshApp {
    pub(super) fn handle_plan_review(
        &mut self,
        session: &str,
        update: &Value,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = update["requestId"].as_str() else {
            return;
        };
        let Some(conversation) = self.conversation_mut(session) else {
            return;
        };
        let reviews = &mut conversation.plan_reviews;
        match update["phase"].as_str() {
            Some("open") if !reviews.iter().any(|review| review.id == id) => {
                if let Some(review) = PlanReview::from_update(id, update) {
                    reviews.push(review);
                }
            }
            Some("closed") => reviews.retain(|review| review.id != id),
            _ => return,
        }
        if self.active_session.as_deref() == Some(session)
            && self.page == WorkspacePage::Conversation
        {
            if self.conversation.plan_reviews.is_empty()
                && self.conversation.permission.is_none()
                && update["phase"] == "closed"
                && self.focused_right_pane(w, cx).is_none()
            {
                self.focus_composer(w, cx);
            }
            self.maybe_follow_tail();
        }
    }

    pub(super) fn auto_open_plan_preview(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.show_welcome
            && self.page == WorkspacePage::Conversation
            && self.conversation.permission.is_none()
            && self
                .conversation
                .plan_reviews
                .last()
                .is_some_and(|review| !review.auto_opened)
        {
            self.open_plan_preview(false, w, cx);
        }
    }

    fn open_plan_preview(&mut self, focus: bool, w: &mut Window, cx: &mut Context<Self>) {
        let Some(review) = self.conversation.plan_reviews.last_mut() else {
            return;
        };
        review.auto_opened = true;
        let call_id = review.call_id.clone();
        let document = review.document.clone();
        let temporary_key = review.preview;
        let key = self.mount_plan_preview(call_id.as_deref(), temporary_key, document);
        self.conversation.plan_reviews.last_mut().unwrap().preview = Some(key);
        let layout = &mut self.sidebar.surface.layout;
        let tab = RightPanelTab::Plan(key);
        let pane = layout.focused();
        if focus {
            self.open_right_tab(pane, tab, w, cx);
        } else {
            layout.open(pane, tab);
            self.sidebar.surface.open |= !layout.focused().is_floating();
            cx.notify();
        }
    }

    fn mount_plan_preview(
        &mut self,
        call_id: Option<&str>,
        temporary_key: Option<u64>,
        document: PlanDocument,
    ) -> u64 {
        let saved = call_id.and_then(|call_id| {
            self.sidebar
                .surface
                .plans
                .iter()
                .find_map(|(&key, stored)| (stored == call_id).then_some(key))
        });
        let key = saved
            .or_else(|| temporary_key.filter(|key| self.sidebar.plans.contains_key(key)))
            .unwrap_or_else(|| {
                self.next_file_request += 1;
                let key = (self.next_file_request..)
                    .find(|id| {
                        !self.sidebar.surface.plans.contains_key(id)
                            && !self.sidebar.plans.contains_key(id)
                    })
                    .expect("there are unclaimed plan keys");
                self.next_file_request = key;
                key
            });
        if let Some(call_id) = call_id {
            self.sidebar
                .surface
                .plans
                .entry(key)
                .or_insert_with(|| call_id.to_owned());
        }
        self.sidebar.plans.insert(key, PlanPreview::Ready(document));
        key
    }

    fn open_submitted_plan(&mut self, call_id: &str, w: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.conversation.entries.iter().find(|entry| {
            entry.role == EntryRole::Plan && entry.external_id.as_deref() == Some(call_id)
        }) else {
            return;
        };
        let document = PlanDocument {
            title: entry.title.clone(),
            markdown: entry.text.clone(),
        };
        let key = self.mount_plan_preview(Some(call_id), None, document);
        self.open_right_tab(
            self.sidebar.surface.layout.focused(),
            RightPanelTab::Plan(key),
            w,
            cx,
        );
    }

    pub(super) fn sync_plan_previews(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.active_session.clone() else {
            return;
        };
        let reads: Vec<_> = self
            .sidebar
            .surface
            .layout
            .all_panes()
            .filter(|pane| self.dock_pane_on_screen(pane.id, cx))
            .flat_map(|pane| pane.tabs().iter())
            .filter_map(|tab| {
                let RightPanelTab::Plan(key) = *tab else {
                    return None;
                };
                if self.sidebar.plans.contains_key(&key) {
                    return None;
                }
                Some((key, self.sidebar.surface.plans.get(&key)?.clone()))
            })
            .collect();
        if reads.is_empty() {
            return;
        }
        for (key, _) in &reads {
            self.sidebar.plans.insert(*key, PlanPreview::Loading);
        }
        let cwd = self.sidebar.workspace.clone();
        cx.spawn(async move |this, cx| {
            let ids: Vec<_> = reads.iter().map(|(_, id)| id).collect();
            let mut result = bridge::invoke(
                "read_session_plans",
                json!({"sessionId": session, "cwd": cwd, "callIds": ids}),
            )
            .await
            .and_then(|value| {
                serde_json::from_value::<HashMap<String, PlanDocument>>(value)
                    .map_err(|error| error.to_string())
            });
            let _ = this.update(cx, |app, cx| {
                let Some(sidebar) = app.sidebar_for_session(&session) else {
                    return;
                };
                for (key, call_id) in reads {
                    if sidebar.surface.plans.get(&key) == Some(&call_id)
                        && let Some(preview @ PlanPreview::Loading) = sidebar.plans.get_mut(&key)
                    {
                        *preview = match &mut result {
                            Ok(plans) => plans
                                .remove(&call_id)
                                .map_or(PlanPreview::Missing, PlanPreview::Ready),
                            Err(error) => PlanPreview::Failed(error.clone()),
                        };
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn decide_plan(&mut self, decision: Decision, cx: &mut Context<Self>) {
        let (Some(session), Some(review)) = (
            self.active_session.clone(),
            self.conversation.plan_reviews.last_mut(),
        ) else {
            return;
        };
        if review.busy {
            return;
        }
        review.busy = true;
        review.error = None;
        let id = review.id.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let decision = match decision {
                Decision::Discuss => "discuss",
                Decision::Approve => "approve",
            };
            let result = bridge::invoke(
                "respond_plan_review",
                json!({"sessionId": session, "requestId": id, "decision": decision}),
            )
            .await;
            if let Err(error) = result {
                let _ = this.update(cx, |this, cx| {
                    if let Some(review) = this.conversation_mut(&session).and_then(|conversation| {
                        conversation
                            .plan_reviews
                            .iter_mut()
                            .find(|review| review.id == id)
                    }) {
                        review.busy = false;
                        review.error = Some(error);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    pub(super) fn submit_plan_command(&mut self, w: &mut Window, cx: &mut Context<Self>) -> bool {
        let draft = self.composer.read(cx).value().to_owned();
        let command = draft.trim();
        let end = command.find(char::is_whitespace).unwrap_or(command.len());
        if !matches!(&command[..end], "/plan" | "/计划") {
            return false;
        }
        let message = command[end..].trim();
        let active = message != "off";
        let attachments = &self.conversation.draft_attachments;
        if !active && !attachments.is_empty() {
            self.conversation.error = Some("/plan off 不支持附加文件。".to_owned());
            cx.notify();
            return true;
        }
        let prompt = (active && (!message.is_empty() || !attachments.is_empty())).then(|| {
            let (display, prompt) = build_prompt_payload(message, attachments);
            QueuedPrompt { display, prompt }
        });
        let attachment_ids = attachments.iter().map(|item| item.id.clone()).collect();
        self.set_plan_mode(
            active,
            Some(PlanSubmission {
                draft,
                attachment_ids,
                prompt,
            }),
            w,
            cx,
        );
        true
    }

    fn set_plan_mode(
        &mut self,
        active: bool,
        command: Option<PlanSubmission>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.active_session.clone() else {
            return;
        };
        if !self.connected() || !self.conversation.acp_open || self.conversation.plan_mode_busy {
            return;
        }
        self.conversation.plan_mode_busy = true;
        cx.notify();
        cx.spawn_in(w, async move |this, cx| {
            let result =
                bridge::invoke("plan_mode", json!({"sessionId": session, "active": active})).await;
            let _ = this.update_in(cx, |this, w, cx| {
                let selected = this.active_session.as_deref() == Some(&session);
                let Some(conversation) = this.conversation_mut(&session) else {
                    return;
                };
                if !std::mem::take(&mut conversation.plan_mode_busy) || !conversation.acp_open {
                    return;
                }
                match result {
                    Ok(value) => conversation.plan_active = value["active"].as_bool(),
                    Err(error) => {
                        conversation.error = Some(error);
                        cx.notify();
                        return;
                    }
                }
                conversation.error = None;
                if let Some(PlanSubmission {
                    draft,
                    attachment_ids,
                    prompt,
                }) = command
                {
                    conversation
                        .draft_attachments
                        .retain(|item| !attachment_ids.contains(&item.id));
                    if !selected && conversation.draft == draft {
                        conversation.draft.clear();
                    }
                    if selected && this.composer.read(cx).value() == draft {
                        this.replace_composer_without_trigger("", w, cx);
                    }
                    if let Some(prompt) = prompt {
                        this.continue_plan_prompt(&session, prompt, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn continue_plan_prompt(
        &mut self,
        session: &str,
        prompt: QueuedPrompt,
        cx: &mut Context<Self>,
    ) {
        let Some(conversation) = self.conversation_mut(session) else {
            return;
        };
        if conversation.running {
            conversation.queued_prompts.push(prompt);
        } else {
            if let Some(row) = self.sessions.iter_mut().find(|row| row.id == session) {
                row.title
                    .get_or_insert_with(|| short_session_title(&prompt.display));
                row.blank = false;
            }
            self.start_prompt(session, prompt, cx);
        }
    }

    pub(super) fn plan_chip(&self, cx: &mut Context<Self>) -> Option<gpui::Stateful<gpui::Div>> {
        if self.conversation.plan_active != Some(true) {
            return None;
        }
        let p = dsh(cx);
        let locked = !self.connected() || self.conversation.plan_mode_busy;
        let group = "plan-mode-chip";
        let chip = row()
            .id("plan-mode-chip")
            .role(gpui::Role::Button)
            .aria_label("计划模式已开启，按下关闭")
            .tab_index(0)
            .tab_stop(!locked)
            .group(group)
            .flex_none()
            .h(px(28.))
            .min_w(px(34.))
            .px(px(8.))
            .gap(px(4.))
            .rounded(px(8.))
            .bg(p.state_business_tertiary)
            .text_color(p.state_business_primary)
            .text_size(px(13.))
            .line_height(px(20.))
            .font_weight(FontWeight::MEDIUM)
            .tooltip(|_, cx| cx.new(|_| TurnActionHint(PLAN_MODE_HINT.into())).into())
            .child(
                div()
                    .relative()
                    .flex_none()
                    .size(px(14.))
                    .child(
                        plan_icon(
                            include_bytes!("../../assets/plan-mode.svg"),
                            p.state_business_primary,
                        )
                        .absolute()
                        .inset_0()
                        .when(!locked, |icon| {
                            icon.group_hover(group, |style| style.opacity(0.))
                        }),
                    )
                    .when(!locked, |glyph| {
                        glyph.child(
                            plan_icon(
                                include_bytes!("../../assets/plan-mode-close.svg"),
                                p.state_business_primary,
                            )
                            .absolute()
                            .inset_0()
                            .opacity(0.)
                            .group_hover(group, |style| style.opacity(1.)),
                        )
                    }),
            )
            .child("计划");
        Some(if locked {
            chip.opacity(0.6)
        } else {
            dock_action_button(
                chip.cursor_pointer().hover(move |style| {
                    style.bg(p
                        .state_business_tertiary
                        .blend(p.state_business_primary.opacity(0.06)))
                }),
                cx.listener(|this, _, w, cx| this.set_plan_mode(false, None, w, cx)),
            )
        })
    }

    pub(super) fn render_plan_review(
        &self,
        width: f32,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let review = self.conversation.plan_reviews.last()?;
        let p = dsh(cx);
        let compact = f32::from(w.viewport_size().width) <= 720.;
        let approve = dialog_action(
            "approve-plan",
            "同意执行",
            true,
            review.busy,
            0.,
            cx,
            |this, _, cx| this.decide_plan(Decision::Approve, cx),
        )
        .when_some(
            review.approval_description.clone(),
            |button, description| {
                button.tooltip(move |_, cx| {
                    cx.new(|_| TurnActionHint(description.clone().into()))
                        .into()
                })
            },
        );
        let discuss = dialog_action(
            "discuss-plan",
            "要求修改",
            false,
            review.busy,
            0.,
            cx,
            |this, _, cx| this.decide_plan(Decision::Discuss, cx),
        )
        .flex_row_reverse()
        .gap(px(6.))
        .child(plan_icon(
            include_bytes!("../../assets/plan-review-edit.svg"),
            p.label_primary,
        ));
        let preview = dock_action_button(
            row()
                .id("open-plan-preview")
                .role(gpui::Role::Button)
                .aria_label(OPEN_PLAN)
                .tab_index(0)
                .gap(px(4.))
                .cursor_pointer()
                .text_color(p.label_secondary)
                .hover(|style| style.text_color(p.label_primary))
                .tooltip(|_, cx| cx.new(|_| TurnActionHint(OPEN_PLAN.into())).into())
                .child("查看全文")
                .child(plan_icon(
                    include_bytes!("../../assets/header-chevron-right.svg"),
                    p.label_secondary,
                )),
            cx.listener(|this, _, w, cx| this.open_plan_preview(true, w, cx)),
        );
        let dot = if review.busy {
            crate::anim::state_spinner(10., 0., p.label_tertiary).into_any_element()
        } else {
            div()
                .size(px(6.))
                .rounded_full()
                .bg(p.state_warn_primary)
                .into_any_element()
        };
        Some(
            row()
                .w_full()
                .flex_none()
                .px(px(16.))
                .pt(px(6.))
                .pb(px(10.))
                .justify_center()
                .child(
                    column()
                        .id("plan-review")
                        .role(gpui::Role::Group)
                        .aria_label(review.question.clone())
                        .w(px((width - 32.).max(0.)))
                        .max_w_full()
                        .gap_0()
                        .overflow_hidden()
                        .rounded(px(20.))
                        .bg(composer_bg(cx))
                        .shadow(crate::tokens::elevation_panel())
                        .child(
                            row()
                                .w_full()
                                .flex_none()
                                .gap(px(8.))
                                .px(px(16.))
                                .py(px(12.))
                                .rounded_t(px(20.))
                                .bg(p.state_warn_tertiary)
                                .text_color(p.state_warn_primary)
                                .text_size(px(14.))
                                .line_height(px(20.))
                                .child(row().size(px(10.)).justify_center().child(dot))
                                .child("计划待审")
                                .child(div().flex_1())
                                .child(preview),
                        )
                        .child(
                            column()
                                .w_full()
                                .min_w_0()
                                .gap(px(8.))
                                .px(px(16.))
                                .pt(px(14.))
                                .pb(px(12.))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(15.))
                                        .line_height(px(22.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(review.title.clone()),
                                )
                                .when(!review.summary.is_empty(), |summary| {
                                    summary.child(
                                        div()
                                            .max_h(px(48.))
                                            .overflow_hidden()
                                            .text_size(px(14.))
                                            .line_height(px(24.))
                                            .text_color(p.label_secondary)
                                            .child(review.summary.clone()),
                                    )
                                }),
                        )
                        .child(
                            row()
                                .w_full()
                                .gap(px(12.))
                                .px(px(if compact { 12. } else { 16. }))
                                .pt(px(8.))
                                .pb(px(if compact { 10. } else { 12. }))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .min_h(px(16.))
                                        .text_color(p.danger)
                                        .text_size(px(11.))
                                        .line_height(px(16.))
                                        .children(review.error.clone()),
                                )
                                .child(row().flex_none().gap(px(8.)).child(discuss).child(approve)),
                        ),
                ),
        )
    }

    pub(super) fn render_plan_document(
        &self,
        key: u64,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let plan = match self.sidebar.plans.get(&key) {
            Some(PlanPreview::Ready(plan)) => plan,
            state => {
                let p = dsh(cx);
                let mut message = column()
                    .p(px(24.))
                    .text_size(px(14.))
                    .text_color(p.label_secondary);
                message = match state {
                    Some(PlanPreview::Failed(error)) => {
                        message.child("无法读取计划").child(error.clone())
                    }
                    Some(PlanPreview::Missing) => {
                        message.child("无法读取计划").child("未找到这份计划")
                    }
                    _ if self.sidebar.surface.plans.contains_key(&key) => {
                        message.child("正在读取计划…")
                    }
                    _ => message.child("计划预览不可用"),
                };
                return message;
            }
        };
        let compact = f32::from(w.viewport_size().width) <= 767.;
        div().size_full().child(
            div()
                .id(gpui::ElementId::Name(format!("plan-preview-{key}").into()))
                .size_full()
                .overflow_y_scroll()
                .px(px(if compact { 18. } else { 24. }))
                .pt(px(if compact { 16. } else { 20. }))
                .pb(px(if compact { 32. } else { 40. }))
                .child(render_markdown(
                    &plan.markdown,
                    &format!("plan-{key}"),
                    self.copied_mark.as_deref(),
                    &self.conversation.wrapped_code,
                    0,
                    None,
                    cx,
                )),
        )
    }
}

pub(super) fn plan_card(entry: &ChatEntry, cx: &mut Context<DshApp>) -> gpui::Div {
    let Some(call_id) = entry.external_id.clone() else {
        return div();
    };
    let p = dsh(cx);
    let dark = dark(cx);
    let fill = gpui::rgb(if dark { 0x212123 } else { 0xfafafa });
    let hover = gpui::rgb(if dark { 0x292929 } else { 0xf5f5f5 });
    div().w_full().child(dock_action_button(
        row()
            .id(gpui::ElementId::Name(entry.key.clone().into()))
            .role(gpui::Role::Button)
            .aria_label(format!("打开计划：{}", entry.title))
            .tab_index(0)
            .cursor_pointer()
            .w_full()
            .h(px(60.))
            .px(px(10.))
            .gap(px(10.))
            .rounded(px(16.))
            .border(px(0.5))
            .border_color(p.border_l1)
            .bg(fill)
            .hover(move |style| style.bg(hover))
            .child(
                row()
                    .size(px(40.))
                    .flex_none()
                    .justify_center()
                    .rounded(px(10.))
                    .border(px(0.5))
                    .border_color(p.border_l1)
                    .bg(gpui::white().opacity(if dark { 0.05 } else { 0.5 }))
                    .child(markdown_file_icon()),
            )
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .child(entry.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_color(p.label_tertiary)
                            .text_size(px(10.))
                            .line_height(px(16.))
                            .child("计划 · Markdown"),
                    ),
            )
            .child(
                row()
                    .flex_none()
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(8.))
                    .border(px(0.5))
                    .border_color(p.border_l3)
                    .bg(p.button_floating)
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .child("打开"),
            ),
        cx.listener(move |this, _, w, cx| this.open_submitted_plan(&call_id, w, cx)),
    ))
}

fn markdown_file_icon() -> gpui::Div {
    div()
        .relative()
        .size(px(20.))
        .child(
            svg()
                .data(include_bytes!("../../assets/files-other.svg"))
                .size_full()
                .text_color(gpui::rgb(0x4176e6)),
        )
        .child(
            svg()
                .data(include_bytes!("../../assets/files-other-fold.svg"))
                .absolute()
                .inset_0()
                .text_color(gpui::white().opacity(0.7)),
        )
        .child(
            svg()
                .data(include_bytes!("../../assets/file-markdown-mark.svg"))
                .absolute()
                .inset_0()
                .text_color(gpui::white()),
        )
}

fn plan_icon(artwork: &'static [u8], color: gpui::Hsla) -> gpui::Svg {
    svg()
        .data(artwork)
        .size(px(14.))
        .flex_none()
        .text_color(color)
}
