//! Official chat history edge labels (`ChatView` 加载更早 状态机).

/// The five states the official history edge can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HistoryEdge {
    /// The first page is still loading: 载入历史…
    LoadingInitial,
    /// An older page is in flight: 正在加载.
    LoadingOlder,
    /// A previous page still exists and nothing is loading: 加载更早.
    More,
    /// The last request failed: 重新加载.
    Error,
    /// Every page arrived but the window was incomplete at some point:
    /// 已到会话开头.
    Start,
}

impl HistoryEdge {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::LoadingInitial => "载入历史…",
            Self::LoadingOlder => "正在加载",
            Self::More => "加载更早",
            Self::Error => "重新加载",
            Self::Start => "已到会话开头",
        }
    }

    /// Whether pressing the edge requests a page.
    pub(crate) fn actionable(self) -> bool {
        matches!(self, Self::More | Self::Error)
    }
}

/// Resolve the edge state from the client's history status flags.
pub(crate) fn history_edge(
    has_more: bool,
    loading: bool,
    prepend_pending: bool,
    error: bool,
    was_partial: bool,
) -> Option<HistoryEdge> {
    if error {
        Some(HistoryEdge::Error)
    } else if loading {
        Some(if prepend_pending {
            HistoryEdge::LoadingOlder
        } else {
            HistoryEdge::LoadingInitial
        })
    } else if has_more {
        Some(HistoryEdge::More)
    } else if was_partial {
        Some(HistoryEdge::Start)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_follows_the_official_five_states() {
        assert_eq!(history_edge(false, false, false, false, false), None);
        assert_eq!(
            history_edge(false, false, false, false, true),
            Some(HistoryEdge::Start)
        );
        assert_eq!(
            history_edge(true, false, false, false, false),
            Some(HistoryEdge::More)
        );
        assert_eq!(
            history_edge(true, true, false, false, true),
            Some(HistoryEdge::LoadingInitial)
        );
        assert_eq!(
            history_edge(true, true, true, false, true),
            Some(HistoryEdge::LoadingOlder)
        );
        assert_eq!(
            history_edge(true, true, true, true, true),
            Some(HistoryEdge::Error)
        );
        assert_eq!(HistoryEdge::More.label(), "加载更早");
        assert_eq!(HistoryEdge::LoadingOlder.label(), "正在加载");
        assert_eq!(HistoryEdge::LoadingInitial.label(), "载入历史…");
        assert_eq!(HistoryEdge::Error.label(), "重新加载");
        assert_eq!(HistoryEdge::Start.label(), "已到会话开头");
        assert!(HistoryEdge::More.actionable());
        assert!(HistoryEdge::Error.actionable());
        assert!(!HistoryEdge::Start.actionable());
        assert!(!HistoryEdge::LoadingOlder.actionable());
    }
}
