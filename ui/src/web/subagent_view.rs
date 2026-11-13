//! Subagent drill-down: list the direct children one session spawned and open
//! a child's execution record read-only.
//!
//! The ACP host refuses to resume a `subagent` session (`session is not
//! resumable: subagent`), so a child is opened the same way KWork already shows
//! a session another instance holds: load its durable log and render it without
//! a live ACP session. The parent stays parked with its live turn intact, so
//! returning is a plain conversation switch.

use super::*;

/// One read-only child view: which parent to return to and whose record it is.
#[derive(Clone)]
pub(super) struct SubagentView {
    parent: SessionSummary,
    label: String,
}

impl DshApp {
    /// Open one catalog child's transcript read-only.
    pub(super) fn open_subagent_child(
        &mut self,
        child_id: &str,
        label: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(parent_id) = self.active_session.clone() else {
            return;
        };
        // An external task has no local transcript to read.
        let Some(parent) = self
            .sessions
            .iter()
            .find(|session| session.id == parent_id)
            .cloned()
        else {
            return;
        };
        if self.subagent_view.is_none() {
            self.subagent_view = Some(SubagentView {
                parent,
                label: label.to_owned(),
            });
        }
        let summary = SessionSummary {
            id: child_id.to_owned(),
            cwd: self.workspace.clone(),
            title: Some(label.to_owned()),
            blank: false,
            created_at_ms: None,
        };
        // `reply: None` means no live ACP session: history-only, read-only.
        self.open_conversation(summary, None, true, None, w, cx);
        cx.notify();
    }

    /// Return from a child transcript to the parked parent conversation.
    pub(super) fn close_subagent_view(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.subagent_view.take() else {
            return;
        };
        self.resume(view.parent, w, cx);
    }

    /// Expand or collapse the composer's subagent list.
    pub(super) fn toggle_subagent_list(&mut self, cx: &mut Context<Self>) {
        self.conversation.subagents_expanded = !self.conversation.subagents_expanded;
        cx.notify();
    }

    /// Label of the child currently open, if any.
    pub(super) fn subagent_view_label(&self) -> Option<&str> {
        self.subagent_view.as_ref().map(|view| view.label.as_str())
    }
}
