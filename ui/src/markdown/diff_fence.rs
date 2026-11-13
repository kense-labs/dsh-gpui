//! Official unified-diff fence parsing (`diff` / `patch` code blocks).
//!
//! Mirrors the rc.3 `Markdown` diff fence: a fence whose info string is `diff`
//! or `patch` renders as diff rows instead of grammar-highlighted plain code.
//! The marker column is split off so `+`/`-` keep their row colors while the
//! body tokenizes with the language each `---`/`+++` file header names.

use crate::markdown::{CodeSpan, code_spans};
use std::collections::HashMap;

/// One parsed diff line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffRow {
    pub(crate) kind: DiffKind,
    /// `+`, `-`, ` ` on content rows; empty on chrome rows.
    pub(crate) marker: &'static str,
    /// Content after the marker column (content rows only).
    pub(crate) body: String,
    /// The full source line (chrome rows render it verbatim).
    pub(crate) raw: String,
    /// Index of the file section this row belongs to; -1 before the first header.
    pub(crate) file: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiffKind {
    Add,
    Del,
    Context,
    Hunk,
    Meta,
}

/// Parsed fence: rows plus the path each file section names.
#[derive(Debug, Default)]
pub(crate) struct DiffFence {
    pub(crate) rows: Vec<DiffRow>,
    /// Header path per file section (still `a/`-prefixed), for language lookup.
    pub(crate) paths: HashMap<i32, String>,
}

impl DiffKind {
    /// Whether the row carries code the body highlighter tokenizes.
    pub(crate) fn is_content(self) -> bool {
        matches!(self, Self::Add | Self::Del | Self::Context)
    }
}

/// File-level chrome a unified diff carries above and between hunks.
const META_PREFIXES: &[&str] = &[
    "diff --git ",
    "index ",
    "old mode ",
    "new mode ",
    "new file mode ",
    "deleted file mode ",
    "similarity index ",
    "dissimilarity index ",
    "rename from ",
    "rename to ",
    "copy from ",
    "copy to ",
    "Binary files ",
    "GIT binary patch",
    "\\ No newline",
];

fn is_meta(raw: &str) -> bool {
    META_PREFIXES.iter().any(|prefix| raw.starts_with(prefix))
}

/// Parse a hunk header's line budget (`@@ -a,b +c,d @@`).
fn hunk_remainder(raw: &str) -> Option<(i64, i64)> {
    let rest = raw.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(' ')?;
    let rest = rest.strip_prefix('+')?;
    let (new, _) = rest.split_once(" @@")?;
    let side = |text: &str| -> Option<i64> {
        let value = text.split_once(',').map_or(text, |(count, _)| count);
        value.parse::<i64>().ok()
    };
    let old = side(old);
    let new = side(new);
    old.zip(new)
}

/// The path a `---`/`+++` header names, still as written (`a/src/lib.rs`).
pub(crate) fn header_path(raw: &str) -> Option<String> {
    let path = raw.get(4..)?;
    let path = path.split('\t').next().unwrap_or(path).trim();
    let path = if path.len() >= 2 && path.starts_with('"') && path.ends_with('"') {
        path[1..path.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        path.to_string()
    };
    if path.is_empty() || path == "/dev/null" {
        return None;
    }
    Some(path)
}

/// Split a unified diff into rows without losing a byte.
pub(crate) fn parse_diff_fence(code: &str) -> DiffFence {
    let mut fence = DiffFence::default();
    let mut in_hunk = false;
    let mut remainder: Option<(i64, i64)> = None;
    let mut file: i32 = -1;
    for raw in code.split('\n') {
        let raw_owned = raw.to_string();
        if raw.starts_with("diff --git ") {
            in_hunk = false;
            remainder = None;
            fence.rows.push(DiffRow {
                kind: DiffKind::Meta,
                marker: "",
                body: String::new(),
                raw: raw_owned,
                file,
            });
            continue;
        }
        if raw.starts_with("@@") {
            in_hunk = true;
            remainder = hunk_remainder(raw);
            fence.rows.push(DiffRow {
                kind: DiffKind::Hunk,
                marker: "",
                body: String::new(),
                raw: raw_owned,
                file,
            });
            continue;
        }
        // A file header carries a marker column but names the file, not a line.
        if !in_hunk && (raw.starts_with("--- ") || raw.starts_with("+++ ")) {
            if raw.starts_with("--- ") {
                file += 1;
            }
            if let Some(path) = header_path(raw) {
                fence.paths.insert(file, path);
            }
            fence.rows.push(DiffRow {
                kind: DiffKind::Meta,
                marker: "",
                body: String::new(),
                raw: raw_owned,
                file,
            });
            continue;
        }
        if is_meta(raw) {
            fence.rows.push(DiffRow {
                kind: DiffKind::Meta,
                marker: "",
                body: String::new(),
                raw: raw_owned,
                file,
            });
            continue;
        }
        let (kind, marker, body) = if let Some(rest) = raw.strip_prefix('+') {
            if let Some(remainder) = remainder.as_mut() {
                remainder.1 -= 1;
            }
            (DiffKind::Add, "+", rest.to_string())
        } else if let Some(rest) = raw.strip_prefix('-') {
            if let Some(remainder) = remainder.as_mut() {
                remainder.0 -= 1;
            }
            (DiffKind::Del, "-", rest.to_string())
        } else if let Some(rest) = raw.strip_prefix(' ') {
            if let Some(remainder) = remainder.as_mut() {
                remainder.0 -= 1;
                remainder.1 -= 1;
            }
            (DiffKind::Context, " ", rest.to_string())
        } else {
            // Authored diffs may omit the context marker column.
            (DiffKind::Context, "", raw.to_string())
        };
        fence.rows.push(DiffRow {
            kind,
            marker,
            body,
            raw: raw_owned,
            file,
        });
        if in_hunk
            && let Some((old, new)) = remainder
            && old <= 0
            && new <= 0
        {
            in_hunk = false;
        }
    }
    fence
}

/// Token spans for every content row, keyed by row index; body-local byte ranges.
///
/// Each file's old side (context + deletions) and new side (context + additions)
/// tokenize separately, so an unterminated comment in a removed line cannot
/// color the added lines and one file's grammar never applies to the next.
pub(crate) fn diff_row_spans(
    fence: &DiffFence,
    language_of: impl Fn(&str) -> Option<&'static str>,
) -> HashMap<usize, Vec<CodeSpan>> {
    struct Group<'a> {
        rows: Vec<(usize, &'a str, DiffKind)>,
    }
    let mut groups: HashMap<(i32, bool), Group<'_>> = HashMap::new();
    for (index, row) in fence.rows.iter().enumerate() {
        if !row.kind.is_content() {
            continue;
        }
        let sides: &[bool] = match row.kind {
            DiffKind::Del => &[true],
            DiffKind::Add => &[false],
            _ => &[true, false],
        };
        for &old_side in sides {
            groups
                .entry((row.file, old_side))
                .or_insert_with(|| Group { rows: Vec::new() })
                .rows
                .push((index, row.body.as_str(), row.kind));
        }
    }
    let mut spans: HashMap<usize, Vec<CodeSpan>> = HashMap::new();
    for ((file, _old_side), group) in groups {
        let Some(language) = fence.paths.get(&file).and_then(|path| language_of(path)) else {
            continue;
        };
        let joined = group
            .rows
            .iter()
            .map(|(_, body, _)| *body)
            .collect::<Vec<_>>()
            .join("\n");
        let line_spans = code_spans(&joined, language);
        let mut line_starts = Vec::with_capacity(group.rows.len());
        let mut offset = 0usize;
        for (_, body, _) in &group.rows {
            line_starts.push(offset);
            offset += body.len() + 1;
        }
        for (line_index, (row_index, body, kind)) in group.rows.iter().enumerate() {
            // A context row's old-side run colors the row; the new side would
            // duplicate the same text.
            if kind == &DiffKind::Context && !_old_side {
                continue;
            }
            let start = line_starts[line_index];
            let end = start + body.len();
            let mut row_spans = Vec::new();
            for span in &line_spans {
                if span.end <= start || span.start >= end {
                    continue;
                }
                row_spans.push(CodeSpan {
                    start: span.start.max(start) - start,
                    end: span.end.min(end) - start,
                    kind: span.kind,
                });
            }
            spans.entry(*row_index).or_insert(row_spans);
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::CodeToken;

    #[test]
    fn parses_a_git_diff_with_hunk_budget_and_file_headers() {
        let code = "diff --git a/src/lib.rs b/src/lib.rs\nindex 111..222 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,3 @@\n context\n-old line\n+new line\n+added line\n\\ No newline at end of file";
        let fence = parse_diff_fence(code);
        let kinds: Vec<DiffKind> = fence.rows.iter().map(|row| row.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DiffKind::Meta,
                DiffKind::Meta,
                DiffKind::Meta,
                DiffKind::Meta,
                DiffKind::Hunk,
                DiffKind::Context,
                DiffKind::Del,
                DiffKind::Add,
                DiffKind::Add,
                DiffKind::Meta,
            ]
        );
        assert_eq!(
            fence.paths.get(&0).map(String::as_str),
            Some("b/src/lib.rs")
        );
        let add = &fence.rows[7];
        assert_eq!(add.marker, "+");
        assert_eq!(add.body, "new line");
        assert_eq!(add.file, 0);
        assert_eq!(fence.rows[5].marker, " ");
        assert_eq!(fence.rows[5].body, "context");
    }

    #[test]
    fn a_finished_hunk_makes_a_following_file_header_chrome_again() {
        // Two files concatenated without `diff --git` lines, exactly like
        // `diff -u old new` output.
        let code = "--- a/one.txt\n+++ b/one.txt\n@@ -1 +1 @@\n-a\n+b\n--- a/two.txt\n+++ b/two.txt\n@@ -1 +1 @@\n-c\n+d";
        let fence = parse_diff_fence(code);
        let kinds: Vec<DiffKind> = fence.rows.iter().map(|row| row.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DiffKind::Meta,
                DiffKind::Meta,
                DiffKind::Hunk,
                DiffKind::Del,
                DiffKind::Add,
                DiffKind::Meta,
                DiffKind::Meta,
                DiffKind::Hunk,
                DiffKind::Del,
                DiffKind::Add,
            ]
        );
        assert_eq!(fence.paths.get(&0).map(String::as_str), Some("b/one.txt"));
        assert_eq!(fence.paths.get(&1).map(String::as_str), Some("b/two.txt"));
        assert_eq!(fence.rows[9].file, 1);
        assert_eq!(fence.rows[9].body, "d");
    }

    #[test]
    fn authored_diffs_without_markers_stay_context_rows() {
        let fence = parse_diff_fence("just text\n+added\n  indented");
        assert_eq!(fence.rows[0].kind, DiffKind::Context);
        assert_eq!(fence.rows[0].marker, "");
        assert_eq!(fence.rows[0].body, "just text");
        assert_eq!(fence.rows[1].kind, DiffKind::Add);
        assert_eq!(fence.rows[2].body, " indented");
    }

    #[test]
    fn language_and_token_spans_follow_the_file_headers() {
        let code = "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,2 +1,2 @@\n fn main() {\n-    let x = 1;\n+    let y = 2;\n }";
        let fence = parse_diff_fence(code);
        let language_for = |path: &str| {
            if path.ends_with(".rs") {
                Some("rust")
            } else {
                None
            }
        };
        let spans = diff_row_spans(&fence, language_for);
        // The deleted and added lines are tokenized; the chrome rows are not.
        assert!(spans.contains_key(&4));
        assert!(spans.contains_key(&5));
        assert!(!spans.contains_key(&2));
        assert!(!spans.contains_key(&1));
        // `let` is a rust keyword in both sides.
        let del = &spans[&4];
        assert!(del.iter().any(|span| span.kind == CodeToken::Keyword));
        let add = &spans[&5];
        assert!(
            add.iter()
                .any(|span| span.kind == CodeToken::Number || span.kind == CodeToken::Keyword)
        );
    }
}
