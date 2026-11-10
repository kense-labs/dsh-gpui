//! Source preview languages and Shiki styles for the GPUI document body.

use super::{document_selection::SelectionLayout, *};
use gpui::{FontStyle, HighlightStyle, Hsla, StrikethroughStyle, StyledText, UnderlineStyle, rgb};
use serde::Deserialize;

#[wasm_bindgen(raw_module = "../document-highlight.js")]
extern "C" {
    pub(super) type HighlightSession;

    #[wasm_bindgen(constructor)]
    pub(super) fn new() -> HighlightSession;

    #[wasm_bindgen(method)]
    pub(super) fn highlight(
        this: &HighlightSession,
        source: &str,
        language: &str,
    ) -> js_sys::Promise;
}

// Matches the official document preview registry's suffix table.
pub(super) fn language_for_path(path: &str) -> Option<&'static str> {
    let basename = path.rsplit(['/', '\\']).next()?;
    let (_, suffix) = basename.rsplit_once('.')?;
    Some(match suffix.to_ascii_lowercase().as_str() {
        "ts" | "tsx" | "mts" | "cts" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "sh" | "bash" | "zsh" => "shellscript",
        "fish" => "fish",
        "json" | "jsonc" | "jsonl" | "ndjson" | "ipynb" => "json",
        "csv" => "csv",
        "py" | "pyw" | "pyi" => "python",
        "rb" | "rake" | "gemspec" => "ruby",
        "go" => "go",
        "rs" => "rust",
        "java" => "java",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => "cpp",
        "cs" => "csharp",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "php" => "php",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "ini" | "conf" | "cfg" | "properties" => "ini",
        "env" => "dotenv",
        "log" => "log",
        "diff" | "patch" => "diff",
        "http" => "http",
        "md" | "markdown" => "markdown",
        "mdx" => "mdx",
        "rst" => "rst",
        "tex" | "sty" | "cls" => "latex",
        "bib" => "bibtex",
        "adoc" => "asciidoc",
        "html" | "htm" | "xhtml" => "html",
        "css" => "css",
        "scss" => "scss",
        "less" => "less",
        "sql" => "sql",
        "xml" | "xsd" | "xsl" | "xslt" | "plist" | "svg" => "xml",
        "lua" => "lua",
        "bat" | "cmd" => "bat",
        "ps1" | "psm1" | "psd1" => "powershell",
        "r" => "r",
        "jl" => "julia",
        "dart" => "dart",
        "scala" => "scala",
        "clj" | "cljs" | "edn" => "clojure",
        "erl" | "hrl" => "erlang",
        "ex" | "exs" => "elixir",
        "hs" => "haskell",
        "fs" | "fsi" | "fsx" => "fsharp",
        "vb" => "vb",
        "pl" | "pm" => "perl",
        "v" => "verilog",
        "sv" | "svh" => "system-verilog",
        "graphql" | "gql" => "graphql",
        "proto" => "proto",
        "tf" | "tfvars" | "hcl" => "hcl",
        "nix" => "nix",
        "vue" => "vue",
        "svelte" => "svelte",
        "makefile" | "mk" => "make",
        "cmake" => "cmake",
        "gradle" | "groovy" => "groovy",

        _ => return None,
    })
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CodeColor {
    Constant,
    String,
    Comment,
    Keyword,
    Parameter,
    Function,
    StringExpression,
    Punctuation,
    Link,
}

impl CodeColor {
    // The corresponding --shiki-token-* values from the official light/dark theme.
    fn ink(self, dark: bool) -> Hsla {
        let (light, dark_color) = match self {
            Self::Constant => (0x1c7ed6, 0x4dabf7),
            Self::String => (0x2f9e44, 0x69db7c),
            Self::Comment => (0x868e96, 0xadb5bd),
            Self::Keyword => (0xd6336c, 0xfaa2c1),
            Self::Parameter => (0xe8590c, 0xffa94d),
            Self::Function => (0x6741d9, 0xb197fc),
            Self::StringExpression => (0x2b8a3e, 0x8ce99a),
            Self::Punctuation => (0x495057, 0xced4da),
            Self::Link => (0x1971c2, 0x74c0fc),
        };
        rgb(if dark { dark_color } else { light }).into()
    }
}

#[derive(Deserialize)]
pub(super) struct CodeSpan {
    start: usize,
    end: usize,
    color: Option<CodeColor>,
    style: u8,
}

impl CodeSpan {
    fn style(&self, dark: bool) -> HighlightStyle {
        let color = self.color.map(|color| color.ink(dark));
        HighlightStyle {
            color,
            font_style: (self.style & 1 != 0).then_some(FontStyle::Italic),
            font_weight: (self.style & 2 != 0).then_some(FontWeight::BOLD),
            underline: (self.style & 4 != 0).then_some(UnderlineStyle {
                thickness: px(1.),
                color,
                wavy: false,
            }),
            strikethrough: (self.style & 8 != 0).then_some(StrikethroughStyle {
                thickness: px(1.),
                color,
            }),
            ..Default::default()
        }
    }
}

pub(super) fn display_source(source: &str) -> &str {
    source.strip_suffix('\n').unwrap_or(source)
}

pub(super) fn render_code(
    source: &str,
    tokens: &[Vec<CodeSpan>],
    selection: &SelectionLayout,
    wrap: bool,
    font_delta: i32,
    cx: &App,
) -> gpui::Div {
    let p = dsh(cx);
    let font = if bridge::is_mac() {
        "Menlo"
    } else {
        "monospace"
    };
    let font_size = px(11. + font_delta as f32);
    let text = cx.text_system();
    let font_id = text.resolve_font(&gpui::font(font));
    let number_width = source.split('\n').count().to_string().len().max(2);
    let digit = text
        .ch_advance(font_id, font_size)
        .unwrap_or(font_size * 0.6);
    let gutter = digit * number_width as f32 + px(12.);
    let dark = super::workspace::dark(cx);
    let mut base = 0;
    let mut highlights = Vec::new();
    for (line, spans) in source.split('\n').zip(tokens) {
        for span in spans {
            if span.start < span.end && line.get(span.start..span.end).is_some() {
                highlights.push((base + span.start..base + span.end, span.style(dark)));
            }
        }
        base += line.len() + 1;
    }
    div()
        .pl(px(16.) + gutter)
        .pr(px(16.))
        .py(px(16.))
        .font_family(font)
        .text_size(font_size)
        .line_height(px(19. + font_delta as f32))
        .text_color(p.label_primary)
        .when(!wrap, |text| text.whitespace_nowrap())
        .child(
            selection.numbered_text(source.to_owned(), p.label_tertiary, |source| {
                StyledText::new(source).with_highlights(highlights)
            }),
        )
}
