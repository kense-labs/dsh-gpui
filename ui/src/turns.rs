//! Turn navigation: one jump mark per Turn of the rendered transcript.
//!
//! Official ChatView renders a TurnNavigator rail beside the transcript once a
//! session has more than one turn: each mark jumps to its turn and a hover
//! preview shows that turn's prompt and latest reply. The rail is derived from
//! the same rows the transcript renders, so what looks like a turn here is
//! exactly one user input plus everything until the next input.

use crate::state::{ChatEntry, EntryRole};

/// One navigable Turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnItem {
    /// 1-based turn number shown in the preview.
    pub turn: u64,
    /// Row key of the turn's opening input (the measured anchor to jump to).
    pub anchor: String,
    /// Opening input, reduced to one short line.
    pub prompt: String,
    /// Latest non-empty assistant reply of the turn.
    pub response: String,
}

/// Collapse one row's text to a single trimmed, ellipsized line.
fn single_line(text: &str, limit: usize) -> String {
    let Some(line) = text.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return String::new();
    };
    let mut chars = line.chars();
    let prefix: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

/// Build the rail items from transcript rows in display order.
///
/// A `User`-role row opens a turn; trigger rows are user-role too and carry the
/// waking notice, which is the closest thing that turn has to a prompt. The
/// response is the turn's last non-empty assistant reply, so a still-running
/// turn previews whatever it has said so far.
pub fn turn_items<'a>(entries: impl IntoIterator<Item = &'a ChatEntry>) -> Vec<TurnItem> {
    let mut items: Vec<TurnItem> = Vec::new();
    for entry in entries {
        match entry.role {
            EntryRole::User => {
                items.push(TurnItem {
                    turn: items.len() as u64 + 1,
                    anchor: entry.key.clone(),
                    prompt: single_line(&entry.text, 120),
                    response: String::new(),
                });
            }
            EntryRole::Assistant => {
                let Some(item) = items.last_mut() else {
                    continue;
                };
                let reply = single_line(&entry.text, 160);
                if !reply.is_empty() {
                    item.response = reply;
                }
            }
            _ => {}
        }
    }
    items
}

/// Index of the turn the reader is looking at.
///
/// Official ChatView picks the latest turn while the viewport is parked at the
/// bottom and otherwise the turn at the top of the viewport:
///
/// ```text
/// activeTurn = nearBottom ? latestTurn : readVisibleTurn(metrics)
/// ```
///
/// `tops` are the per-turn anchor offsets from the transcript measurement pass;
/// `None` means the anchor has not been measured yet and cannot win. The
/// `slack` keeps a turn active while its opening row is just above the fold.
pub fn active_turn(
    tops: &[Option<f32>],
    scroll_top: f32,
    slack: f32,
    near_bottom: bool,
) -> Option<usize> {
    if near_bottom && !tops.is_empty() {
        return Some(tops.len() - 1);
    }
    let mut active = None;
    for (index, top) in tops.iter().enumerate() {
        let Some(top) = top else {
            continue;
        };
        if *top <= scroll_top + slack {
            active = Some(index);
        } else {
            break;
        }
    }
    active
}

/// Preview lines for one rail mark: prompt plus latest reply.
pub fn preview_lines(item: &TurnItem) -> (String, String) {
    (item.prompt.clone(), item.response.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(role: EntryRole, text: &str) -> ChatEntry {
        ChatEntry::local(role, format!("k-{}", text), text)
    }

    fn trigger(text: &str) -> ChatEntry {
        let mut row = ChatEntry::local(EntryRole::User, format!("t-{text}"), text);
        row.title = "goal".to_owned();
        row
    }

    #[test]
    fn each_user_input_opens_one_turn() {
        let rows = [
            entry(EntryRole::SystemPrompt, "system"),
            trigger("目标第一轮"),
            entry(EntryRole::Assistant, "第一轮回答"),
            entry(EntryRole::Tool, "tool row"),
            entry(EntryRole::User, "第二个问题"),
            entry(EntryRole::Assistant, "第二个回答"),
        ];
        let items = turn_items(rows.iter());
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].turn, 1);
        assert_eq!(items[0].anchor, "t-目标第一轮");
        assert_eq!(items[0].prompt, "目标第一轮");
        assert_eq!(items[0].response, "第一轮回答");
        assert_eq!(items[1].turn, 2);
        assert_eq!(items[1].anchor, "k-第二个问题");
        assert_eq!(items[1].prompt, "第二个问题");
        assert_eq!(items[1].response, "第二个回答");
    }

    #[test]
    fn the_preview_keeps_the_last_reply_of_the_turn() {
        let rows = [
            entry(EntryRole::User, "问题"),
            entry(EntryRole::Assistant, "过程说明"),
            entry(EntryRole::Tool, "tool"),
            entry(EntryRole::Assistant, "最终回答"),
        ];
        let items = turn_items(rows.iter());
        assert_eq!(items[0].response, "最终回答");
        // A running turn previews whatever it has said so far.
        let rows = [
            entry(EntryRole::User, "问题"),
            entry(EntryRole::Assistant, "半句话"),
        ];
        assert_eq!(turn_items(rows.iter())[0].response, "半句话");
    }

    #[test]
    fn previews_are_single_trimmed_lines() {
        let rows = [
            entry(EntryRole::User, "\n  第一行\n第二行"),
            entry(EntryRole::Assistant, "   "),
        ];
        let items = turn_items(rows.iter());
        assert_eq!(items[0].prompt, "第一行");
        assert_eq!(items[0].response, "");
        let long = "字".repeat(130);
        let rows = [entry(EntryRole::User, &long)];
        assert_eq!(turn_items(rows.iter())[0].prompt.chars().count(), 121);
    }

    #[test]
    fn the_active_turn_follows_the_top_of_the_viewport() {
        let tops = [Some(0.), Some(400.), Some(900.), None];
        assert_eq!(active_turn(&tops, 0., 8., false), Some(0));
        assert_eq!(active_turn(&tops, 30., 8., false), Some(0));
        assert_eq!(active_turn(&tops, 404., 8., false), Some(1));
        assert_eq!(active_turn(&tops, 1200., 8., false), Some(2));
        // Before the first measured anchor nothing is active.
        assert_eq!(active_turn(&[None, Some(10.)], 0., 8., false), None);
        // Parked at the bottom the latest turn wins even though an earlier turn
        // still owns the top of the viewport.
        assert_eq!(active_turn(&tops, 4000., 8., true), Some(3));
    }

    #[test]
    fn the_latest_turn_is_active_at_the_bottom_even_before_measuring() {
        let tops = [None, None, None];
        assert_eq!(active_turn(&tops, 0., 8., true), Some(2));
        assert_eq!(active_turn(&[], 0., 8., true), None);
    }

    #[test]
    fn no_rows_means_no_rail() {
        assert!(turn_items(std::iter::empty()).is_empty());
    }
}
