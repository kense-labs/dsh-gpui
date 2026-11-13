//! GPUI question composer and the Host's foreground and continued requests.

use super::{
    dock_controls::{dock_action_button, traverse_dock_focus},
    permission_dialog::dialog_action,
    workspace::{TurnActionHint, composer_bg, render_markdown},
    *,
};
use crate::questions::{Clock, Draft, Question, RecordedAnswer};
use gpui::{Entity, FocusHandle, Subscription, svg};
use std::{collections::HashSet, time::Duration};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Foreground,
    Continuing,
    Continued,
    Review,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Decision {
    Answer,
    Cancel,
    Timeout,
}

pub(super) struct PendingQuestions {
    id: String,
    pub(super) call_id: Option<String>,
    questions: Vec<Question>,
    drafts: Vec<Draft>,
    editors: Vec<Entity<Editor>>,
    _edits: Vec<Subscription>,
    body_focus: FocusHandle,
    scroll: ScrollHandle,
    autofocus: HashSet<usize>,
    expanded_copy: HashSet<CopyPart>,
    index: usize,
    pub(super) hidden: bool,
    minimized: bool,
    stage: Stage,
    clock: Option<Clock>,
    busy: Option<Decision>,
    /// 官方：超时放行 agent 后卡片仍可回答，不用 busy 锁住控件。
    timed_out: bool,
    error: Option<String>,
}

impl PendingQuestions {
    fn new(
        session: &str,
        id: &str,
        update: &Value,
        stage: Stage,
        w: &mut Window,
        cx: &mut Context<DshApp>,
    ) -> Option<Self> {
        let questions: Vec<Question> = serde_json::from_value(update["questions"].clone()).ok()?;
        if questions.is_empty() {
            return None;
        }
        let drafts = questions.iter().map(Draft::new).collect();
        let mut editors = Vec::with_capacity(questions.len());
        let mut edits = Vec::with_capacity(questions.len());
        for index in 0..questions.len() {
            let input = editor("输入你的答案".into(), InputMode::InlineMulti, w, cx);
            input.update(cx, |input, _| input.font_size = 14.);
            let (session, id) = (session.to_owned(), id.to_owned());
            edits.push(
                cx.subscribe_in(
                    &input,
                    w,
                    move |app, input, event: &EditEvent, w, cx| match event {
                        EditEvent::Changed => {
                            let value = input.read(cx).value();
                            if let Some(panel) = app.question_mut(&session, &id)
                                && panel.busy.is_none()
                                && value != panel.drafts[index].custom
                            {
                                panel.drafts[index].type_answer(
                                    value.to_owned(),
                                    panel.questions[index].multi_select,
                                );
                                panel.engage();
                                panel.error = None;
                                cx.notify();
                            }
                        }
                        EditEvent::Submitted
                            if app.active_session.as_deref() == Some(session.as_str())
                                && app.conversation.visible_question().is_some_and(|panel| {
                                    panel.id == id && panel.index == index && !panel.minimized
                                }) =>
                        {
                            app.question_action(Action::Continue, w, cx);
                        }
                        _ => {}
                    },
                ),
            );
            editors.push(input);
        }
        Some(Self {
            id: id.to_owned(),
            call_id: update["callId"].as_str().map(str::to_owned),
            questions,
            drafts,
            editors,
            _edits: edits,
            body_focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            autofocus: HashSet::new(),
            expanded_copy: HashSet::new(),
            index: 0,
            hidden: stage == Stage::Continued,
            minimized: false,
            stage,
            clock: update["remainingMs"]
                .as_f64()
                .map(|remaining| Clock::Counting {
                    deadline: bridge::monotonic_ms() + remaining.max(0.),
                }),
            busy: None,
            timed_out: false,
            error: None,
        })
    }

    fn engage(&mut self) {
        if self.clock.is_some() {
            self.clock = Some(Clock::Held);
        }
    }

    fn navigate(&mut self, index: usize) {
        if self.index != index {
            self.expanded_copy.clear();
            self.scroll.set_offset(gpui::point(px(0.), px(0.)));
        }
        self.index = index;
        self.error = None;
    }

    fn locked(&self) -> bool {
        self.stage == Stage::Continuing || self.busy.is_some()
    }

    pub(super) fn is_pending(&self) -> bool {
        self.stage != Stage::Review
    }
}

impl Conversation {
    pub(super) fn visible_question(&self) -> Option<&PendingQuestions> {
        self.questions.iter().rev().find(|panel| !panel.hidden)
    }
}

#[derive(Clone, Copy)]
enum Action {
    Toggle,
    Close,
    Hold,
    Previous,
    Next,
    Choose(usize),
    ToggleCopy(CopyPart),
    Submit,
    Continue,
    Skip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum CopyPart {
    Title,
    Description(usize),
}

impl DshApp {
    fn question_mut(&mut self, session: &str, id: &str) -> Option<&mut PendingQuestions> {
        self.conversation_mut(session)?
            .questions
            .iter_mut()
            .find(|panel| panel.id == id)
    }

    pub(super) fn handle_questions(
        &mut self,
        session: &str,
        update: &Value,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_showing = self.active_session.as_deref() == Some(session)
            && self.conversation.visible_question().is_some();
        match update["phase"].as_str() {
            Some("open") => {
                let Some(id) = update["requestId"].as_str() else {
                    return;
                };
                if self.question_mut(session, id).is_none()
                    && let Some(panel) =
                        PendingQuestions::new(session, id, update, Stage::Foreground, w, cx)
                    && let Some(conversation) = self.conversation_mut(session)
                {
                    let timed = panel.clock.is_some();
                    conversation.questions.push(panel);
                    if timed {
                        self.tick_question(session.to_owned(), id.to_owned(), w, cx);
                    }
                }
            }
            Some("closed") => {
                let Some(id) = update["requestId"].as_str() else {
                    return;
                };
                if let Some(panel) = self.question_mut(session, id)
                    && update["willContinue"] == true
                {
                    // Projection and close can arrive in either order.
                    if panel.stage == Stage::Foreground {
                        panel.stage = Stage::Continuing;
                    }
                    panel.clock = None;
                } else if let Some(conversation) = self.conversation_mut(session) {
                    conversation.questions.retain(|panel| panel.id != id);
                }
            }
            Some("continued") => {
                let Some(rows) = update["rows"].as_array() else {
                    return;
                };
                let Some(conversation) = self.conversation_mut(session) else {
                    return;
                };
                conversation.settled_questions = question_records(&update["settled"]);
                conversation.queued_question_replies = question_records(&update["queued"]);
                conversation.questions.retain(|panel| {
                    panel.stage != Stage::Continued
                        || rows.iter().any(|row| row["callId"] == panel.id)
                });
                for row in rows {
                    let Some(id) = row["callId"].as_str() else {
                        continue;
                    };
                    if let Some(panel) = conversation
                        .questions
                        .iter_mut()
                        .find(|panel| panel.id == id)
                    {
                        if panel.stage != Stage::Continued {
                            panel.error = (panel.busy == Some(Decision::Answer))
                                .then(|| "回答未送达，工作已继续，请再提交一次。".into());
                            panel.stage = Stage::Continued;
                            panel.clock = None;
                            panel.busy = None;
                        }
                    } else if let Some(panel) =
                        PendingQuestions::new(session, id, row, Stage::Continued, w, cx)
                    {
                        conversation.questions.push(panel);
                    }
                }
            }
            _ => return,
        }
        if self.active_session.as_deref() == Some(session)
            && self.page == WorkspacePage::Conversation
        {
            if was_showing {
                self.focus_after_questions(session, w, cx);
            }
            self.maybe_follow_tail();
        }
        cx.notify();
    }

    /// Rehydrate the pending questions after a cold resume.
    ///
    /// `question_state` returns the bridge's `rows` projection directly, without
    /// the `settled` / `queued` snapshots carried by the `continued` event. Keep
    /// those snapshots intact and only reconcile the live panels here.
    pub(super) fn apply_question_state(
        &mut self,
        session: &str,
        rows: &[Value],
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids: HashSet<&str> = rows
            .iter()
            .filter_map(|row| row["callId"].as_str())
            .collect();
        if let Some(conversation) = self.conversation_mut(session) {
            conversation
                .questions
                .retain(|panel| panel.stage != Stage::Continued || ids.contains(panel.id.as_str()));
        }
        for row in rows {
            let Some(id) = row["callId"].as_str() else {
                continue;
            };
            if self.question_mut(session, id).is_some() {
                continue;
            }
            let Some(panel) = PendingQuestions::new(session, id, row, Stage::Continued, w, cx)
            else {
                continue;
            };
            if let Some(conversation) = self.conversation_mut(session) {
                conversation.questions.push(panel);
            }
        }
    }

    fn tick_question(
        &mut self,
        session: String,
        id: String,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(w, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let keep_going = this.update_in(cx, |app, w, cx| {
                    let visible = app.active_session.as_deref() == Some(session.as_str())
                        && app.page == WorkspacePage::Conversation
                        && !app.show_settings
                        && app.conversation.permission.is_none()
                        && app.conversation.plan_reviews.is_empty()
                        && app
                            .conversation
                            .visible_question()
                            .is_some_and(|panel| panel.id == id);
                    let Some(panel) = app.question_mut(&session, &id) else {
                        return false;
                    };
                    let Some(clock) = panel.clock else {
                        return false;
                    };
                    if clock == Clock::Held {
                        return false;
                    }
                    let now = bridge::monotonic_ms();
                    let holding = visible
                        && !panel.minimized
                        && w.is_window_active()
                        && panel.body_focus.contains_focused(w, cx);
                    let clock = clock.step(now, holding);
                    panel.clock = Some(clock);
                    if panel.busy.is_none() && clock.remaining(now) == Some(0.) {
                        app.send_question(&session, &id, Decision::Timeout, w, cx);
                    }
                    if visible {
                        cx.notify();
                    }
                    true
                });
                if !matches!(keep_going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn send_question(
        &mut self,
        session: &str,
        id: &str,
        decision: Decision,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.question_mut(session, id) else {
            return;
        };
        if panel.locked() || panel.stage == Stage::Review {
            return;
        }
        let stage = panel.stage;
        let answers = (decision == Decision::Answer).then(|| {
            panel
                .drafts
                .iter()
                .zip(&panel.questions)
                .map(|(draft, question)| draft.payload(question))
                .collect::<Vec<_>>()
        });
        // 官方：到期放行 agent，但卡片保持可用，用户仍可提交答案。
        if decision == Decision::Timeout {
            panel.timed_out = true;
            panel.clock = None;
        } else {
            panel.busy = Some(decision);
        }
        panel.error = None;
        let (session, id) = (session.to_owned(), id.to_owned());
        let args = json!({
            "sessionId": session, "requestId": id, "decision": match decision {
                Decision::Answer => "answer",
                Decision::Cancel => "cancel",
                Decision::Timeout => "timeout",
            },
            "answers": answers,
        });
        cx.spawn_in(w, async move |this, cx| {
            let result = bridge::invoke("respond_question", args).await;
            let _ = this.update_in(cx, |app, w, cx| {
                let Some(conversation) = app.conversation_mut(&session) else {
                    return;
                };
                let Some(panel) = conversation
                    .questions
                    .iter_mut()
                    .find(|panel| panel.id == id)
                else {
                    return;
                };
                if panel.stage != stage
                    || (decision != Decision::Timeout && panel.busy != Some(decision))
                {
                    return;
                }
                match result {
                    Ok(_) if stage == Stage::Continued => {
                        conversation.questions.retain(|panel| panel.id != id);
                        app.focus_after_questions(&session, w, cx);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        panel.busy = None;
                        panel.error = Some(error);
                        if decision == Decision::Timeout {
                            panel.engage();
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn focus_after_questions(&mut self, session: &str, w: &mut Window, cx: &mut Context<Self>) {
        if self.active_session.as_deref() == Some(session)
            && self.page == WorkspacePage::Conversation
            && !self.show_settings
            && self.conversation.permission.is_none()
            && self.conversation.plan_reviews.is_empty()
            && self.conversation.visible_question().is_none()
            && self.focused_right_pane(w, cx).is_none()
        {
            self.focus_composer(w, cx);
        }
    }

    fn question_action(&mut self, action: Action, w: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.active_session.clone() else {
            return;
        };
        let Some(panel) = self
            .conversation
            .questions
            .iter_mut()
            .rev()
            .find(|panel| !panel.hidden)
        else {
            return;
        };
        if let Action::ToggleCopy(copy) = action {
            if !panel.expanded_copy.remove(&copy) {
                panel.expanded_copy.insert(copy);
            }
            cx.notify();
            return;
        }
        if panel.locked() {
            return;
        }
        if panel.stage == Stage::Review
            && !matches!(
                action,
                Action::Toggle | Action::Close | Action::Previous | Action::Next
            )
        {
            return;
        }
        let index = panel.index;
        let last = index + 1 == panel.questions.len();
        match action {
            Action::Toggle => panel.minimized = !panel.minimized,
            Action::Close => {
                if panel.call_id.is_none() {
                    let id = panel.id.clone();
                    self.send_question(&session, &id, Decision::Cancel, w, cx);
                    return;
                }
                panel.hidden = true;
                self.focus_after_questions(&session, w, cx);
            }
            Action::Hold => panel.engage(),
            Action::Previous if index > 0 => panel.navigate(index - 1),
            Action::Next if !last => panel.navigate(index + 1),
            Action::Choose(choice) => {
                let question = &panel.questions[index];
                if let Some(choice) = question.options.get(choice) {
                    let multi = question.multi_select;
                    panel.drafts[index].choose(choice.label.clone(), multi);
                    if !multi {
                        replace(&panel.editors[index], "", w, cx);
                        if !last {
                            panel.navigate(index + 1);
                        }
                    }
                    panel.engage();
                    panel.error = None;
                }
            }
            Action::Continue | Action::Skip | Action::Submit => {
                if matches!(action, Action::Skip) {
                    panel.drafts[index].skip();
                    replace(&panel.editors[index], "", w, cx);
                    panel.engage();
                } else if matches!(action, Action::Continue) && !panel.drafts[index].answered() {
                    panel.error = Some("请选择一个选项或填写自定义答案。".into());
                    cx.notify();
                    return;
                }
                if !last && !matches!(action, Action::Submit) {
                    panel.navigate(index + 1);
                } else if let Some(missing) = panel
                    .drafts
                    .iter()
                    .position(|draft| !draft.skipped && !draft.answered())
                {
                    panel.navigate(missing);
                    panel.error = Some("请先完成这道问题。".into());
                } else {
                    let id = panel.id.clone();
                    self.send_question(&session, &id, Decision::Answer, w, cx);
                    return;
                }
            }
            _ => return,
        }
        cx.notify();
    }

    pub(super) fn reopen_question(&mut self, call_id: &str, cx: &mut Context<Self>) {
        let panels = &mut self.conversation.questions;
        if let Some(index) = panels
            .iter()
            .position(|panel| panel.is_pending() && panel.call_id.as_deref() == Some(call_id))
        {
            let mut panel = panels.remove(index);
            if panel.hidden {
                panel.expanded_copy.clear();
                panel.scroll.set_offset(gpui::point(px(0.), px(0.)));
            }
            panel.hidden = false;
            panel.minimized = false;
            panels.push(panel);
            cx.notify();
        }
    }

    fn review_question(&mut self, call_id: &str, w: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.active_session.as_deref() else {
            return;
        };
        let Some(answers) = self.conversation.settled_questions.get(call_id) else {
            return;
        };
        let Some(arguments) = self
            .conversation
            .entries
            .iter()
            .find(|entry| {
                entry.role == EntryRole::Tool && entry.external_id.as_deref() == Some(call_id)
            })
            .and_then(|entry| serde_json::from_str::<Value>(&entry.input).ok())
        else {
            return;
        };
        let id = format!("review-{call_id}");
        let mut update = json!({"callId":call_id});
        update["questions"] = arguments["questions"].clone();
        let Some(mut panel) = PendingQuestions::new(session, &id, &update, Stage::Review, w, cx)
        else {
            return;
        };
        let Some(paired) = pair_recorded_answers(&panel.questions, answers) else {
            return;
        };
        panel.drafts = paired
            .into_iter()
            .map(|answer| {
                let custom = answer.custom.clone().unwrap_or_default();
                Draft {
                    skipped: answer.selected.is_empty() && custom.is_empty(),
                    selected: answer.selected.clone(),
                    custom,
                }
            })
            .collect();
        for (editor, draft) in panel.editors.iter().zip(&panel.drafts) {
            replace(editor, draft.custom.clone(), w, cx);
        }
        let panels = &mut self.conversation.questions;
        if let Some(index) = panels.iter().position(|panel| panel.id == id) {
            panel.index = panels.remove(index).index.min(panel.questions.len() - 1);
        }
        panels.push(panel);
        cx.notify();
    }
}

fn question_records(rows: &Value) -> HashMap<String, Vec<RecordedAnswer>> {
    rows.as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let id = row["callId"].as_str()?.to_owned();
            Some((id, serde_json::from_value(row["answers"].clone()).ok()?))
        })
        .collect()
}

fn pair_recorded_answers<'a>(
    questions: &[Question],
    answers: &'a [RecordedAnswer],
) -> Option<Vec<&'a RecordedAnswer>> {
    if questions.is_empty() || questions.len() != answers.len() {
        return None;
    }
    let mut by_id: HashMap<_, _> = answers
        .iter()
        .map(|answer| (answer.id.as_str(), answer))
        .collect();
    questions
        .iter()
        .map(|question| by_id.remove(question.id.as_str()))
        .collect()
}

const DOWN: &[u8] = include_bytes!("../../assets/question-chevron-down.svg");
const UP: &[u8] = include_bytes!("../../assets/question-chevron-up.svg");
const LEFT: &[u8] = include_bytes!("../../assets/question-chevron-left.svg");
const RIGHT: &[u8] = include_bytes!("../../assets/header-chevron-right.svg");
const CLOSE: &[u8] = include_bytes!("../../assets/question-close.svg");
const CHECK: &[u8] = include_bytes!("../../assets/question-check.svg");
const QUESTION: &[u8] = include_bytes!("../../assets/question-question.svg");
const EDIT: &[u8] = include_bytes!("../../assets/plan-review-edit.svg");

fn question_icon(bytes: &'static [u8], size: f32, color: gpui::Hsla) -> gpui::Svg {
    svg()
        .data(bytes)
        .size(px(size))
        .flex_none()
        .text_color(color)
}

fn icon_action(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    artwork: &'static [u8],
    disabled: bool,
    action: Action,
    cx: &mut Context<DshApp>,
) -> gpui::Stateful<gpui::Div> {
    let p = dsh(cx);
    let mut button = row()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .tab_index(0)
        .tab_stop(!disabled)
        .size(px(24.))
        .flex_none()
        .justify_center()
        .rounded_full()
        .child(question_icon(
            artwork,
            16.,
            if disabled {
                p.label_dimmed
            } else {
                p.label_tertiary
            },
        ));
    if !disabled {
        button = dock_action_button(
            button.cursor_pointer().hover(|style| style.bg(p.hover)),
            cx.listener(move |app, _, w, cx| app.question_action(action, w, cx)),
        );
    }
    button
}

fn copy_metrics(
    text: &str,
    size: f32,
    weight: FontWeight,
    width: f32,
    w: &Window,
    cx: &App,
) -> (usize, f32) {
    let mut run = super::document_fonts::interface_style(w, cx).to_run(text.len());
    run.font.weight = weight;
    let Ok(lines) = w.text_system().shape_text(
        text.to_owned().into(),
        px(size),
        &[run],
        Some(px(width.max(1.))),
        None,
    ) else {
        return (1, width);
    };
    let height = lines
        .iter()
        .map(|line| line.wrap_boundaries().len() + 1)
        .sum();
    let width = lines
        .iter()
        .map(|line| f32::from(line.unwrapped_layout.width))
        .fold(0., f32::max);
    (height, width)
}

fn copy_toggle(
    part: CopyPart,
    expanded: bool,
    text: &str,
    cx: &mut Context<DshApp>,
) -> gpui::Stateful<gpui::Div> {
    let id = match part {
        CopyPart::Title => gpui::ElementId::from("question-expand-title"),
        CopyPart::Description(index) => {
            gpui::ElementId::from(("question-expand-description", index))
        }
    };
    let label = if expanded { "收起" } else { "展开" };
    icon_action(
        id,
        label,
        if expanded { UP } else { DOWN },
        false,
        Action::ToggleCopy(part),
        cx,
    )
    .aria_expanded(expanded)
    .aria_description(text.to_owned())
    .block_mouse_except_scroll()
    .tooltip(move |_, cx| cx.new(|_| TurnActionHint(label.into())).into())
}

fn fade_gradient(bg: gpui::Hsla) -> gpui::Background {
    gpui::linear_gradient(
        90.,
        gpui::linear_color_stop(bg.opacity(0.), 0.),
        gpui::linear_color_stop(bg, 0.5),
    )
}

fn copy_fade(line_height: f32, bg: gpui::Hsla) -> gpui::Div {
    div()
        .absolute()
        .right_0()
        .bottom_0()
        .w(px(56.))
        .h(px(line_height))
        .bg(fade_gradient(bg))
}

fn indicator(checked: bool, multi: bool, number: Option<usize>, cx: &App) -> gpui::Div {
    let p = dsh(cx);
    let seat = row().size(px(20.)).mt(px(2.)).flex_none().justify_center();
    if multi {
        seat.child(
            row()
                .size(px(14.))
                .justify_center()
                .rounded(px(4.))
                .border(px(0.5))
                .border_color(if checked {
                    p.label_primary
                } else {
                    p.border_l4
                })
                .when(checked, |box_| {
                    box_.bg(p.label_primary).child(question_icon(
                        CHECK,
                        12.,
                        p.label_primary_foreground,
                    ))
                }),
        )
    } else {
        let seat = seat
            .rounded(px(4.))
            .bg(p.bg_overlay)
            .text_color(p.label_secondary)
            .text_size(px(12.))
            .line_height(px(18.))
            .font_weight(FontWeight::MEDIUM);
        if let Some(number) = number {
            seat.child(number.to_string())
        } else {
            seat.child(question_icon(EDIT, 12., p.label_secondary))
        }
    }
}

impl DshApp {
    pub(super) fn render_questions(
        &mut self,
        width: f32,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let panel = self
            .conversation
            .questions
            .iter_mut()
            .rev()
            .find(|panel| !panel.hidden)?;
        let index = panel.index;
        let question = &panel.questions[index];
        let draft = &panel.drafts[index];
        let input = &panel.editors[index];
        let locked = panel.locked();
        let reviewing = panel.stage == Stage::Review;
        if input.read(cx).disabled != (locked || reviewing) {
            input.update(cx, |input, cx| {
                input.disabled = locked || reviewing;
                cx.notify();
            });
        }
        let focus = input.read(cx).focus.clone();
        let focused = focus.is_focused(w);
        if question.options.is_empty()
            && panel.clock.is_none()
            && !locked
            && !reviewing
            && !panel.minimized
            && !self.show_settings
            && panel.autofocus.insert(index)
        {
            let session = self.active_session.clone();
            let id = panel.id.clone();
            cx.on_next_frame(w, move |app, w, cx| {
                if app.active_session == session
                    && app.page == WorkspacePage::Conversation
                    && !app.show_settings
                    && app.conversation.permission.is_none()
                    && app.conversation.plan_reviews.is_empty()
                    && app.conversation.visible_question().is_some_and(|panel| {
                        panel.id == id && panel.index == index && !panel.minimized
                    })
                {
                    focus.focus(w, cx);
                }
            });
        }

        let p = dsh(cx);
        let compact = f32::from(w.viewport_size().width) <= 720.;
        let last = index + 1 == panel.questions.len();
        let (status, take_time) = match (panel.stage, panel.clock) {
            (_, _) if panel.timed_out && panel.stage != Stage::Review => {
                ("已超时，工作继续，你仍可回答".to_owned(), false)
            }
            (Stage::Review, _) => ("已回答".to_owned(), false),
            (Stage::Continued | Stage::Continuing, _) => ("已继续工作，仍可回答".to_owned(), false),
            (_, Some(Clock::Held)) => ("会一直等你回答".to_owned(), false),
            (_, Some(clock)) => {
                let seconds =
                    (clock.remaining(bridge::monotonic_ms()).unwrap_or(0.) / 1000.).ceil();
                let status = if matches!(clock, Clock::Paused { .. }) {
                    format!("已暂停 · 剩余 {seconds} 秒")
                } else {
                    format!("{seconds} 秒后继续工作")
                };
                (status, true)
            }
            _ => (String::new(), false),
        };
        let title_size = if compact { 15. } else { 16. };
        let title_height = if compact { 21. } else { 22. };
        let card_width = (width - 32.).max(0.);
        let card_background = composer_bg(cx);
        let title = || {
            div()
                .min_w_0()
                .text_size(px(title_size))
                .line_height(px(title_height))
                .font_weight(FontWeight::MEDIUM)
                .child(question.question.clone())
        };
        let mut heading = column().min_w_0().flex_1().gap_0();
        if let Some(header) = &question.header {
            heading = heading.child(
                div()
                    .mb(px(5.))
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(p.label_tertiary)
                    .child(header.clone()),
            );
        }
        if panel.minimized {
            heading = heading.child(title().line_clamp(2).text_ellipsis().overflow_hidden());
        }
        let actions = row()
            .flex_none()
            .gap(px(4.))
            .when(!status.is_empty(), |actions| {
                actions.child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(24.))
                        .text_color(p.label_secondary)
                        .child(status),
                )
            })
            .when(take_time, |actions| {
                actions.child(dialog_action(
                    "hold-question",
                    "慢慢回答",
                    false,
                    locked,
                    0.,
                    cx,
                    |app, w, cx| app.question_action(Action::Hold, w, cx),
                ))
            })
            .child(
                icon_action(
                    "toggle-questions",
                    if panel.minimized {
                        "展开问题卡片"
                    } else {
                        "收起问题卡片"
                    },
                    if panel.minimized { UP } else { DOWN },
                    locked,
                    Action::Toggle,
                    cx,
                )
                .aria_expanded(!panel.minimized),
            )
            .child(icon_action(
                "close-questions",
                if panel.call_id.is_some() {
                    "收起问题面板，可从工具调用重新打开"
                } else {
                    "放弃整组问题"
                },
                CLOSE,
                locked,
                Action::Close,
                cx,
            ));
        let header = row()
            .items_start()
            .flex_none()
            .gap(px(16.))
            .pl(px(if compact { 18. } else { 24. }))
            .pr(px(if compact { 12. } else { 16. }))
            .pt(px(if compact { 10. } else { 20. }))
            .when(panel.minimized, |header| header.pb(px(14.)))
            .child(heading)
            .child(actions);
        let card = column()
            .id("user-questions")
            .role(gpui::Role::Group)
            .aria_label(question.question.clone())
            .tab_group()
            .on_key_down(|event, w, cx| {
                traverse_dock_focus(event, w, cx);
            })
            .w(px(card_width))
            .max_w_full()
            .gap_0()
            .pb(px(10.))
            .overflow_hidden()
            .rounded(px(20.))
            .bg(composer_bg(cx))
            .shadow(crate::tokens::elevation_panel())
            .child(header);

        let card = if panel.minimized {
            card
        } else {
            let title_part = CopyPart::Title;
            let title_expanded = panel.expanded_copy.contains(&title_part);
            let (title_rows, _) = copy_metrics(
                &question.question,
                title_size,
                FontWeight::MEDIUM,
                card_width - 48.,
                w,
                cx,
            );
            let title_clipped = title_rows > 3 && !title_expanded;
            let title_block = div()
                .relative()
                .mx(px(24.))
                .mt(px(5.))
                .flex_none()
                .when(title_expanded, |block| block.pb(px(24.)))
                .child(
                    div()
                        .relative()
                        .child(title().when(title_clipped, |title| {
                            title.max_h(px(title_height * 2.)).overflow_hidden()
                        }))
                        .when(title_clipped, |line| {
                            line.child(copy_fade(title_height, card_background))
                        }),
                )
                .when(title_rows > 3 || title_expanded, |block| {
                    block.child(
                        copy_toggle(title_part, title_expanded, &question.question, cx)
                            .absolute()
                            .right(px(-4.))
                            .bottom(px(-1.)),
                    )
                });

            let options_padding = if compact { 8. } else { 12. };
            let option_left = if compact { 6. } else { 8. };
            let option_right = if compact { 6. } else { 12. };
            let copy_width =
                (card_width - 2. * options_padding - option_left - option_right - 29.).max(1.);
            let selectable = !locked && !reviewing;
            let mut options = column()
                .id("question-options")
                .role(if question.multi_select {
                    gpui::Role::Group
                } else {
                    gpui::Role::RadioGroup
                })
                .w_full()
                .flex_none()
                .mt(px(8.))
                .px(px(options_padding))
                .py(px(4.))
                .gap(px(1.));
            for (choice_index, choice) in question.options.iter().enumerate() {
                let (label, recommended) = choice.display();
                let checked = draft.selected.contains(&choice.label);
                let selected = !question.multi_select && checked;
                let description = choice.description.as_deref().unwrap_or_default();
                let has_description = !description.trim().is_empty();
                let part = CopyPart::Description(choice_index);
                let description_expanded = panel.expanded_copy.contains(&part);
                let group = format!("question-choice-row-{choice_index}");
                let mut description_toggle = false;
                let mut copy = row()
                    .flex_1()
                    .min_w_0()
                    .flex_wrap()
                    .gap_x(px(6.))
                    .gap_y_0()
                    .items_start()
                    .child(
                        row()
                            .min_w_0()
                            .max_w_full()
                            .flex_wrap()
                            .gap_x(px(6.))
                            .gap_y(px(2.))
                            .items_baseline()
                            .child(
                                div()
                                    .min_w_0()
                                    .text_size(px(14.))
                                    .line_height(px(24.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(label.to_owned()),
                            )
                            .when(recommended, |line| {
                                line.child(
                                    div()
                                        .rounded(px(4.))
                                        .px(px(4.))
                                        .bg(p.sidebar_nav_active_accent)
                                        .text_color(p.button_info_fill)
                                        .text_size(px(11.))
                                        .line_height(px(18.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("推荐"),
                                )
                            }),
                    );
                if has_description {
                    let (rows, natural_width) =
                        copy_metrics(description, 13., FontWeight::NORMAL, copy_width, w, cx);
                    let (_, label_width) =
                        copy_metrics(label, 14., FontWeight::MEDIUM, copy_width, w, cx);
                    let badge_width = if recommended {
                        let (_, width) =
                            copy_metrics("推荐", 11., FontWeight::SEMIBOLD, copy_width, w, cx);
                        width + 14.
                    } else {
                        0.
                    };
                    let description_width = natural_width.min(copy_width).ceil().max(1.);
                    let shares_label_line =
                        label_width + badge_width + 6. + description_width <= copy_width;
                    description_toggle = rows > 2 || description_expanded;
                    let clipped = rows > 2 && !description_expanded;
                    let rest_bg = if selected {
                        card_background.blend(p.hover)
                    } else {
                        card_background
                    };
                    let hover_bg = card_background.blend(p.hover);
                    let description_line = div()
                        .relative()
                        .flex_none()
                        .max_w_full()
                        .w(px(description_width))
                        .when(shares_label_line, |line| line.mt(px(2.)))
                        .child(
                            div()
                                .text_size(px(13.))
                                .line_height(px(20.))
                                .text_color(p.label_tertiary)
                                .child(description.to_owned())
                                .when(clipped, |text| text.max_h(px(20.)).overflow_hidden()),
                        )
                        .when(clipped, |line| {
                            line.child(
                                div()
                                    .id(("question-description-fade", choice_index))
                                    .child(copy_fade(20., rest_bg))
                                    .absolute()
                                    .inset_0()
                                    .when(selectable, |fade| {
                                        // The fade needs the same background as the hovered option.
                                        fade.group_hover(group.as_str(), |style| style.opacity(0.))
                                    }),
                            )
                            .when(selectable, |line| {
                                line.child(
                                    div()
                                        .id(("question-description-fade-hover", choice_index))
                                        .absolute()
                                        .inset_0()
                                        .opacity(0.)
                                        .group_hover(group.as_str(), |style| style.opacity(1.))
                                        .child(copy_fade(20., hover_bg)),
                                )
                            })
                        });
                    copy = copy.child(description_line);
                }
                let mut choice = row()
                    .id(("question-choice", choice_index))
                    .role(if question.multi_select {
                        gpui::Role::CheckBox
                    } else {
                        gpui::Role::RadioButton
                    })
                    .aria_label(label.to_owned())
                    .aria_toggled(if checked {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .when(has_description, |choice| {
                        choice.aria_description(description.to_owned())
                    })
                    .tab_index(0)
                    .tab_stop(selectable)
                    .w_full()
                    .flex_none()
                    .min_h(px(40.))
                    .items_start()
                    .gap(px(8.))
                    .pt(px(8.))
                    .pb(px(if description_expanded { 32. } else { 8. }))
                    .pl(px(option_left))
                    .pr(px(option_right))
                    .child(indicator(
                        checked,
                        question.multi_select,
                        Some(choice_index + 1),
                        cx,
                    ))
                    .child(copy);
                if selectable {
                    choice = choice
                        .cursor_pointer()
                        .rounded(px(12.))
                        .focus_visible(|style| {
                            style.shadow(vec![
                                gpui::BoxShadow::new(px(0.), px(0.), p.brand_accent)
                                    .spread_radius(px(1.))
                                    .inset(),
                            ])
                        })
                        .on_click(cx.listener(move |app, _, w, cx| {
                            app.question_action(Action::Choose(choice_index), w, cx)
                        }))
                        .capture_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, w, cx| {
                            if event.keystroke.key == "enter" && !w.default_prevented() {
                                app.question_action(Action::Submit, w, cx);
                                w.prevent_default();
                                cx.stop_propagation();
                            }
                        }));
                }
                let option_row = div()
                    .id(("question-option-row", choice_index))
                    .group(group)
                    .relative()
                    .w_full()
                    .flex_none()
                    .rounded(px(12.))
                    .border(px(0.5))
                    .border_color(if selected {
                        p.border_l2
                    } else {
                        gpui::transparent_black()
                    })
                    .when(selected, |row| row.bg(p.hover))
                    .when(selectable, |row| row.hover(|style| style.bg(p.hover)))
                    .child(choice)
                    .when(description_toggle, |row| {
                        row.child(
                            copy_toggle(part, description_expanded, description, cx)
                                .absolute()
                                .right(px(if compact { 2. } else { 8. }))
                                .bottom(px(6.)),
                        )
                    });
                options = options.child(option_row);
            }

            let field_width = if question.options.is_empty() {
                card_width - 2. * options_padding - 49.
            } else {
                copy_width
            };
            let (lines, _) =
                copy_metrics(&draft.custom, 14., FontWeight::NORMAL, field_width, w, cx);
            let lines = lines.clamp(1, 6) as f32;
            // Keep the 24px Harness row budget while limiting the KUI
            // editor's own scroll viewport to six of its text rows.
            let editor_line_height = input.read(cx).font_size + 7.;
            let field = div()
                .flex_1()
                .min_w_0()
                .h(px(lines * 24.))
                .child(div().h(px(lines * editor_line_height)).child(input.clone()));
            let custom = if question.options.is_empty() {
                row()
                    .mx(px(12.))
                    .gap_0()
                    .min_h(px(64.))
                    .items_start()
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(16.))
                    .border(px(0.5))
                    .border_color(if focused {
                        p.state_business_primary
                    } else {
                        p.border_l4
                    })
                    .bg(p.bg_module_platform)
                    .child(field)
            } else {
                let active = !draft.custom.is_empty()
                    || (draft.selected.is_empty() || question.multi_select) && focused;
                row()
                    .w_full()
                    .flex_none()
                    .min_h(px(40.))
                    .items_start()
                    .gap(px(8.))
                    .py(px(8.))
                    .pl(px(option_left))
                    .pr(px(option_right))
                    .rounded(px(12.))
                    .border(px(0.5))
                    .border_color(if active {
                        p.border_l2
                    } else {
                        gpui::transparent_black()
                    })
                    .when(active, |custom| custom.bg(p.hover))
                    .when(selectable, |custom| custom.hover(|style| style.bg(p.hover)))
                    .child(indicator(
                        !draft.custom.is_empty(),
                        question.multi_select,
                        None,
                        cx,
                    ))
                    .child(field)
            };
            if reviewing && draft.skipped {
                options = options.child(
                    div()
                        .px(px(12.))
                        .py(px(8.))
                        .text_size(px(14.))
                        .line_height(px(24.))
                        .text_color(p.label_tertiary)
                        .child("这道问题当时被跳过。"),
                );
            }
            if !reviewing || !draft.custom.is_empty() {
                options = options.child(custom);
            }

            let body = column()
                .id("question-body")
                .track_focus(&panel.body_focus)
                .track_scroll(&panel.scroll)
                .overflow_y_scroll()
                .min_h_0()
                .gap_0()
                .child(title_block)
                .when_some(question.detail.as_deref(), |body, detail| {
                    body.child(
                        render_markdown(
                            detail,
                            "question-detail",
                            None,
                            &HashSet::new(),
                            0,
                            None,
                            None,
                            cx,
                        )
                        .w_auto()
                        .mx(px(24.))
                        .mb(px(8.))
                        .flex_none(),
                    )
                })
                .child(options);
            let progress = row()
                .gap(px(6.))
                .flex_none()
                .child(icon_action(
                    "question-prev",
                    "上一题",
                    LEFT,
                    locked || index == 0,
                    Action::Previous,
                    cx,
                ))
                .child(
                    div()
                        .px(px(4.))
                        .text_size(px(14.))
                        .line_height(px(24.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.label_secondary)
                        .child(format!("{} / {}", index + 1, panel.questions.len())),
                )
                .child(icon_action(
                    "question-next",
                    "下一题",
                    RIGHT,
                    locked || last,
                    Action::Next,
                    cx,
                ));
            let footer = row()
                .flex_none()
                .mt(px(12.))
                .pl(px(if compact { 10. } else { 18. }))
                .pr(px(10.))
                .when(compact, |footer| footer.items_end())
                .gap(px(12.))
                .child(progress)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_right()
                        .text_color(p.state_error_primary)
                        .child(panel.error.clone().unwrap_or_default()),
                )
                .when(!reviewing, |footer| {
                    footer
                        .child(dialog_action(
                            "skip-question",
                            "跳过",
                            false,
                            locked,
                            0.,
                            cx,
                            |app, w, cx| app.question_action(Action::Skip, w, cx),
                        ))
                        .child(dialog_action(
                            "submit-question",
                            if panel.busy == Some(Decision::Answer) {
                                "正在提交…"
                            } else if last {
                                "提交"
                            } else {
                                "下一题"
                            },
                            true,
                            locked || !draft.answered(),
                            0.,
                            cx,
                            |app, w, cx| app.question_action(Action::Continue, w, cx),
                        ))
                });
            let max_height = (f32::from(w.viewport_size().height) * 0.6).min(520.);
            card.max_h(px(max_height)).child(body).child(footer)
        };
        Some(
            row()
                .w_full()
                .flex_none()
                .px(px(16.))
                .pt(px(6.))
                .pb(px(10.))
                .justify_center()
                .child(card),
        )
    }

    pub(super) fn render_question_tool(
        &self,
        call_id: &str,
        entry: Option<&ChatEntry>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        let pending = self
            .conversation
            .questions
            .iter()
            .find(|panel| panel.is_pending() && panel.call_id.as_deref() == Some(call_id));
        let settled = self.conversation.settled_questions.get(call_id);
        let queued = self.conversation.queued_question_replies.get(call_id);
        let questions: Vec<Question> = entry
            .and_then(|entry| serde_json::from_str::<Value>(&entry.input).ok())
            .and_then(|arguments| serde_json::from_value(arguments["questions"].clone()).ok())
            .unwrap_or_default();
        let output = entry
            .and_then(|entry| serde_json::from_str::<Value>(&entry.text).ok())
            .unwrap_or_default();
        let result_answers: Option<Vec<RecordedAnswer>> =
            serde_json::from_value(output["answers"].clone()).ok();
        let answers = queued.or(settled).or(result_answers.as_ref());
        let paired = answers.and_then(|answers| pair_recorded_answers(&questions, answers));
        let continued = pending
            .is_some_and(|panel| matches!(panel.stage, Stage::Continuing | Stage::Continued));
        let running =
            entry.is_some_and(|entry| matches!(entry.status.as_str(), "in_progress" | "pending"));
        let status = entry.map(|entry| entry.status.as_str()).unwrap_or_default();
        let failed = status == "failed";
        let mut verdict = None;
        let summary = if let Some(answers) = answers.filter(|answers| !answers.is_empty()) {
            let answered = answers
                .iter()
                .filter(|answer| {
                    !answer.selected.is_empty()
                        || answer.custom.as_ref().is_some_and(|text| !text.is_empty())
                })
                .count();
            format!("{answered}/{} 已回答", answers.len())
        } else if continued {
            verdict = Some("等待中的问题仍可在输入框中回答");
            "已继续工作，仍可回答".into()
        } else if pending.is_some() || running {
            "等待回答".into()
        } else if output["pending"] == true || settled.is_some() {
            verdict = Some("该问题已结束，结果见下方对话");
            "已结束".into()
        } else if status == "ASK_CANCELLED" {
            verdict = Some("本轮已取消，未提交回答");
            "已取消".into()
        } else if status == "ASK_ABORTED" {
            verdict = Some("本轮已中断，未提交回答");
            "已中断".into()
        } else if failed {
            "失败".into()
        } else {
            String::new()
        };
        let key = format!("question-tool-{call_id}");
        let expanded = self.conversation.expanded_entries.contains(&key);
        let for_toggle = key.clone();
        let mut title = row()
            .flex_1()
            .min_w_0()
            .gap(px(6.))
            .h(px(24.))
            .child(question_icon(
                if expanded { DOWN } else { QUESTION },
                16.,
                p.label_tertiary,
            ))
            .child(
                div()
                    .flex_none()
                    .text_size(px(13.))
                    .text_color(p.label_tertiary)
                    .child("提问"),
            )
            .when(!summary.is_empty(), |row| {
                row.child(div().text_color(p.label_dimmed).child("·"))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(if failed {
                                p.state_error_primary
                            } else {
                                p.label_tertiary
                            })
                            .child(summary),
                    )
            });
        if entry.is_some() {
            title = title.child(div().flex_1());
        }
        let mut header = row().w_full().min_h(px(24.)).gap(px(8.));
        if entry.is_some() {
            header = header.child(dock_action_button(
                title
                    .id(gpui::ElementId::Name(key.clone().into()))
                    .role(gpui::Role::Button)
                    .aria_label("提问")
                    .aria_expanded(expanded)
                    .tab_index(0)
                    .cursor_pointer(),
                cx.listener(move |app, _, _, cx| toggle_question_disclosure(app, &for_toggle, cx)),
            ));
        } else {
            header = header.child(title);
        }
        if pending.is_some() || ((settled.is_some() || queued.is_some()) && paired.is_some()) {
            let id = call_id.to_owned();
            let answerable = pending.is_some();
            let read_panel = settled.is_some() && queued.is_none();
            let label = if answerable { "回答" } else { "查看回答" };
            header = header.child(dock_action_button(
                row()
                    .id(gpui::ElementId::Name(
                        format!("answer-question-{id}").into(),
                    ))
                    .role(gpui::Role::Button)
                    .aria_label(label)
                    .tab_index(0)
                    .h(px(22.))
                    .px(px(10.))
                    .gap(px(4.))
                    .rounded_full()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(p.label_primary)
                    .bg(p.button_floating)
                    .cursor_pointer()
                    .hover(|style| style.bg(p.button_floating_hover))
                    .child(question_icon(QUESTION, 13., p.label_primary))
                    .child(label),
                cx.listener(move |app, _, w, cx| {
                    if answerable {
                        app.reopen_question(&id, cx);
                    } else if read_panel {
                        app.review_question(&id, w, cx);
                    } else {
                        toggle_question_disclosure(app, &key, cx);
                    }
                }),
            ));
        }
        let mut row = column().w_full().mt(px(4.)).gap_0().child(header);
        if expanded && let Some(entry) = entry {
            let mut detail = column()
                .id(gpui::ElementId::Name(
                    format!("question-details-{call_id}").into(),
                ))
                .w_full()
                .ml(px(4.))
                .my(px(4.))
                .px(px(20.))
                .py(px(16.))
                .gap(px(16.))
                .max_h(px(360.))
                .overflow_y_scroll()
                .rounded(px(16.))
                .border(px(0.5))
                .border_color(p.border_l1)
                .bg(p.bg_base)
                .text_size(px(14.))
                .line_height(px(24.));
            if let Some(paired) = paired {
                for (question, answer) in questions.iter().zip(paired) {
                    let answers: Vec<_> = answer
                        .selected
                        .iter()
                        .cloned()
                        .chain(answer.custom.clone().filter(|value| !value.is_empty()))
                        .collect();
                    let item = column().gap(px(2.)).child(
                        div()
                            .text_color(p.label_tertiary)
                            .child(question.question.clone()),
                    );
                    detail = detail.child(if answers.is_empty() {
                        item.child(div().text_color(p.label_tertiary).child("未回答"))
                    } else {
                        item.children(answers.into_iter().map(|value| div().child(value)))
                    });
                }
            } else if let Some(verdict) = verdict.filter(|_| !questions.is_empty()) {
                detail = detail.child(verdict).child(
                    column()
                        .pl(px(20.))
                        .gap(px(8.))
                        .text_color(p.label_tertiary)
                        .children(
                            questions
                                .into_iter()
                                .map(|q| div().child(format!("• {}", q.question))),
                        ),
                );
            } else {
                detail = detail
                    .when(!entry.input.is_empty(), |d| d.child(entry.input.clone()))
                    .when(!entry.text.is_empty(), |d| d.child(entry.text.clone()));
            }
            row = row.child(detail);
        }
        row
    }
}

fn toggle_question_disclosure(app: &mut DshApp, key: &str, cx: &mut Context<DshApp>) {
    super::workspace::toggle_entry(&mut app.conversation.expanded_entries, key);
    cx.notify();
}

pub(super) fn render_question_reply(
    entry: &ChatEntry,
    copied_mark: Option<&str>,
    font_delta: i32,
    cx: &mut Context<DshApp>,
) -> gpui::Div {
    let p = dsh(cx);
    let font_delta = font_delta as f32;
    let mut bubble = column()
        .px(px(16.))
        .py(px(10.))
        .rounded(px(20.))
        .bg(p.bubble)
        .gap(px(12.))
        .text_size(px(14. + font_delta))
        .line_height(px(22. + font_delta));
    let reply = serde_json::from_str::<crate::questions::QuestionReply>(&entry.input).ok();
    if let Some(reply) = reply.filter(|reply| !reply.questions.is_empty()) {
        for question in &reply.questions {
            let mut title = column().gap(px(3.)).text_color(p.label_secondary);
            if let Some(header) = question
                .header
                .as_ref()
                .filter(|header| !header.is_empty() && **header != question.question)
            {
                title = title.child(div().text_size(px(13. + font_delta)).child(header.clone()));
            }
            let values = reply.answer_values(&question.id);
            let answer = if values.is_empty() {
                "已跳过".to_owned()
            } else {
                values.join(", ")
            };
            bubble = bubble.child(
                column()
                    .gap(px(4.))
                    .child(title.child(question.question.clone()))
                    .child(div().child(answer)),
            );
        }
    } else {
        bubble = bubble.child(entry.text.clone());
    }

    let copied = copied_mark == Some(&entry.key);
    let key = entry.key.clone();
    let copy_text = entry.text.clone();
    let actions = row()
        .h(px(28.))
        .gap(px(8.))
        .pr(px(4.))
        .text_color(p.label_tertiary)
        .when_some(entry.at_ms.map(question_reply_time), |actions, time| {
            actions.child(
                div()
                    .pr(px(4.))
                    .text_size(px(13.))
                    .line_height(px(24.))
                    .child(time),
            )
        })
        .child(dock_action_button(
            row()
                .id(gpui::ElementId::Name(
                    format!("copy-question-{}", entry.key).into(),
                ))
                .role(gpui::Role::Button)
                .aria_label(if copied { "已复制" } else { "复制" })
                .tab_index(0)
                .size(px(28.))
                .justify_center()
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|style| style.bg(p.hover).text_color(p.label_secondary))
                .child(icon(
                    if copied { Glyph::Check } else { Glyph::Copy },
                    15.,
                    p.label_tertiary,
                )),
            cx.listener(move |app, _, _, cx| {
                bridge::copy_text(&copy_text);
                super::workspace::mark_copied(app, &key, cx);
            }),
        ));
    row().w_full().justify_end().child(
        column()
            .id(gpui::ElementId::Name(entry.key.clone().into()))
            .role(gpui::Role::Group)
            .aria_label("问题与回答")
            .max_w(gpui::relative(0.702))
            .min_w_0()
            .items_end()
            .gap(px(6.))
            .child(bubble)
            .child(actions),
    )
}

fn question_reply_time(ms: f64) -> String {
    let date = js_sys::Date::new(&ms.into());
    format!("{:02}:{:02}", date.get_hours(), date.get_minutes())
}
