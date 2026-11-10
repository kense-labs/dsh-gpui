//! Bounded, versioned text pages for the GPUI document viewer.

use super::{checked_relative, tree_directory};
use serde::Serialize;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};

const PAGE_LINES: usize = 5_000;
const PAGE_BYTES: usize = 2 * 1024 * 1024;
const HTML_BYTES: u64 = 32 * 1024 * 1024;
const HTML_ASSET_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(tag = "code", content = "detail", rename_all = "kebab-case")]
pub(crate) enum ReadFailure {
    NotFound,
    NotRegularFile,
    NotText,
    TooLarge,
    Changed,
    Unavailable(String),
}

impl From<io::Error> for ReadFailure {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::NotADirectory => Self::NotRegularFile,
            _ => Self::Unavailable(error.to_string()),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TextPage {
    absolute_path: String,
    version: String,
    offset: usize,
    text: String,
    lines: usize,
    eof: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HtmlSource {
    absolute_path: String,
    version: String,
    source: String,
}

#[tauri::command]
pub(crate) async fn read_workspace_text(
    workspace: String,
    path: String,
    offset: usize,
) -> Result<TextPage, ReadFailure> {
    tokio::task::spawn_blocking(move || read_text(&workspace, &path, offset))
        .await
        .map_err(|error| ReadFailure::Unavailable(error.to_string()))?
}

#[tauri::command]
pub(crate) async fn read_workspace_html(
    webview: tauri::Webview,
    workspace: String,
    path: String,
) -> Result<HtmlSource, ReadFailure> {
    if webview.label() != "main" {
        return Err(ReadFailure::Unavailable(
            "HTML 预览必须由主界面读取。".into(),
        ));
    }
    tokio::task::spawn_blocking(move || read_html(&workspace, &path))
        .await
        .map_err(|error| ReadFailure::Unavailable(error.to_string()))?
}

#[tauri::command]
pub(crate) async fn read_workspace_html_asset(
    webview: tauri::Webview,
    workspace: String,
    base_file: String,
    path: String,
) -> Result<HtmlSource, ReadFailure> {
    if webview.label() != "main" {
        return Err(ReadFailure::Unavailable(
            "HTML 预览必须由主界面读取。".into(),
        ));
    }
    tokio::task::spawn_blocking(move || {
        let asset = html_asset_path(&workspace, &base_file, &path)?;
        read_complete_text(&asset, HTML_ASSET_BYTES)
    })
    .await
    .map_err(|error| ReadFailure::Unavailable(error.to_string()))?
}

/// Resolve ancestors before checking the final component. The final component
/// must be checked without following symlinks when opening it for a read.
pub(super) fn document_path(workspace: &str, path: &str) -> Result<PathBuf, ReadFailure> {
    let root = tree_directory(Path::new(workspace)).map_err(ReadFailure::Unavailable)?;
    let relative = checked_relative(path).map_err(ReadFailure::Unavailable)?;
    let name = relative.file_name().ok_or(ReadFailure::NotRegularFile)?;
    let parent = root
        .join(relative.parent().unwrap_or(Path::new("")))
        .canonicalize()?;
    if !parent.starts_with(&root) {
        return Err(ReadFailure::Unavailable("文件在工作区之外。".into()));
    }
    Ok(parent.join(name))
}

#[derive(Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum DocumentSnapshot {
    Live {
        absolute_path: String,
        version: String,
    },
    Failed {
        failure: ReadFailure,
    },
}

impl DocumentSnapshot {
    pub(super) fn observe(path: &Path) -> Self {
        let version = fs::symlink_metadata(path)
            .map_err(ReadFailure::from)
            .and_then(|metadata| {
                if metadata.is_file() {
                    Ok(file_version(&metadata)?)
                } else {
                    Err(ReadFailure::NotRegularFile)
                }
            });
        match version {
            Ok(version) => Self::Live {
                absolute_path: path.to_string_lossy().into_owned(),
                version,
            },
            Err(failure) => Self::Failed { failure },
        }
    }
}

fn open_regular_file(path: &Path) -> Result<File, ReadFailure> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(ReadFailure::NotRegularFile);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(ReadFailure::NotRegularFile);
    }
    Ok(file)
}

fn read_text(workspace: &str, path: &str, offset: usize) -> Result<TextPage, ReadFailure> {
    if offset == 0 {
        return Err(ReadFailure::Unavailable("行号从 1 开始。".into()));
    }
    let absolute = document_path(workspace, path)?;
    let file = open_regular_file(&absolute)?;
    let version = file_version(&file.metadata()?)?;
    let mut reader = BufReader::new(file);
    let (text, lines, eof) = cut_page(&mut reader, offset, PAGE_LINES, PAGE_BYTES)?;
    verify_version(reader.get_ref(), &absolute, &version)?;
    Ok(TextPage {
        absolute_path: absolute.to_string_lossy().into_owned(),
        version,
        offset,
        text,
        lines,
        eof,
    })
}

fn has_extension(path: &str, extensions: &[&str]) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, suffix)| {
            extensions
                .iter()
                .any(|ext| suffix.eq_ignore_ascii_case(ext))
        })
}

fn read_html(workspace: &str, path: &str) -> Result<HtmlSource, ReadFailure> {
    if !has_extension(path, &["html", "htm"]) {
        return Err(ReadFailure::NotText);
    }
    read_complete_text(&document_path(workspace, path)?, HTML_BYTES)
}

pub(super) fn html_asset_path(
    workspace: &str,
    base_file: &str,
    path: &str,
) -> Result<PathBuf, ReadFailure> {
    if !has_extension(base_file, &["html", "htm"])
        || !has_extension(path, &["js", "css"])
        || path.contains(['\0', '\\'])
    {
        return Err(ReadFailure::NotText);
    }
    let base = document_path(workspace, base_file)?;
    if !fs::symlink_metadata(&base)?.is_file() {
        return Err(ReadFailure::NotRegularFile);
    }
    let mut target = base.parent().ok_or(ReadFailure::NotRegularFile)?.to_owned();
    // The official workspace byte reader resolves relative to the HTML file and
    // supports sibling directories outside the workspace via an explicit `..`.
    for part in Path::new(path).components() {
        match part {
            Component::Normal(name) => target.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                target.pop();
            }
            Component::RootDir | Component::Prefix(_) => return Err(ReadFailure::NotRegularFile),
        }
    }
    if let (Some(parent), Some(name)) = (target.parent(), target.file_name())
        && let Ok(canonical) = parent.canonicalize()
    {
        return Ok(canonical.join(name));
    }
    Ok(target)
}

fn read_complete_text(absolute: &Path, limit: u64) -> Result<HtmlSource, ReadFailure> {
    let mut file = open_regular_file(absolute)?;
    let metadata = file.metadata()?;
    if metadata.len() > limit {
        return Err(ReadFailure::TooLarge);
    }
    let version = file_version(&metadata)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file).take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(ReadFailure::TooLarge);
    }
    verify_version(&file, absolute, &version)?;
    let source = String::from_utf8(bytes).map_err(|_| ReadFailure::NotText)?;
    Ok(HtmlSource {
        absolute_path: absolute.to_string_lossy().into_owned(),
        version,
        source,
    })
}

fn verify_version(file: &File, path: &Path, version: &str) -> Result<(), ReadFailure> {
    if version != file_version(&file.metadata()?)?
        || version != file_version(&fs::symlink_metadata(path)?)?
    {
        return Err(ReadFailure::Changed);
    }
    Ok(())
}

fn cut_page(
    reader: &mut impl BufRead,
    offset: usize,
    limit: usize,
    bytes: usize,
) -> Result<(String, usize, bool), ReadFailure> {
    if reader.fill_buf()?.starts_with(b"\xef\xbb\xbf") {
        reader.consume(3);
    }
    for _ in 1..offset {
        if reader.skip_until(b'\n')? == 0 {
            return Ok((String::new(), 0, true));
        }
    }
    let mut page = Vec::new();
    let mut lines = 0;
    while lines < limit && !reader.fill_buf()?.is_empty() {
        let start = page.len();
        // One byte for the final delimiter and one to detect an overlong page.
        reader
            .take((bytes + 2 - start) as u64)
            .read_until(b'\n', &mut page)?;
        let terminated = page.last() == Some(&b'\n');
        if page.len() - usize::from(terminated) > bytes {
            return Err(ReadFailure::TooLarge);
        }
        lines += 1;
    }
    if page.last() == Some(&b'\n') {
        page.pop();
    }
    if page.contains(&0) {
        return Err(ReadFailure::NotText);
    }
    let text = String::from_utf8(page).map_err(|_| ReadFailure::NotText)?;
    Ok((text, lines, reader.fill_buf()?.is_empty()))
}

fn file_version(metadata: &Metadata) -> io::Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!(
            "{}:{}:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec()
        ))
    }
    #[cfg(not(unix))]
    {
        Ok(format!(
            "{}:{:?}:{:?}",
            metadata.len(),
            metadata.modified()?,
            metadata.created().ok()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_preserve_blank_lines_and_do_not_add_a_terminal_line() {
        let read = |offset, lines, bytes| {
            cut_page(
                &mut io::Cursor::new(b"\xef\xbb\xbfalpha\n\nomega\n"),
                offset,
                lines,
                bytes,
            )
            .unwrap()
        };
        assert_eq!(read(1, 2, 6), ("alpha\n".into(), 2, false));
        assert_eq!(read(2, 1, 0), (String::new(), 1, false));
        assert_eq!(read(3, 2, 5), ("omega".into(), 1, true));
        assert_eq!(read(4, 1, 0), (String::new(), 0, true));
    }

    #[cfg(unix)]
    #[test]
    fn real_files_provide_one_version_across_pages_and_reject_symlinks() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let base =
            Fixture(std::env::temp_dir().join(format!("dsh-preview-{}", uuid::Uuid::new_v4())));
        let root = base.0.join("root");
        fs::create_dir_all(&root).unwrap();
        let text = root.join("notes.txt");
        fs::write(&text, format!("{}last", "x\n".repeat(PAGE_LINES))).unwrap();
        let workspace = root.to_str().unwrap();
        let first = read_text(workspace, "notes.txt", 1).unwrap();
        let second = read_text(workspace, "notes.txt", PAGE_LINES + 1).unwrap();
        assert_eq!(first.lines, PAGE_LINES);
        assert!(!first.eof);
        assert_eq!(second.text, "last");
        assert!(second.eof);
        assert_eq!(first.version, second.version);
        fs::write(text, "replacement").unwrap();
        let replacement = read_text(workspace, "notes.txt", 1).unwrap();
        assert_ne!(replacement.version, first.version);

        std::os::unix::fs::symlink("notes.txt", root.join("alias.txt")).unwrap();
        assert!(matches!(
            read_text(workspace, "alias.txt", 1),
            Err(ReadFailure::NotRegularFile)
        ));
        assert!(read_text(workspace, "../outside.txt", 1).is_err());

        let html = root.join("preview.HTM");
        let source = format!(
            "\u{feff}<!doctype html>\r\n{}<p>结尾</p>\r\n",
            "\r\n".repeat(PAGE_LINES)
        );
        fs::write(&html, source.as_bytes()).unwrap();
        let page = read_html(workspace, "preview.HTM").unwrap();
        assert_eq!(page.source, source);
        assert_eq!(
            page.version,
            read_text(workspace, "preview.HTM", 1).unwrap().version
        );
        std::os::unix::fs::symlink("preview.HTM", root.join("alias.html")).unwrap();
        assert!(matches!(
            read_html(workspace, "alias.html"),
            Err(ReadFailure::NotRegularFile)
        ));
        assert!(read_html(workspace, "../preview.HTM").is_err());
        let outside = base.0.join("outside.JS");
        fs::write(&outside, "document.title = '邻接资源';").unwrap();
        let resolved = html_asset_path(workspace, "preview.HTM", "../outside.JS").unwrap();
        let asset = read_complete_text(&resolved, HTML_ASSET_BYTES).unwrap();
        assert_eq!(asset.source, "document.title = '邻接资源';");
        assert!(html_asset_path(workspace, "preview.HTM", outside.to_str().unwrap()).is_err());
        assert!(html_asset_path(workspace, "alias.html", "../outside.JS").is_err());
        assert!(html_asset_path(workspace, "preview.HTM", "../outside.txt").is_err());
        std::os::unix::fs::symlink(&outside, root.join("alias.js")).unwrap();
        let alias = html_asset_path(workspace, "preview.HTM", "./alias.js").unwrap();
        assert!(matches!(
            read_complete_text(&alias, HTML_ASSET_BYTES),
            Err(ReadFailure::NotRegularFile)
        ));
        File::create(&outside)
            .unwrap()
            .set_len(HTML_ASSET_BYTES + 1)
            .unwrap();
        assert!(matches!(
            read_complete_text(&resolved, HTML_ASSET_BYTES),
            Err(ReadFailure::TooLarge)
        ));
        fs::write(&html, [0xff]).unwrap();
        assert!(matches!(
            read_html(workspace, "preview.HTM"),
            Err(ReadFailure::NotText)
        ));
        File::create(&html)
            .unwrap()
            .set_len(HTML_BYTES + 1)
            .unwrap();
        assert!(matches!(
            read_html(workspace, "preview.HTM"),
            Err(ReadFailure::TooLarge)
        ));
    }

    #[test]
    fn oversized_or_binary_pages_are_refused() {
        assert!(matches!(
            cut_page(&mut io::Cursor::new(b"abcd\n"), 1, 1, 3),
            Err(ReadFailure::TooLarge)
        ));
        assert!(matches!(
            cut_page(&mut io::Cursor::new(b"\0"), 1, 1, 3),
            Err(ReadFailure::NotText)
        ));
        assert!(matches!(
            cut_page(&mut io::Cursor::new(b"\xff"), 1, 1, 3),
            Err(ReadFailure::NotText)
        ));
    }
}
