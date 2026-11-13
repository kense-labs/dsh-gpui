//! Assistant 消息 Markdown 投影，对齐官方 ui-chat / ui-primitives 的常见块级与行内形态。
//! 覆盖：标题、段落、引用、分隔线、列表、表格、代码块、图片占位、粗体/行内代码/链接。

use std::ops::Range;

/// Byte ranges of the leading YAML and the Markdown body after its closing line.
pub(crate) fn frontmatter_bounds(text: &str) -> Option<(Range<usize>, usize)> {
    let (opening, remainder) = text.split_once('\n')?;
    let opening = opening.strip_prefix('\u{feff}').unwrap_or(opening);
    if marker(opening, true) != "---" {
        return None;
    }
    let start = text.len() - remainder.len();
    let mut offset = 0;
    for line in remainder.split_inclusive('\n') {
        let bare = line.strip_suffix('\n').unwrap_or(line);
        if matches!(marker(bare, bare.len() != line.len()), "---" | "...") {
            let source = &remainder[..offset];
            let source = source
                .strip_suffix("\r\n")
                .or_else(|| source.strip_suffix('\n'))
                .unwrap_or(source);
            return Some((start..start + source.len(), start + offset + line.len()));
        }
        offset += line.len();
    }
    None
}

fn marker(line: &str, terminated: bool) -> &str {
    let line = if terminated {
        line.strip_suffix('\r').unwrap_or(line)
    } else {
        line
    };
    line.trim_end_matches([' ', '\t'])
}

/// One block-level markdown element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Block {
    Paragraph(String),
    Heading(u8, String),
    Quote(String),
    Rule,
    /// 0 = 无序列表项；>0 = 有序序号。
    ListItem(u32, String),
    Code {
        lang: String,
        text: String,
    },
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// 图片：先以说明+路径占位，宿主侧 gallery 后续对齐。
    Image {
        alt: String,
        url: String,
    },
}

/// Destinations of every block-level image in the document, in order.
///
/// The transcript preloads absolute workspace paths before rendering, so a
/// reopened session never flashes an empty picture box.
pub(crate) fn image_destinations(source: &str) -> Vec<String> {
    parse_markdown(source)
        .into_iter()
        .filter_map(|block| match block {
            Block::Image { url, .. } => {
                let trimmed = url.trim();
                if let Some(inner) = trimmed
                    .strip_prefix('<')
                    .and_then(|rest| rest.strip_suffix('>'))
                {
                    Some(inner.to_owned())
                } else {
                    Some(trimmed.to_owned())
                }
            }
            _ => None,
        })
        .collect()
}

/// 行内片段：普通文本（可粗体/代码）或链接。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InlinePart {
    Text {
        text: String,
        bold: bool,
        code: bool,
    },
    Link {
        label: String,
        url: String,
    },
}

/// Parse assistant markdown into renderable blocks.
pub(crate) fn parse_markdown(source: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut ordered_index = 0u32;

    fn flush(blocks: &mut Vec<Block>, paragraph: &mut Vec<String>) {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(paragraph.join("\n")));
            paragraph.clear();
        }
    }

    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i].trim_end();
        if let Some(rest) = line.trim_start().strip_prefix("```") {
            flush(&mut blocks, &mut paragraph);
            ordered_index = 0;
            let lang = rest.trim().to_string();
            let mut code: Vec<&str> = Vec::new();
            i += 1;
            while i < lines.len() {
                if lines[i].trim_start().starts_with("```") {
                    break;
                }
                code.push(lines[i]);
                i += 1;
            }
            blocks.push(Block::Code {
                lang,
                text: code.join("\n"),
            });
            i += 1;
            continue;
        }
        if line.trim() == "---" || line.trim() == "***" || line.trim() == "___" {
            flush(&mut blocks, &mut paragraph);
            ordered_index = 0;
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }
        if let Some((headers, rows, consumed)) = parse_table(&lines[i..]) {
            flush(&mut blocks, &mut paragraph);
            ordered_index = 0;
            blocks.push(Block::Table { headers, rows });
            i += consumed;
            continue;
        }
        if let Some((alt, url)) = parse_image_line(line.trim()) {
            flush(&mut blocks, &mut paragraph);
            ordered_index = 0;
            blocks.push(Block::Image { alt, url });
            i += 1;
            continue;
        }
        if line.trim().is_empty() {
            flush(&mut blocks, &mut paragraph);
            ordered_index = 0;
            i += 1;
            continue;
        }
        let trimmed = line.trim_start();
        if let Some(quote) = trimmed.strip_prefix("> ") {
            flush(&mut blocks, &mut paragraph);
            ordered_index = 0;
            let mut body = vec![quote.to_string()];
            i += 1;
            while i < lines.len() {
                let next = lines[i].trim_start();
                if let Some(more) = next.strip_prefix("> ") {
                    body.push(more.to_string());
                    i += 1;
                } else if next == ">" {
                    body.push(String::new());
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(Block::Quote(body.join("\n")));
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush(&mut blocks, &mut paragraph);
            ordered_index = 0;
            blocks.push(Block::Heading(
                hashes as u8,
                trimmed[hashes + 1..].trim().to_string(),
            ));
            i += 1;
            continue;
        }
        if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("• "))
        {
            flush(&mut blocks, &mut paragraph);
            ordered_index = 0;
            blocks.push(Block::ListItem(0, item.trim().to_string()));
            i += 1;
            continue;
        }
        if let Some(item) = ordered_item(trimmed) {
            flush(&mut blocks, &mut paragraph);
            ordered_index += 1;
            blocks.push(Block::ListItem(ordered_index, item.to_string()));
            i += 1;
            continue;
        }
        paragraph.push(line.to_string());
        i += 1;
    }
    flush(&mut blocks, &mut paragraph);
    blocks
}

fn parse_image_line(trimmed: &str) -> Option<(String, String)> {
    let rest = trimmed.strip_prefix("![")?;
    let close = rest.find("](")?;
    let alt = rest[..close].to_string();
    let after = &rest[close + 2..];
    let end = after.find(')')?;
    if end + 1 != after.len() {
        return None;
    }
    Some((alt, after[..end].to_string()))
}

fn parse_table(lines: &[&str]) -> Option<(Vec<String>, Vec<Vec<String>>, usize)> {
    if lines.len() < 2 {
        return None;
    }
    let header = split_table_row(lines[0])?;
    if header.is_empty() {
        return None;
    }
    let sep = lines[1].trim();
    if !is_table_separator(sep, header.len()) {
        return None;
    }
    let mut rows = Vec::new();
    let mut consumed = 2usize;
    for line in &lines[2..] {
        if line.trim().is_empty() {
            break;
        }
        let Some(cells) = split_table_row(line) else {
            break;
        };
        if cells.len() != header.len() {
            break;
        }
        rows.push(cells);
        consumed += 1;
    }
    Some((header, rows, consumed))
}

fn split_table_row(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') {
        return None;
    }
    let body = trimmed.trim_matches('|');
    let cells: Vec<String> = body
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect();
    if cells.is_empty() { None } else { Some(cells) }
}

fn is_table_separator(line: &str, columns: usize) -> bool {
    let Some(cells) = split_table_row(line) else {
        return false;
    };
    cells.len() == columns
        && cells.iter().all(|cell| {
            let cell = cell.trim();
            let cell = cell.trim_matches(':');
            !cell.is_empty() && cell.chars().all(|c| c == '-')
        })
}

/// “1. ” / “1) ” 形式的有序列表项。
fn ordered_item(trimmed: &str) -> Option<&str> {
    let digits = trimmed
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit())
        .map(|(index, _)| index + 1)
        .last()
        .unwrap_or(0);
    if digits == 0 || digits > 3 {
        return None;
    }
    let rest = &trimmed[digits..];
    let separator = rest
        .strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))
        .or_else(|| rest.strip_prefix("． "))?;
    Some(separator.trim())
}

/// 兼容旧调用：仅返回粗体/代码区间（不含链接）。
pub(crate) struct InlineSpan {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) code: bool,
}

#[allow(dead_code)]
pub(crate) fn inline_spans(text: &str) -> Vec<InlineSpan> {
    let mut spans = Vec::new();
    let mut cursor = 0usize;
    while cursor < text.len() {
        if text[cursor..].starts_with('[')
            && let Some((_label_end, url_end)) = link_bounds(&text[cursor..])
        {
            cursor += url_end;
            continue;
        }
        if text[cursor..].starts_with("**")
            && let Some(offset) = text[cursor + 2..].find("**")
        {
            let start = cursor + 2;
            let end = start + offset;
            spans.push(InlineSpan {
                start,
                end,
                code: false,
            });
            cursor = end + 2;
            continue;
        }
        if text[cursor..].starts_with('`')
            && let Some(offset) = text[cursor + 1..].find('`')
        {
            let start = cursor + 1;
            let end = start + offset;
            spans.push(InlineSpan {
                start,
                end,
                code: true,
            });
            cursor = end + 1;
            continue;
        }
        cursor += text[cursor..].chars().next().map_or(1, char::len_utf8);
    }
    spans
}

/// 将行内 Markdown 拆成可渲染片段（链接独立成段）。
pub(crate) fn inline_parts(text: &str) -> Vec<InlinePart> {
    let mut parts = Vec::new();
    let mut cursor = 0usize;
    let mut plain = String::new();

    let flush_plain = |plain: &mut String, parts: &mut Vec<InlinePart>| {
        if !plain.is_empty() {
            parts.extend(split_emphasis(std::mem::take(plain)));
        }
    };

    while cursor < text.len() {
        if text[cursor..].starts_with('[')
            && let Some((label_end, url_end)) = link_bounds(&text[cursor..])
        {
            flush_plain(&mut plain, &mut parts);
            let label = text[cursor + 1..cursor + label_end].to_string();
            let url = text[cursor + label_end + 2..cursor + url_end - 1].to_string();
            parts.push(InlinePart::Link { label, url });
            cursor += url_end;
            continue;
        }
        let ch = text[cursor..].chars().next().unwrap();
        plain.push(ch);
        cursor += ch.len_utf8();
    }
    flush_plain(&mut plain, &mut parts);
    parts
}

fn link_bounds(slice: &str) -> Option<(usize, usize)> {
    // slice starts with '['
    let label_close = slice.find("](")?;
    if label_close == 1 {
        return None;
    }
    let url_start = label_close + 2;
    let url_close = slice[url_start..].find(')')? + url_start;
    Some((label_close, url_close + 1))
}

fn split_emphasis(text: String) -> Vec<InlinePart> {
    let mut parts = Vec::new();
    let mut cursor = 0usize;
    while cursor < text.len() {
        if text[cursor..].starts_with("**")
            && let Some(offset) = text[cursor + 2..].find("**")
        {
            let start = cursor + 2;
            let end = start + offset;
            parts.push(InlinePart::Text {
                text: text[start..end].to_string(),
                bold: true,
                code: false,
            });
            cursor = end + 2;
            continue;
        }
        if text[cursor..].starts_with('`')
            && let Some(offset) = text[cursor + 1..].find('`')
        {
            let start = cursor + 1;
            let end = start + offset;
            parts.push(InlinePart::Text {
                text: text[start..end].to_string(),
                bold: false,
                code: true,
            });
            cursor = end + 1;
            continue;
        }
        let next_bold = text[cursor..].find("**").map(|i| cursor + i);
        let next_code = text[cursor..].find('`').map(|i| cursor + i);
        let next = match (next_bold, next_code) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let end = next.unwrap_or(text.len());
        if end > cursor {
            parts.push(InlinePart::Text {
                text: text[cursor..end].to_string(),
                bold: false,
                code: false,
            });
        }
        cursor = end;
        if next.is_none() {
            break;
        }
    }
    parts
}

/// 代码高亮的 token 类别（近似 shiki 的关键词/字符串/数字/注释）。
pub(crate) mod diff_fence;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodeToken {
    Keyword,
    Str,
    Number,
    Comment,
}

pub(crate) struct CodeSpan {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) kind: CodeToken,
}

fn lang_keywords(lang: &str) -> &'static [&'static str] {
    match lang {
        "rust" | "rs" => &[
            "fn", "let", "mut", "pub", "use", "mod", "struct", "enum", "impl", "match", "if",
            "else", "for", "while", "loop", "return", "const", "static", "type", "trait", "where",
            "async", "await", "move", "ref", "dyn", "as", "in", "break", "continue", "unsafe",
            "crate", "self", "Self", "super", "Some", "None", "Ok", "Err", "true", "false",
        ],
        "js" | "javascript" | "ts" | "typescript" | "jsx" | "tsx" => &[
            "const",
            "let",
            "var",
            "function",
            "return",
            "if",
            "else",
            "for",
            "while",
            "do",
            "switch",
            "case",
            "break",
            "continue",
            "new",
            "class",
            "extends",
            "import",
            "from",
            "export",
            "default",
            "async",
            "await",
            "try",
            "catch",
            "finally",
            "throw",
            "typeof",
            "instanceof",
            "this",
            "null",
            "undefined",
            "true",
            "false",
            "interface",
            "type",
            "enum",
            "implements",
            "public",
            "private",
            "readonly",
        ],
        "python" | "py" => &[
            "def", "class", "return", "if", "elif", "else", "for", "while", "import", "from", "as",
            "with", "try", "except", "finally", "raise", "lambda", "yield", "pass", "break",
            "continue", "global", "nonlocal", "assert", "del", "in", "is", "not", "and", "or",
            "None", "True", "False", "self", "async", "await",
        ],
        "json" => &["true", "false", "null"],
        "sh" | "bash" | "shell" | "zsh" => &[
            "if", "then", "else", "fi", "for", "do", "done", "while", "case", "esac", "function",
            "return", "export", "local", "echo", "cd", "ls", "cat", "grep", "sed", "awk", "sudo",
            "rm", "mkdir", "cp", "mv", "source",
        ],
        _ => &[],
    }
}

fn line_comment_marker(lang: &str) -> Option<&'static str> {
    match lang {
        "python" | "py" | "sh" | "bash" | "shell" | "zsh" => Some("#"),
        "rust" | "rs" | "js" | "javascript" | "ts" | "typescript" | "jsx" | "tsx" => Some("//"),
        _ => None,
    }
}

/// 逐行扫描代码，产出关键词/字符串/数字/注释的字节区间（近似高亮）。
pub(crate) fn code_spans(code: &str, lang: &str) -> Vec<CodeSpan> {
    let keywords = lang_keywords(lang);
    let comment_marker = line_comment_marker(lang);
    let mut spans = Vec::new();
    let bytes = code.as_bytes();
    let mut line_start = 0usize;
    for line in code.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let line_trim = line.trim_start();
        let pad = line.len() - line_trim.len();
        let mut i = line_start + pad;
        let content_end = line_end - if line.ends_with('\n') { 1 } else { 0 };
        while i < content_end {
            if let Some(marker) = comment_marker
                && code[i..content_end].starts_with(marker)
            {
                let mut end = content_end;
                while end > i && matches!(bytes[end - 1], b' ' | b'\t' | b'\r') {
                    end -= 1;
                }
                spans.push(CodeSpan {
                    start: i,
                    end,
                    kind: CodeToken::Comment,
                });
                break;
            }
            let c = bytes[i];
            if c == b'"' || c == b'\'' || c == b'`' {
                let quote = c;
                let start = i;
                let mut j = i + 1;
                while j < content_end && bytes[j] != quote {
                    if bytes[j] == b'\\' && j + 1 < content_end {
                        j += 1;
                    }
                    j += code[j..].chars().next().map_or(1, char::len_utf8).max(1);
                }
                let end = (j + 1).min(content_end);
                spans.push(CodeSpan {
                    start,
                    end,
                    kind: CodeToken::Str,
                });
                i = end;
                continue;
            }
            if c.is_ascii_digit() {
                let start = i;
                while i < content_end
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_')
                {
                    i += 1;
                }
                spans.push(CodeSpan {
                    start,
                    end: i,
                    kind: CodeToken::Number,
                });
                continue;
            }
            if c.is_ascii_alphabetic() || c == b'_' {
                let start = i;
                while i < content_end && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                if keywords.contains(&&code[start..i]) {
                    spans.push(CodeSpan {
                        start,
                        end: i,
                        kind: CodeToken::Keyword,
                    });
                }
                continue;
            }
            i += code[i..].chars().next().map_or(1, char::len_utf8);
        }
        line_start = line_end;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::{
        Block, CodeToken, InlinePart, code_spans, frontmatter_bounds, image_destinations,
        inline_parts, parse_markdown,
    };

    #[test]
    fn frontmatter_delimiters_must_start_the_document_and_close_on_a_full_line() {
        for text in [
            "\n---\nkey: value\n---",
            " ---\n---",
            "---\nkey: value",
            "---\r\nkey: value\r\n---\r",
        ] {
            assert_eq!(frontmatter_bounds(text), None, "{text:?}");
        }
        let text = "\u{feff}--- \t\r\n中文: 值\r\n...\t\r\n# Body\r\n---";
        let (yaml, body) = frontmatter_bounds(text).unwrap();
        assert_eq!(&text[yaml], "中文: 值");
        assert_eq!(&text[body..], "# Body\r\n---");
    }

    #[test]
    fn frontmatter_accepts_empty_yaml_and_a_closer_at_a_page_boundary() {
        let page = "---\n--- \t";
        let (yaml, body) = frontmatter_bounds(page).unwrap();
        assert!(yaml.is_empty());
        assert_eq!(body, page.len());
        let complete = format!("{page}\n# Body");
        let (appended_yaml, appended_body) = frontmatter_bounds(&complete).unwrap();
        assert_eq!(appended_yaml, yaml);
        assert_eq!(&complete[appended_body..], "# Body");
    }

    #[test]
    fn blocks_split_headings_lists_and_code() {
        let blocks = parse_markdown(
            "## 标题\n\n第一段 **加粗** 与 `代码`。\n\n- 甲\n- 乙\n\n1. 步骤一\n2. 步骤二\n\n```rust\nfn x() {}\n```\n",
        );
        assert_eq!(
            blocks,
            vec![
                Block::Heading(2, "标题".into()),
                Block::Paragraph("第一段 **加粗** 与 `代码`。".into()),
                Block::ListItem(0, "甲".into()),
                Block::ListItem(0, "乙".into()),
                Block::ListItem(1, "步骤一".into()),
                Block::ListItem(2, "步骤二".into()),
                Block::Code {
                    lang: "rust".into(),
                    text: "fn x() {}".into(),
                },
            ]
        );
    }

    #[test]
    fn parses_quote_rule_table_link_and_image() {
        let blocks = parse_markdown(
            "> 引用一行\n\n---\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\n![图](https://example.com/a.png)\n\n见 [文档](https://example.com)。\n",
        );
        assert!(matches!(blocks[0], Block::Quote(_)));
        assert_eq!(blocks[1], Block::Rule);
        assert!(matches!(
            blocks[2],
            Block::Table {
                ref headers,
                ref rows
            } if headers == &["A".to_string(), "B".to_string()]
                && rows == &vec![vec!["1".to_string(), "2".to_string()]]
        ));
        assert_eq!(
            blocks[3],
            Block::Image {
                alt: "图".into(),
                url: "https://example.com/a.png".into()
            }
        );
        let InlinePart::Link { label, url } = &inline_parts("见 [文档](https://example.com)。")
            .into_iter()
            .find(|part| matches!(part, InlinePart::Link { .. }))
            .unwrap()
        else {
            panic!("expected link");
        };
        assert_eq!(label, "文档");
        assert_eq!(url, "https://example.com");
    }

    #[test]
    fn image_destinations_collect_block_images() {
        let blocks = image_destinations(
            "intro\n\n![a](/tmp/a.png)\n\n![b](<relative.png>)\n\n![c](/tmp/c.png?raw=1)\n",
        );
        assert_eq!(
            blocks,
            vec!["/tmp/a.png", "relative.png", "/tmp/c.png?raw=1"]
        );
        assert!(image_destinations("no images here").is_empty());
    }

    #[test]
    fn consecutive_lines_form_one_paragraph() {
        let blocks = parse_markdown("同一行的\n延续文本");
        assert_eq!(blocks, vec![Block::Paragraph("同一行的\n延续文本".into())]);
    }

    #[test]
    fn code_spans_flag_keywords_strings_numbers_comments() {
        let code = "let name = \"鲸鱼\"; // 备注\nfn main() { 42 }";
        let spans = code_spans(code, "rust");
        assert!(spans
            .iter()
            .any(|span| span.kind == CodeToken::Keyword && &code[span.start..span.end] == "let"));
        assert!(
            spans
                .iter()
                .any(|span| span.kind == CodeToken::Keyword && &code[span.start..span.end] == "fn")
        );
        assert!(
            spans.iter().any(
                |span| span.kind == CodeToken::Str && &code[span.start..span.end] == "\"鲸鱼\""
            )
        );
        assert!(
            spans.iter().any(|span| span.kind == CodeToken::Comment
                && code[span.start..span.end].starts_with("//"))
        );
        assert!(
            spans
                .iter()
                .any(|span| span.kind == CodeToken::Number && &code[span.start..span.end] == "42")
        );
        for span in &spans {
            assert!(span.start < span.end && span.end <= code.len());
        }
    }

    #[test]
    fn python_comments_use_hash_marker() {
        let code = "# 注释\nprint(1)";
        let spans = code_spans(code, "python");
        assert!(spans.iter().any(|span| span.kind == CodeToken::Comment));
    }
}
