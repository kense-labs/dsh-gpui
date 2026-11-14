//! One-line summary text for the official `GenericCommandCard`.
//!
//! Official `GenericCommandCard` renders `outcome.text` inside a single-line
//! span: the browser collapses newlines and repeated whitespace into one space
//! before ellipsizing, while the full text stays available in the expandable
//! `<pre>` body. GPUI renders raw newlines as real lines, which turned a `/goal`
//! result into a multi-line block inside the collapsed row, so the collapsed
//! summary is normalized here and the body keeps the original text.

/// Collapse every whitespace run (including newlines and tabs) into one space.
pub(crate) fn command_summary(text: &str) -> String {
    let mut summary = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !summary.is_empty() && !summary.ends_with(' ') {
                summary.push(' ');
            }
        } else {
            summary.push(ch);
        }
    }
    summary.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::command_summary;

    #[test]
    fn newlines_and_runs_collapse_to_a_single_line() {
        let text = "Goal created\nStatus: active\nObjective: 帮我从1数到50\nRounds: 0/256\nActivation: armed\n\nCommands: /goal edit <objective>, /goal pause, /goal clear";
        assert_eq!(
            command_summary(text),
            "Goal created Status: active Objective: 帮我从1数到50 Rounds: 0/256 Activation: armed Commands: /goal edit <objective>, /goal pause, /goal clear"
        );
        assert_eq!(command_summary("a \t\r\n b   c"), "a b c");
        assert_eq!(
            command_summary("  leading and trailing  "),
            "leading and trailing"
        );
        assert_eq!(command_summary(""), "");
        assert_eq!(command_summary("\n\n"), "");
    }
}
