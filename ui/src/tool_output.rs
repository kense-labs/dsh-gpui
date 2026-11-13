//! Tool-result envelopes: the `<path>/<type>/<content>` wrapper read and write
//! tools put around their payload.
//!
//! The expanded row wants the payload, not the wire envelope, and the collapsed
//! summary must not start with `<path>…`. Bash and other plain-text results pass
//! through unchanged.

/// One tool result split into the envelope fields and its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolOutput {
    /// The envelope `<path>`, when the result wrapped one.
    pub(crate) path: Option<String>,
    /// The envelope `<type>`, when the result wrapped one.
    pub(crate) kind: Option<String>,
    /// The payload to show: envelope content, or the whole text for plain results.
    pub(crate) content: String,
}

/// Parse one tool-result text for display.
pub(crate) fn parse(text: &str) -> ToolOutput {
    let trimmed = text.trim();
    if !trimmed.contains("<content>")
        && !trimmed.starts_with("<path>")
        && !trimmed.starts_with("<type>")
    {
        return ToolOutput {
            path: None,
            kind: None,
            content: text.to_owned(),
        };
    }
    let path = field(trimmed, "path");
    let kind = field(trimmed, "type");
    let content = match (
        trimmed
            .find("<content>")
            .map(|index| index + "<content>".len()),
        trimmed.rfind("</content>"),
    ) {
        (Some(start), Some(end)) if end > start => trimmed[start..end].to_owned(),
        (Some(start), None) => trimmed[start..].to_owned(),
        _ => trimmed.to_owned(),
    };
    ToolOutput {
        path,
        kind,
        content: content.trim_matches('\n').to_owned(),
    }
}

/// Read one `<tag>value</tag>` field of the envelope.
fn field(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    let value = text[start..end].trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// The text an expanded row or its summary should show.
pub(crate) fn display(text: &str) -> String {
    parse(text).content
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_results_show_the_payload_not_the_envelope() {
        let text = "<path>/Users/leon/tmp/card-live.txt</path>\n<type>file</type>\n<content>\n1: hi\n\n(End of file - total 1 lines)\n</content>";
        let output = parse(text);
        assert_eq!(
            output.path.as_deref(),
            Some("/Users/leon/tmp/card-live.txt")
        );
        assert_eq!(output.kind.as_deref(), Some("file"));
        assert_eq!(output.content, "1: hi\n\n(End of file - total 1 lines)");
        assert_eq!(display(text), output.content);
    }

    #[test]
    fn write_results_drop_the_envelope_too() {
        let text =
            "<path>/tmp/a.txt</path>\n<type>file</type>\n<content>\nCreated file\n</content>";
        assert_eq!(display(text), "Created file");
    }

    #[test]
    fn plain_results_pass_through_unchanged() {
        assert_eq!(
            display("exit code: 0\nall good\n"),
            "exit code: 0\nall good\n"
        );
        assert_eq!(display(""), "");
        assert_eq!(parse("total 0\ndrwxr-xr-x").content, "total 0\ndrwxr-xr-x");
    }

    #[test]
    fn an_unclosed_envelope_keeps_the_rest() {
        let text = "<path>/tmp/a.txt</path>\n<content>\nline one\nline two";
        let output = parse(text);
        assert_eq!(output.path.as_deref(), Some("/tmp/a.txt"));
        assert_eq!(output.content, "line one\nline two");
    }
}
