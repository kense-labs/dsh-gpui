//! Pending Inbox rows: the messages waiting in the Host for the running or the
//! next turn.
//!
//! Official `QueueDock` keeps one row per durable Inbox item and lets the user
//! edit, remove, or steer it. The rows are Host-owned here too: the client only
//! mirrors what `_dsh/gpui/session/inbox-state` reports and drops a row when the
//! Host admits it, so a queued message survives closing the app.

use super::*;

impl DshApp {
    /// Ask the Host for the authoritative pending list of one session.
    ///
    /// The locally allocated sequence travels with the request and comes back
    /// inside the reply, so a slow snapshot cannot delete a row that a newer
    /// submission added meanwhile.
    pub(super) fn request_inbox_state(&mut self, session: &str) {
        let Some(conversation) = self.conversation_mut(session) else {
            return;
        };
        let issued = conversation.inbox_snapshot_request();
        invoke_with_reply(
            "inbox_state",
            "inbox_state",
            json!({"sessionId": session, "requestSeq": issued}),
        );
    }

    /// Apply one `inbox_state` reply.
    pub(super) fn finish_inbox_state(
        &mut self,
        owner: Option<&str>,
        result: Result<Value, String>,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(reply) = result else {
            // A failed snapshot keeps the mirrored rows: they are still owned by
            // the Host, and the next mutation refreshes them anyway.
            return;
        };
        let issued = reply["requestSeq"].as_u64().unwrap_or(0);
        let items = reply["items"].as_array().cloned().unwrap_or_default();
        let Some(conversation) = owner.and_then(|session| self.conversation_mut(session)) else {
            return;
        };
        conversation.apply_inbox_snapshot(&items, issued);
        cx.notify();
    }

    /// Render one user message the Host logged.
    ///
    /// A message the client submitted already has its optimistic transcript row;
    /// the durable identity only has to be adopted. A message admitted from the
    /// Inbox (steering, or a queue row that just became a turn) has no local row
    /// yet and must be appended at its real position in the log.
    pub(super) fn apply_logged_user_message(
        &mut self,
        session: &str,
        update: &Value,
        cx: &mut Context<Self>,
    ) {
        let Some(message_id) = update["messageId"].as_str() else {
            return;
        };
        let text = update["content"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_owned();
        let has_image = update["hasImage"].as_bool().unwrap_or(false);
        let at = update["time"].as_f64().unwrap_or_else(bridge::now_ms);
        let selected = self.active_session.as_deref() == Some(session);
        let Some(conversation) = self.conversation_mut(session) else {
            return;
        };
        conversation.admit_inbox_row(message_id);
        let display = pending_inbox_display(&text, has_image);
        let appended = crate::state::adopt_logged_user_message(
            &mut conversation.entries,
            message_id,
            display,
            Some(at),
        );
        if appended && selected {
            self.maybe_follow_tail();
            cx.notify();
        }
    }

    /// Current draft text for a pending row that the user takes back to the composer.
    fn queued_display(&self, index: usize) -> Option<String> {
        self.conversation
            .queued_prompts
            .get(index)
            .map(|row| row.display.clone())
    }

    /// Delete one pending row from the Host Inbox (official queue `remove`).
    pub(super) fn remove_queued_prompt(
        &mut self,
        index: usize,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(session), Some(item_id)) = (
            self.active_session.clone(),
            self.conversation
                .queued_prompts
                .get(index)
                .map(|row| row.item_id.clone()),
        ) else {
            return;
        };
        cx.spawn_in(w, async move |this, cx| {
            let result = bridge::invoke(
                "inbox_action",
                json!({"sessionId": session, "itemId": item_id, "action": "remove"}),
            )
            .await;
            let _ = this.update_in(cx, |this, _w, cx| {
                let Some(conversation) = this.conversation_mut(&session) else {
                    return;
                };
                match result {
                    Ok(_) => conversation
                        .queued_prompts
                        .retain(|row| row.item_id != item_id),
                    Err(error) => conversation.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Take one pending row back into the composer, matching KWork's existing
    /// "取回编辑" affordance: the Host row is removed, the text stays local.
    pub(super) fn edit_queued_prompt(
        &mut self,
        index: usize,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(display) = self.queued_display(index) else {
            return;
        };
        let (Some(session), Some(item_id)) = (
            self.active_session.clone(),
            self.conversation
                .queued_prompts
                .get(index)
                .map(|row| row.item_id.clone()),
        ) else {
            return;
        };
        // Remove locally first so a double click cannot duplicate the text.
        self.conversation
            .queued_prompts
            .retain(|row| row.item_id != item_id);
        replace(&self.composer, &display, w, cx);
        self.focus_composer(w, cx);
        cx.notify();
        cx.spawn_in(w, async move |this, cx| {
            let result = bridge::invoke(
                "inbox_action",
                json!({"sessionId": session, "itemId": item_id, "action": "remove"}),
            )
            .await;
            if let Err(error) = result {
                let _ = this.update_in(cx, |this, _w, cx| {
                    if let Some(conversation) = this.conversation_mut(&session) {
                        conversation.error = Some(error);
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Move one queued (next-turn) row into the running turn (official `steer`).
    pub(super) fn steer_queued_prompt(
        &mut self,
        index: usize,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(session), Some(item_id)) = (
            self.active_session.clone(),
            self.conversation
                .queued_prompts
                .get(index)
                .map(|row| row.item_id.clone()),
        ) else {
            return;
        };
        cx.spawn_in(w, async move |this, cx| {
            let result = bridge::invoke(
                "inbox_action",
                json!({"sessionId": session, "itemId": item_id, "action": "steer"}),
            )
            .await;
            let _ = this.update_in(cx, |this, _w, cx| {
                let Some(conversation) = this.conversation_mut(&session) else {
                    return;
                };
                match result {
                    Ok(_) => conversation.steer_inbox_row(&item_id),
                    Err(error) => {
                        conversation.error = Some(error);
                        // A lost race means the row may already be admitted;
                        // the Host list is the truth.
                        this.request_inbox_state(&session);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
