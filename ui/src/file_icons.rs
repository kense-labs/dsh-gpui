//! File-kind classification behind the Files tree and document-tab icons.
//!
//! The official client classifies by extension (`classifyFileType`) and draws a
//! colour glyph per category. The GPUI client has a smaller artwork set, so this
//! module only resolves the category; the renderer picks the matching glyph.

/// Render categories the GPUI client can actually tell apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Markdown,
    Image,
    Code,
    Office,
    Other,
}

impl FileKind {
    /// Extensions that the renderer draws with the markdown mark.
    const MARKDOWN: &'static [&'static str] = &["md", "mdx", "markdown"];

    /// Picture formats opened by the image viewer.
    const IMAGE: &'static [&'static str] = &[
        "png", "jpg", "jpeg", "gif", "svg", "webp", "avif", "bmp", "ico", "tif", "tiff", "heic",
        "heif",
    ];

    /// Source and configuration files.
    const CODE: &'static [&'static str] = &[
        "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "rb", "php", "lua", "sql",
        "proto", "dart", "zig", "swift", "kt", "kts", "java", "cs", "c", "h", "cc", "cpp", "hpp",
        "m", "mm", "r", "pl", "ps1", "sh", "bash", "zsh", "fish", "css", "scss", "sass", "less",
        "html", "htm", "xml", "json", "jsonc", "toml", "yaml", "yml", "ini", "env", "lock", "vue",
        "svelte", "astro", "ex", "exs", "clj", "scala", "cmake", "mk", "wasm",
    ];

    /// Documents and spreadsheets.
    const OFFICE: &'static [&'static str] = &[
        "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "csv", "tsv", "rtf", "pages",
        "numbers", "key",
    ];
}

/// Resolve one file name or path to the category its icon should show.
///
/// Matching is case-insensitive, works on the basename and never panics for
/// names without an extension, dotfiles or paths with directories.
pub fn file_kind(name: &str) -> FileKind {
    let basename = name
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(name);
    let Some((_, extension)) = basename.rsplit_once('.') else {
        return FileKind::Other;
    };
    let extension = extension.to_ascii_lowercase();
    if extension.is_empty() {
        return FileKind::Other;
    }
    let extension = extension.as_str();
    if FileKind::MARKDOWN.contains(&extension) {
        FileKind::Markdown
    } else if FileKind::IMAGE.contains(&extension) {
        FileKind::Image
    } else if FileKind::OFFICE.contains(&extension) {
        FileKind::Office
    } else if FileKind::CODE.contains(&extension) {
        FileKind::Code
    } else {
        FileKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::{FileKind, file_kind};

    #[test]
    fn classifies_by_extension_case_insensitively() {
        assert_eq!(file_kind("README.md"), FileKind::Markdown);
        assert_eq!(file_kind("notes.MDX"), FileKind::Markdown);
        assert_eq!(file_kind("diagram.PNG"), FileKind::Image);
        assert_eq!(file_kind("main.rs"), FileKind::Code);
        assert_eq!(file_kind("Cargo.lock"), FileKind::Code);
        assert_eq!(file_kind("预算.xlsx"), FileKind::Office);
        assert_eq!(file_kind("report.pdf"), FileKind::Office);
        assert_eq!(file_kind("archive.zip"), FileKind::Other);
        assert_eq!(file_kind("Makefile"), FileKind::Other);
        assert_eq!(file_kind(".gitignore"), FileKind::Other);
        assert_eq!(file_kind("trailing."), FileKind::Other);
        assert_eq!(file_kind(""), FileKind::Other);
    }

    #[test]
    fn classifies_on_the_basename_not_the_directory() {
        assert_eq!(file_kind("src/image.png/main.ts"), FileKind::Code);
        assert_eq!(file_kind("docs/image.png"), FileKind::Image);
        assert_eq!(file_kind("C:\\work\\README.md"), FileKind::Markdown);
    }
}
