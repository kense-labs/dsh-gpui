//! Durable goal state for the composer's GoalBar.
//!
//! Official `ui-goal` docks a strip above the input showing the current goal's
//! phase, its objective, and the pause/resume/edit/clear actions. The durable
//! phase comes from the `goal` projection; activation (whether this process may
//! auto-continue) is process-local and rides on the same live view.
//!
//! Everything here is pure so the phase labels and action availability stay
//! host-testable; the view itself is only a JSON reader.

use serde_json::Value;

/// Durable continuation phase (`GoalPhase` in the goal domain).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalPhase {
    Active,
    Paused,
    Blocked,
    Complete,
}

/// Whether the live process may automatically continue an active goal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalActivation {
    Armed,
    Disarmed,
    /// The activation projection has not arrived (or named an unknown value):
    /// the pause/resume toggle stays disabled until it is known.
    Unknown,
}

/// One live goal view: durable snapshot plus replay counters and activation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalView {
    pub id: String,
    pub revision: u64,
    pub objective: String,
    pub phase: GoalPhase,
    pub activation: GoalActivation,
    /// Present exactly while the phase is `blocked`.
    pub blocked_reason: Option<String>,
}

impl GoalView {
    /// Read one `GoalView` from the Host's JSON. Unknown phases are dropped: a
    /// future phase must not render with a wrong label.
    pub fn from_json(value: &Value) -> Option<Self> {
        let id = value["id"].as_str().filter(|id| !id.is_empty())?;
        let objective = value["objective"].as_str().unwrap_or_default();
        let phase = match value["phase"].as_str()? {
            "active" => GoalPhase::Active,
            "paused" => GoalPhase::Paused,
            "blocked" => GoalPhase::Blocked,
            "complete" => GoalPhase::Complete,
            _ => return None,
        };
        let activation = match value["activation"].as_str() {
            Some("armed") => GoalActivation::Armed,
            Some("disarmed") => GoalActivation::Disarmed,
            // rc.3 keeps the toggle disabled until activation is known instead
            // of assuming the process may continue.
            _ => GoalActivation::Unknown,
        };
        Some(Self {
            id: id.to_owned(),
            revision: value["revision"].as_u64().unwrap_or(0),
            objective: objective.to_owned(),
            phase,
            activation,
            blocked_reason: value
                .pointer("/blockedReason/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .filter(|message| !message.trim().is_empty()),
        })
    }

    /// Whether the strip renders at all; a completed goal renders nothing.
    pub fn visible(&self) -> bool {
        self.phase != GoalPhase::Complete
    }

    /// Durable phase label, matching the official PhaseLabels dictionary.
    pub fn label(&self) -> &'static str {
        match (self.phase, self.activation) {
            (GoalPhase::Active, GoalActivation::Disarmed) => "待继续的目标",
            (GoalPhase::Active, _) => "进行中的目标",
            (GoalPhase::Paused, _) => "已暂停的目标",
            (GoalPhase::Blocked, _) => "受阻的目标",
            (GoalPhase::Complete, _) => "",
        }
    }

    /// Pause is offered only while the process may still continue the goal.
    pub fn can_pause(&self) -> bool {
        self.phase == GoalPhase::Active && self.activation == GoalActivation::Armed
    }

    /// Resume is offered for every phase that is not currently running.
    pub fn can_resume(&self) -> bool {
        self.phase != GoalPhase::Active || self.activation == GoalActivation::Disarmed
    }

    /// rc.3: the pause/resume toggle pauses only an armed active goal and
    /// resumes every other phase; it is disabled while activation is unknown.
    pub fn can_toggle(&self) -> bool {
        self.can_pause() || self.can_resume()
    }

    /// Hover text: the blocked reason when the goal is blocked.
    pub fn title(&self) -> Option<&str> {
        self.blocked_reason.as_deref()
    }

    /// Single-line objective preview; the strip ellipsizes anything longer.
    pub fn objective_preview(&self, limit: usize) -> String {
        let single_line = self.objective.lines().next().unwrap_or_default().trim();
        let mut chars = single_line.chars();
        let prefix: String = chars.by_ref().take(limit).collect();
        if chars.next().is_some() {
            format!("{prefix}…")
        } else {
            prefix
        }
    }
}

/// Apply one Host goal view (or its absence) to the current strip state.
///
/// A revision that moves backwards means the Session was reopened with a
/// different goal; the Host view is authoritative either way, so the new value
/// simply replaces the old one.
pub fn apply_goal_view(current: &mut Option<GoalView>, next: Option<GoalView>) {
    *current = next.filter(GoalView::visible);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn view(phase: &str, activation: &str) -> GoalView {
        GoalView::from_json(&json!({
            "id": "goal-1", "revision": 3, "objective": "让 KWork 完全替代 DSH",
            "phase": phase, "activation": activation, "maxGoalRounds": 256,
            "roundsStarted": 2,
        }))
        .expect("goal view")
    }

    #[test]
    fn phases_and_activation_pick_the_official_labels() {
        assert_eq!(view("active", "armed").label(), "进行中的目标");
        assert_eq!(view("active", "disarmed").label(), "待继续的目标");
        // Missing/unknown activation keeps the active label but disables the toggle.
        assert_eq!(view("active", "unknown").label(), "进行中的目标");
        assert!(!view("active", "unknown").can_toggle());
        assert_eq!(view("paused", "disarmed").label(), "已暂停的目标");
        assert_eq!(view("blocked", "disarmed").label(), "受阻的目标");
        assert!(!view("complete", "disarmed").visible());
    }

    #[test]
    fn only_an_armed_active_goal_can_pause() {
        assert!(view("active", "armed").can_pause());
        assert!(!view("active", "disarmed").can_pause());
        assert!(!view("paused", "disarmed").can_pause());
        assert!(!view("blocked", "armed").can_pause());
    }

    #[test]
    fn every_non_running_phase_can_resume() {
        assert!(!view("active", "armed").can_resume());
        assert!(view("active", "disarmed").can_resume());
        assert!(view("paused", "armed").can_resume());
        assert!(view("blocked", "armed").can_resume());
        assert!(view("complete", "armed").can_resume());
    }

    #[test]
    fn unknown_phases_and_missing_identities_are_dropped() {
        assert!(GoalView::from_json(&json!({"phase": "active"})).is_none());
        assert!(GoalView::from_json(&json!({"id": "g", "phase": "archived"})).is_none());
        assert!(GoalView::from_json(&json!(null)).is_none());
    }

    #[test]
    fn blocked_goals_carry_the_reason_and_the_rest_do_not() {
        let blocked = GoalView::from_json(&json!({
            "id": "g", "phase": "blocked", "activation": "disarmed",
            "objective": "x", "blockedReason": {"code": "tool-denied", "message": "沙箱拒绝访问"},
        }))
        .expect("blocked goal");
        assert_eq!(blocked.title(), Some("沙箱拒绝访问"));
        assert_eq!(view("active", "armed").title(), None);
    }

    #[test]
    fn the_preview_keeps_one_line_and_marks_a_truncation() {
        let mut goal = view("active", "armed");
        goal.objective = "第一行\n第二行".to_owned();
        assert_eq!(goal.objective_preview(96), "第一行");
        goal.objective = "一二三四五六".to_owned();
        assert_eq!(goal.objective_preview(4), "一二三四…");
        assert_eq!(goal.objective_preview(6), "一二三四五六");
    }

    #[test]
    fn a_completed_goal_clears_the_strip_and_a_live_one_restores_it() {
        let mut current = Some(view("active", "armed"));
        apply_goal_view(&mut current, Some(view("complete", "armed")));
        assert!(current.is_none());
        apply_goal_view(&mut current, Some(view("paused", "disarmed")));
        assert_eq!(current.as_ref().map(GoalView::label), Some("已暂停的目标"));
        apply_goal_view(&mut current, None);
        assert!(current.is_none());
    }
}
