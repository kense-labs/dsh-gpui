//! The per-session feedback list and versioned rating operations.
use super::{dock_controls::dock_action_button, *};
use gpui::{Role, svg};
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Rating {
    Positive,
    Negative,
}

impl Rating {
    fn as_str(self) -> &'static str {
        match self {
            Self::Positive => "positive",
            Self::Negative => "negative",
        }
    }

    fn artwork(self, active: bool) -> &'static [u8] {
        match (self, active) {
            (Self::Positive, false) => include_bytes!("../../assets/turn-like.svg"),
            (Self::Positive, true) => include_bytes!("../../assets/turn-like-fill.svg"),
            (Self::Negative, false) => include_bytes!("../../assets/turn-dislike.svg"),
            (Self::Negative, true) => include_bytes!("../../assets/turn-dislike-fill.svg"),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    message_id: String,
    rating: Rating,
    version: String,
}

struct RowFocus {
    handle: gpui::FocusHandle,
    _subscriptions: [Subscription; 2],
}

enum Action {
    Choose(Rating),
    Submit {
        draft: u64,
        rating: Rating,
        note: String,
        category: Option<&'static str>,
    },
}

#[derive(Default)]
pub(super) struct MessageFeedback {
    epoch: u64,
    ready: bool,
    load_error: Option<String>,
    busy: bool,
    items: HashMap<String, Item>,
    seeded: HashSet<String>,
    row_focus: HashMap<String, RowFocus>,
    queue: VecDeque<(String, Action)>,
    pending: HashSet<String>,
    failures: HashMap<String, &'static str>,
}

impl MessageFeedback {
    pub(super) fn reset(&mut self) {
        *self = Self {
            epoch: self.epoch + 1,
            ..Self::default()
        };
    }
}

impl DshApp {
    pub(super) fn prepare_rating_rows(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        for message in self.conversation.entries.iter().filter_map(|entry| {
            entry.external_id.as_deref().filter(|id| {
                entry.role == EntryRole::Assistant && self.conversation.turn_tails.contains(id)
            })
        }) {
            self.message_feedback
                .row_focus
                .entry(message.to_owned())
                .or_insert_with(|| {
                    let handle = cx.focus_handle();
                    let message = message.to_owned();
                    let enter = cx.on_focus_in(&handle, w, move |app, w, cx| {
                        app.seed_message_feedback(&message, w, cx);
                        cx.notify();
                    });
                    let leave = cx.on_focus_out(&handle, w, |_, _, _, cx| cx.notify());
                    RowFocus {
                        handle,
                        _subscriptions: [enter, leave],
                    }
                });
        }
    }

    pub(super) fn rating_row_focus(&self, message: &str) -> Option<&gpui::FocusHandle> {
        self.message_feedback
            .row_focus
            .get(message)
            .map(|row| &row.handle)
    }

    fn seed_message_feedback(&mut self, message: &str, w: &mut Window, cx: &mut Context<Self>) {
        if self.message_feedback.seeded.insert(message.to_owned()) {
            self.load_message_feedback(w, cx);
        }
    }

    fn load_message_feedback(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.message_feedback;
        let Some(session) = self.active_session.clone() else {
            return;
        };
        if state.ready || state.busy {
            return;
        }
        state.busy = true;
        state.load_error = None;
        let epoch = state.epoch;
        cx.spawn_in(w, async move |this, cx| {
            let result = request(&session, json!({"operation":"list"}))
                .await
                .and_then(|value| {
                    serde_json::from_value::<Vec<Item>>(value["items"].clone())
                        .map_err(|error| error.to_string())
                });
            let _ = this.update_in(cx, |app, w, cx| {
                let state = &mut app.message_feedback;
                if state.epoch != epoch {
                    return;
                }
                state.busy = false;
                match result {
                    Ok(items) => {
                        state.ready = true;
                        state.items = items
                            .into_iter()
                            .map(|item| (item.message_id.clone(), item))
                            .collect();
                    }
                    Err(error) => state.load_error = Some(error),
                }
                app.run_message_feedback(false, w, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn queue_message_feedback(
        &mut self,
        message: String,
        action: Action,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = &mut self.message_feedback;
        if state.pending.insert(message.clone()) {
            state.failures.remove(&message);
            state.queue.push_back((message, action));
            self.run_message_feedback(true, w, cx);
            cx.notify();
        }
    }

    pub(super) fn submit_message_feedback(
        &mut self,
        message: String,
        rating: Rating,
        draft: u64,
        note: String,
        category: Option<&'static str>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.queue_message_feedback(
            message,
            Action::Submit {
                draft,
                rating,
                note,
                category,
            },
            w,
            cx,
        );
    }

    /// Run one list or mutation at a time. A click waiting on a failed list can
    /// still open the form, and submitting that form retries the read first.
    fn run_message_feedback(&mut self, seed: bool, w: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.active_session.clone() else {
            return;
        };
        while !self.message_feedback.busy && !self.message_feedback.queue.is_empty() {
            if seed && !self.message_feedback.ready {
                self.load_message_feedback(w, cx);
                return;
            }
            let state = &mut self.message_feedback;
            let (message, action) = state.queue.pop_front().unwrap();
            let observed = state.items.get(&message);
            let (params, draft) = match action {
                Action::Choose(rating) => {
                    if let Some(item) = observed.filter(|item| state.ready && item.rating == rating)
                    {
                        (
                            json!({"operation":"delete","messageId":message,"ifVersion":item.version}),
                            None,
                        )
                    } else {
                        state.pending.remove(&message);
                        self.open_message_feedback(session.clone(), message, rating, w, cx);
                        continue;
                    }
                }
                Action::Submit {
                    draft,
                    rating,
                    note,
                    category,
                } => {
                    if !state.ready {
                        state.pending.remove(&message);
                        let error = state
                            .load_error
                            .clone()
                            .unwrap_or_else(|| "feedback-failed".into());
                        self.finish_feedback(Some(draft), Err(error), w, cx);
                        continue;
                    }
                    (
                        json!({
                            "operation":"put", "messageId":message, "rating":rating.as_str(),
                            "note":(!note.is_empty()).then_some(note), "category":category,
                            "ifVersion":observed.map(|item| item.version.as_str()),
                        }),
                        Some(draft),
                    )
                }
            };
            state.busy = true;
            let epoch = state.epoch;
            cx.spawn_in(w, async move |this, cx| {
                let result = invoke_feedback(&session, params).await;
                let _ = this.update_in(cx, |app, w, cx| {
                    let state = &mut app.message_feedback;
                    if state.epoch != epoch {
                        return;
                    }
                    state.busy = false;
                    state.pending.remove(&message);
                    let result = apply_mutation(&mut state.items, message.clone(), result);
                    if let Some(draft) = draft {
                        app.finish_feedback(Some(draft), result, w, cx);
                    } else if let Err(error) = result {
                        state
                            .failures
                            .insert(message, super::session_feedback::feedback_failure(&error));
                    }
                    app.run_message_feedback(true, w, cx);
                    cx.notify();
                });
            })
            .detach();
            return;
        }
    }

    pub(super) fn render_message_ratings(
        &self,
        message: &str,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let state = &self.message_feedback;
        let pending = state.pending.contains(message);
        let current = state.items.get(message).map(|item| item.rating);
        let failure = state
            .failures
            .get(message)
            .copied()
            .or(state.load_error.as_ref().map(|_| "反馈状态加载失败"));
        let p = dsh(cx);
        let delta = self.content_font_delta as f32;
        let mut buttons = row().gap(px(8.)).flex_none();
        for rating in [Rating::Positive, Rating::Negative] {
            let active = current == Some(rating);
            let label = match (rating, active) {
                (_, true) => "取消标记",
                (Rating::Positive, false) => "好的回答",
                (Rating::Negative, false) => "有问题的回答",
            };
            let message = message.to_owned();
            let hover = message.clone();
            let group = format!("rating-{message}-{}", rating.as_str());
            buttons = buttons.child(dock_action_button(
                row()
                    .id(group.clone())
                    .group(group.clone())
                    .role(Role::Button)
                    .aria_label(label)
                    .aria_toggled(if active {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .tab_index(0)
                    .size(px(28. + delta))
                    .justify_center()
                    .flex_none()
                    .rounded(px(8.))
                    .text_color(p.label_tertiary)
                    .when(pending, |button| button.opacity(0.4))
                    .when(!pending, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(p.hover).text_color(p.label_secondary))
                    })
                    .child(
                        svg()
                            .data(rating.artwork(active))
                            .size(px(15. + delta))
                            .flex_none()
                            .text_color(p.label_tertiary)
                            .when(!pending, |icon| {
                                icon.group_hover(group, |style| style.text_color(p.label_secondary))
                            }),
                    )
                    .on_hover(cx.listener(move |app, inside, w, cx| {
                        if *inside {
                            app.seed_message_feedback(&hover, w, cx);
                        }
                    })),
                cx.listener(move |app, _, w, cx| {
                    app.queue_message_feedback(message.clone(), Action::Choose(rating), w, cx);
                }),
            ));
        }
        buttons.when_some(failure, |element, failure| {
            element.child(
                div()
                    .id(format!("rating-status-{message}"))
                    .role(Role::Status)
                    .pl(px(4.))
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .text_color(p.label_tertiary)
                    .child(failure),
            )
        })
    }
}

async fn invoke_feedback(session: &str, operation: Value) -> Result<Value, String> {
    bridge::invoke(
        "message_feedback",
        json!({"sessionId":session,"request":operation}),
    )
    .await
}

async fn request(session: &str, operation: Value) -> Result<Value, String> {
    let reply = invoke_feedback(session, operation).await?;
    if reply["ok"] == true {
        Ok(reply["value"].clone())
    } else {
        Err(error_code(&reply))
    }
}

fn error_code(reply: &Value) -> String {
    reply["error"]["code"]
        .as_str()
        .unwrap_or("feedback-failed")
        .to_owned()
}

fn apply_mutation(
    items: &mut HashMap<String, Item>,
    message: String,
    reply: Result<Value, String>,
) -> Result<(), String> {
    let reply = reply?;
    let update = |items: &mut HashMap<String, Item>, value: &Value| {
        if let Ok(item) = serde_json::from_value::<Item>(value.clone()) {
            if item.message_id == message {
                items.insert(message.clone(), item);
                return Ok(());
            }
        }
        Err("feedback-failed".to_string())
    };
    if reply["ok"] == true {
        if reply["value"]["absent"] == true {
            items.remove(&message);
            return Ok(());
        }
        return update(items, &reply["value"]);
    }
    let code = error_code(&reply);
    if code == "version-conflict" {
        if let Some(current) = reply["error"].get("current") {
            if current.is_null() {
                items.remove(&message);
            } else {
                update(items, current)?;
            }
        }
    }
    Err(code)
}
