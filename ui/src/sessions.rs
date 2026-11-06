//! Local projection of ACP sessions, which omit active rows and title metadata.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionSummary {
    pub(crate) id: String,
    pub(crate) cwd: String,
    pub(crate) title: Option<String>,
    pub(crate) blank: bool,
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
        if let Some(existing) = known
            .iter()
            .find(|existing| existing.id == session.id && existing.title.is_some())
        {
            session.title = existing.title.clone();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, title: Option<&str>, blank: bool) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            cwd: "/work".into(),
            title: title.map(str::to_string),
            blank,
        }
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
