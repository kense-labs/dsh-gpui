//! Caps for the expanded tool-row detail.
//!
//! A tool result or argument payload can be megabytes (a long build log, a
//! generated file dump). Laying the whole string out as one GPUI text run makes
//! the transcript janky and keeps the whole payload in the layout cache, so the
//! expanded row shows a bounded prefix plus the official truncation note.

/// Characters one expanded argument or output section may show.
pub const TOOL_DETAIL_LIMIT: usize = 20_000;

/// Text as the expanded row should draw it.
pub(crate) struct DetailText {
    /// The prefix to draw (the whole text when it fits).
    pub(crate) shown: String,
    /// Total characters of the original text.
    pub(crate) total: usize,
}

impl DetailText {
    /// Whether the row must append the truncation note.
    pub(crate) fn truncated(&self) -> bool {
        self.total > TOOL_DETAIL_LIMIT
    }
}

/// Bound one detail section, splitting on a character boundary.
pub(crate) fn detail_text(text: &str) -> DetailText {
    let total = text.chars().count();
    if total <= TOOL_DETAIL_LIMIT {
        return DetailText {
            shown: text.to_owned(),
            total,
        };
    }
    DetailText {
        shown: text.chars().take(TOOL_DETAIL_LIMIT).collect(),
        total,
    }
}

/// The note an expanded row shows under a truncated section.
pub(crate) fn truncation_note(total: usize) -> String {
    format!("… 已截断，共 {total} 字符")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_sections_are_drawn_whole() {
        let detail = detail_text("cargo test\nok");
        assert!(!detail.truncated());
        assert_eq!(detail.shown, "cargo test\nok");
        assert_eq!(detail.total, 13);
    }

    #[test]
    fn long_sections_are_capped_on_a_character_boundary() {
        // Multi-byte characters must not be split; the note counts characters.
        let text = "测".repeat(TOOL_DETAIL_LIMIT + 5);
        let detail = detail_text(&text);
        assert!(detail.truncated());
        assert_eq!(detail.shown.chars().count(), TOOL_DETAIL_LIMIT);
        assert_eq!(detail.total, TOOL_DETAIL_LIMIT + 5);
        assert_eq!(
            truncation_note(detail.total),
            format!("… 已截断，共 {} 字符", TOOL_DETAIL_LIMIT + 5)
        );
    }

    #[test]
    fn exactly_the_limit_is_not_truncated() {
        let detail = detail_text(&"a".repeat(TOOL_DETAIL_LIMIT));
        assert!(!detail.truncated());
        assert_eq!(detail.shown.chars().count(), TOOL_DETAIL_LIMIT);
    }
}
