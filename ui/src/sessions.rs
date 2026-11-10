//! Local projection of ACP sessions, which omit active rows and title metadata.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionSummary {
    pub(crate) id: String,
    pub(crate) cwd: String,
    pub(crate) title: Option<String>,
    pub(crate) blank: bool,
    /// Unix epoch milliseconds from ACP `session/list` `createdAt`（官方相对时间）。
    pub(crate) created_at_ms: Option<u64>,
}

impl SessionSummary {
    pub(crate) fn title(&self) -> &str {
        self.title.as_deref().unwrap_or(if self.blank {
            "新会话"
        } else {
            "未命名会话"
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SessionIndicator {
    #[default]
    Idle,
    Running,
    Subagents(usize),
    Approval,
    PlanReview,
    Question,
    Unread,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingInteraction {
    Approval,
    PlanReview,
    Question,
}

#[derive(Default)]
pub(crate) struct SessionActivity {
    host_running: Option<bool>,
    unread: bool,
    pub(crate) last_prompt_at_ms: Option<u64>,
}

impl SessionActivity {
    pub(crate) fn observe_host(&mut self, running: bool, local_running: bool, selected: bool) {
        let previous = self.host_running.replace(running).unwrap_or(local_running);
        self.transition(previous, running, selected);
    }

    pub(crate) fn observe_local(&mut self, previous: bool, running: bool, selected: bool) {
        if self.host_running.is_none() {
            self.transition(previous, running, selected);
        }
    }

    fn transition(&mut self, previous: bool, running: bool, selected: bool) {
        if running || selected {
            self.unread = false;
        } else if previous {
            self.unread = true;
        }
    }

    pub(crate) fn acknowledge(&mut self) {
        self.unread = false;
    }

    pub(crate) fn indicator(
        &self,
        local_running: bool,
        pending: Option<PendingInteraction>,
        subagents: usize,
    ) -> SessionIndicator {
        if let Some(pending) = pending {
            match pending {
                PendingInteraction::Approval => SessionIndicator::Approval,
                PendingInteraction::PlanReview => SessionIndicator::PlanReview,
                PendingInteraction::Question => SessionIndicator::Question,
            }
        } else if self.host_running.unwrap_or(local_running) {
            SessionIndicator::Running
        } else if subagents > 0 {
            SessionIndicator::Subagents(subagents)
        } else if self.unread {
            SessionIndicator::Unread
        } else {
            SessionIndicator::Idle
        }
    }
}

pub(crate) fn short_session_title(text: &str) -> String {
    const LIMIT: usize = 80;
    let line = text.lines().next().unwrap_or(text).trim();
    let mut chars = line.chars();
    let title = chars.by_ref().take(LIMIT).collect::<String>();
    if chars.next().is_some() {
        format!("{title}…")
    } else {
        title
    }
}

pub(crate) fn merge_session_page(
    known: &mut Vec<SessionSummary>,
    incoming: impl IntoIterator<Item = SessionSummary>,
    active_id: Option<&str>,
) {
    let active = active_id.and_then(|id| {
        known
            .iter()
            .find(|session| session.id == id)
            .cloned()
            .or_else(|| {
                Some(SessionSummary {
                    id: id.to_string(),
                    cwd: String::new(),
                    title: None,
                    blank: true,
                    created_at_ms: None,
                })
            })
    });
    for mut session in incoming {
        if active
            .as_ref()
            .is_some_and(|active| active.id == session.id)
        {
            continue;
        }
        if let Some(existing) = known.iter().find(|existing| existing.id == session.id) {
            if existing.title.is_some() {
                session.title = existing.title.clone();
            }
            if session.created_at_ms.is_none() {
                session.created_at_ms = existing.created_at_ms;
            }
        }
        if let Some(existing) = known.iter_mut().find(|known| known.id == session.id) {
            *existing = session;
        } else {
            known.push(session);
        }
    }
    known.retain(|session| !session.blank || active.as_ref().is_some_and(|a| a.id == session.id));
    if let Some(active) = active {
        known.retain(|session| session.id != active.id);
        known.insert(0, active);
    }
}

/// Compact relative time used by the official workspace rows.
pub(crate) fn relative_session_age(updated_at_ms: u64, now_ms: u64) -> String {
    let delta = now_ms.saturating_sub(updated_at_ms);
    let minutes = delta / 60_000;
    if minutes < 1 {
        return "刚刚".into();
    }
    if minutes < 60 {
        return format!("{minutes}分钟");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}小时");
    }
    let days = hours / 24;
    if days < 30 {
        format!("{days}天")
    } else if days < 365 {
        format!("{}个月", days / 30)
    } else {
        format!("{}年", days / 365)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, title: Option<&str>, blank: bool) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            cwd: "/work".into(),
            title: title.map(str::to_string),
            blank,
            created_at_ms: None,
        }
    }

    #[test]
    fn relative_age_uses_the_official_buckets() {
        assert_eq!(relative_session_age(10, 0), "刚刚");
        assert_eq!(relative_session_age(0, 30_000), "刚刚");
        assert_eq!(relative_session_age(0, 3 * 60 * 60 * 1000), "3小时");
        assert_eq!(relative_session_age(0, 4 * 24 * 60 * 60 * 1000), "4天");
        assert_eq!(relative_session_age(0, 30 * 86_400_000), "1个月");
        assert_eq!(relative_session_age(0, 364 * 86_400_000), "12个月");
        assert_eq!(relative_session_age(0, 365 * 86_400_000), "1年");
    }

    #[test]
    fn unread_tracks_host_activity_and_yields_to_pending_work() {
        use SessionIndicator::*;
        let mut status = SessionActivity::default();
        status.observe_host(false, false, false);
        assert_eq!(status.indicator(false, None, 0), Idle);
        status.observe_host(true, false, false);
        assert_eq!(status.indicator(false, None, 0), Running);
        assert_eq!(
            status.indicator(false, Some(PendingInteraction::Approval), 1),
            Approval
        );
        assert_eq!(
            status.indicator(false, Some(PendingInteraction::PlanReview), 1),
            PlanReview
        );
        status.observe_host(false, true, false);
        assert_eq!(status.indicator(true, None, 2), Subagents(2));
        assert_eq!(status.indicator(true, None, 0), Unread);
        status.acknowledge();
        assert_eq!(status.indicator(false, None, 0), Idle);
        status.observe_host(true, false, true);
        status.observe_host(false, true, true);
        assert_eq!(status.indicator(false, None, 0), Idle);
    }

    #[test]
    fn local_completion_is_only_a_fallback_for_an_unobserved_agent() {
        use SessionIndicator::*;
        let mut status = SessionActivity::default();
        status.observe_local(true, false, false);
        assert_eq!(status.indicator(false, None, 0), Unread);
        status.observe_local(false, true, false);
        assert_eq!(status.indicator(true, None, 0), Running);
        status.observe_host(true, true, false);
        status.observe_local(true, false, false);
        assert_eq!(status.indicator(false, None, 0), Running);
    }

    #[test]
    fn merge_keeps_filtered_active_blank_and_real_titles() {
        let mut known = vec![
            session("active", None, true),
            session("old", Some("真实标题"), false),
        ];
        merge_session_page(
            &mut known,
            vec![session("old", None, false), session("history", None, false)],
            Some("active"),
        );
        assert_eq!(known[0], session("active", None, true));
        assert_eq!(known[0].title(), "新会话");
        assert_eq!(known[1].title(), "真实标题");
        assert_eq!(known[2].title(), "未命名会话");
    }

    #[test]
    fn merge_dedupes_pages_and_drops_stale_blanks() {
        let mut known = vec![session("blank", None, true)];
        merge_session_page(
            &mut known,
            vec![session("one", None, false), session("one", None, false)],
            None,
        );
        assert_eq!(known, vec![session("one", None, false)]);
    }

    #[test]
    fn first_message_title_uses_text_instead_of_an_id() {
        assert_eq!(
            short_session_title("  修复登录问题\n详细说明"),
            "修复登录问题"
        );
        assert_eq!(short_session_title(&"字".repeat(81)).chars().count(), 81);
    }
}
