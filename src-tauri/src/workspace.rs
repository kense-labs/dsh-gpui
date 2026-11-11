//! Workspace directory listings and native open actions.

#[cfg(target_os = "macos")]
pub(crate) mod applications;
pub(crate) mod document;
pub(crate) mod watch;

use serde::Serialize;
use std::fs;
use std::path::{Component, Path, PathBuf};

const ENTRY_LIMIT: usize = 200;
const TREE_ENTRY_LIMIT: usize = 2_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_other: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileList {
    pub request_id: u64,
    pub directory: String,
    pub entries: Vec<WorkspaceFileEntry>,
    pub truncated: bool,
}

/// "添加工作区" follows DeepSeek Harness: open the platform directory picker,
/// then register the chosen directory as the active workspace. Cancelling
/// yields `null`; a platform without a native picker reports an error instead
/// of silently doing nothing.
#[tauri::command]
pub async fn pick_workspace_directory(webview: tauri::Webview) -> Result<Option<String>, String> {
    #[cfg(target_os = "macos")]
    {
        directory_picker::choose_directory(webview).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = webview;
        Err("当前平台暂不支持原生目录选择器，请在「通用设置」中填写工作区绝对路径。".to_owned())
    }
}

#[cfg(target_os = "macos")]
mod directory_picker {
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSModalResponseCancel, NSModalResponseOK, NSOpenPanel, NSWindow};
    use std::cell::RefCell;

    const PICKER_ERROR: &str = "无法打开系统目录选择器。";

    /// NSOpenPanel as a window sheet, matching the AppKit save sheet helper in
    /// `history::export::archive::macos`. The host never blocks on the panel.
    pub(super) async fn choose_directory(
        webview: tauri::Webview,
    ) -> Result<Option<String>, String> {
        let window = webview.window().clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        window
            .clone()
            .run_on_main_thread(move || {
                let Some(mtm) = MainThreadMarker::new() else {
                    let _ = send.send(Err(PICKER_ERROR.to_owned()));
                    return;
                };
                let native = window
                    .ns_window()
                    .ok()
                    .and_then(|window| unsafe { window.cast::<NSWindow>().as_ref() });
                let Some(native) = native else {
                    let _ = send.send(Err(PICKER_ERROR.to_owned()));
                    return;
                };
                let panel = NSOpenPanel::openPanel(mtm);
                panel.setCanChooseDirectories(true);
                panel.setCanChooseFiles(false);
                panel.setAllowsMultipleSelection(false);
                panel.setCanCreateDirectories(true);
                let send = RefCell::new(Some(send));
                let retained = panel.clone();
                let complete = RcBlock::new(move |response| {
                    let result = match response {
                        value if value == NSModalResponseCancel => Ok(None),
                        value if value == NSModalResponseOK => retained
                            .URL()
                            .and_then(|url| url.path())
                            .map(|path| Some(path.to_string()))
                            .ok_or_else(|| PICKER_ERROR.to_owned()),
                        _ => Err(PICKER_ERROR.to_owned()),
                    };
                    if let Some(send) = send.borrow_mut().take() {
                        let _ = send.send(result);
                    }
                });
                panel.beginSheetModalForWindow_completionHandler(native, &complete);
            })
            .map_err(|_| PICKER_ERROR.to_owned())?;
        receive.await.map_err(|_| PICKER_ERROR.to_owned())?
    }
}

#[tauri::command]
pub async fn list_workspace_files(
    workspace: String,
    directory: String,
    request_id: u64,
    tree: Option<bool>,
) -> Result<WorkspaceFileList, String> {
    tokio::task::spawn_blocking(move || {
        if tree.unwrap_or(false) {
            list_tree_level(&workspace, &directory, request_id)
        } else {
            list_files(&workspace, &directory, request_id)
        }
    })
    .await
    .map_err(|error| format!("读取工作区任务失败：{error}"))?
}

#[tauri::command]
pub async fn open_start_tool(workspace: String, tool: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || open_tool(&workspace, &tool))
        .await
        .map_err(|error| format!("打开工作区工具失败：{error}"))?
}

#[tauri::command]
pub async fn open_browser_url(url: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || open_url(&url))
        .await
        .map_err(|error| format!("打开链接失败：{error}"))?
}

fn normalize_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("请输入网址".to_string());
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Ok(trimmed.to_string());
    }
    if trimmed.contains(' ') {
        return Err("网址无效".to_string());
    }
    Ok(format!("https://{trimmed}"))
}

#[cfg(target_os = "macos")]
fn open_url(raw: &str) -> Result<String, String> {
    let url = normalize_url(raw)?;
    let status = std::process::Command::new("open")
        .arg(&url)
        .status()
        .map_err(|error| format!("无法打开链接：{error}"))?;
    if status.success() {
        Ok(url)
    } else {
        Err(format!("打开链接失败：{status}"))
    }
}

#[cfg(not(target_os = "macos"))]
fn open_url(raw: &str) -> Result<String, String> {
    let url = normalize_url(raw)?;
    let status = std::process::Command::new("xdg-open")
        .arg(&url)
        .status()
        .map_err(|error| format!("无法打开链接：{error}"))?;
    if status.success() {
        Ok(url)
    } else {
        Err(format!("打开链接失败：{status}"))
    }
}

#[cfg(target_os = "macos")]
fn open_tool(workspace: &str, tool: &str) -> Result<(), String> {
    let root = canonical_root(Path::new(workspace))?;
    let mut command = std::process::Command::new("open");
    match tool {
        "files" => {
            command.arg(&root);
        }
        "terminal" => {
            command.arg("-a").arg("Terminal").arg(&root);
        }
        "browser" => {
            command.arg("https://www.deepseek.com/");
        }
        _ => return Err("未知的工作区工具".to_string()),
    }
    let status = command
        .status()
        .map_err(|error| format!("无法打开工作区工具：{error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("工作区工具打开失败：{status}"))
    }
}

#[cfg(not(target_os = "macos"))]
fn open_tool(_workspace: &str, _tool: &str) -> Result<(), String> {
    Err("当前桌面平台暂不支持此工作区工具".to_string())
}

fn list_files(
    workspace: &str,
    directory: &str,
    request_id: u64,
) -> Result<WorkspaceFileList, String> {
    let root = canonical_root(Path::new(workspace))?;
    let relative = checked_relative(directory)?;
    let target = canonical_target(&root, &relative)?;
    let mut entries = Vec::new();
    let mut truncated = false;
    for item in fs::read_dir(&target).map_err(|error| format!("无法读取工作区目录：{error}"))?
    {
        let item = item.map_err(|error| format!("无法读取工作区条目：{error}"))?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = item.path();
        let Ok(canonical) = fs::canonicalize(&path) else {
            continue;
        };
        if !canonical.starts_with(&root) {
            continue;
        }
        let Ok(metadata) = fs::metadata(&canonical) else {
            continue;
        };
        if !metadata.is_dir() && !metadata.is_file() {
            continue;
        }
        let child_relative = relative.join(&name);
        entries.push(WorkspaceFileEntry {
            name,
            path: slash_path(&child_relative),
            is_directory: metadata.is_dir(),
            is_other: false,
        });
    }
    entries.sort_by(|left, right| {
        right
            .is_directory
            .cmp(&left.is_directory)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
    if entries.len() > ENTRY_LIMIT {
        entries.truncate(ENTRY_LIMIT);
        truncated = true;
    }
    Ok(WorkspaceFileList {
        request_id,
        directory: slash_path(&relative),
        entries,
        truncated,
    })
}

fn list_tree_level(
    workspace: &str,
    directory: &str,
    request_id: u64,
) -> Result<WorkspaceFileList, String> {
    let (relative, target) = tree_target(workspace, directory)?;
    let mut entries = Vec::new();
    for item in fs::read_dir(&target).map_err(tree_read_error)? {
        let item = item.map_err(tree_read_error)?;
        let kind = item.file_type().map_err(tree_read_error)?;
        let kind = if kind.is_symlink() {
            match fs::metadata(item.path()) {
                Ok(info) => Some(info.file_type()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(tree_read_error(error)),
            }
        } else {
            Some(kind)
        };
        let name = item.file_name().to_string_lossy().into_owned();
        entries.push(WorkspaceFileEntry {
            path: slash_path(&relative.join(&name)),
            name,
            is_directory: kind.is_some_and(|kind| kind.is_dir()),
            is_other: kind.is_none_or(|kind| !kind.is_dir() && !kind.is_file()),
        });
    }
    entries.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    let truncated = entries.len() > TREE_ENTRY_LIMIT;
    entries.truncate(TREE_ENTRY_LIMIT);
    Ok(WorkspaceFileList {
        request_id,
        directory: slash_path(&relative),
        entries,
        truncated,
    })
}

fn tree_target(workspace: &str, directory: &str) -> Result<(PathBuf, PathBuf), String> {
    let root = tree_directory(Path::new(workspace))?;
    let relative = checked_relative(directory)
        .map_err(|_| "这个目录在工作区之外，侧栏不会读取它。".to_owned())?;
    let target = tree_directory(&root.join(&relative))?;
    if !target.starts_with(&root) {
        return Err("这个目录在工作区之外，侧栏不会读取它。".to_owned());
    }
    Ok((relative, target))
}

fn tree_directory(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("工作区必须是绝对路径".to_owned());
    }
    let canonical = fs::canonicalize(path).map_err(tree_read_error)?;
    if !canonical.is_dir() {
        return Err("这不是一个目录。".to_owned());
    }
    Ok(canonical)
}

fn tree_read_error(error: std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => "这个目录不在了。可能已被移动或删除。".to_owned(),
        std::io::ErrorKind::NotADirectory => "这不是一个目录。".to_owned(),
        _ => format!("读取失败：{error}"),
    }
}

fn canonical_root(workspace: &Path) -> Result<PathBuf, String> {
    if !workspace.is_absolute() {
        return Err("工作区必须是绝对路径".to_string());
    }
    let root = fs::canonicalize(workspace).map_err(|error| format!("工作区不可用：{error}"))?;
    if !root.is_dir() {
        return Err("工作区不是目录".to_string());
    }
    Ok(root)
}

fn checked_relative(directory: &str) -> Result<PathBuf, String> {
    let path = Path::new(directory);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("文件选择器不能访问工作区外的路径".to_string());
    }
    Ok(path.to_path_buf())
}

fn canonical_target(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let target = fs::canonicalize(root.join(relative))
        .map_err(|error| format!("工作区目录不可用：{error}"))?;
    if !target.starts_with(root) || !target.is_dir() {
        return Err("文件选择器不能访问工作区外的路径".to_string());
    }
    Ok(target)
}

fn slash_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_cannot_escape_the_workspace() {
        assert!(checked_relative("src/main.rs").is_ok());
        assert!(checked_relative("").is_ok());
        assert!(checked_relative("../secrets").is_err());
        assert!(checked_relative("/etc").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn tree_lists_dotfiles_and_directory_links_but_only_reads_inside_the_root() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("dsh-tree-{}", std::process::id()));
        fs::create_dir(&base).unwrap();
        struct RemoveTree(PathBuf);
        impl Drop for RemoveTree {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = RemoveTree(base.clone());
        let root = base.join("root");
        fs::create_dir_all(root.join("dir")).unwrap();
        fs::write(root.join(".env"), "").unwrap();
        fs::write(root.join("dir/child.txt"), "").unwrap();
        symlink("dir", root.join("inside")).unwrap();
        symlink(&base, root.join("outside")).unwrap();
        symlink("missing", root.join("broken")).unwrap();
        let root = root.to_str().unwrap();

        let tree = list_tree_level(root, "", 1).unwrap();
        let entries: Vec<_> = tree
            .entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.is_directory, entry.is_other))
            .collect();
        assert_eq!(
            entries,
            [
                (".env", false, false),
                ("broken", false, true),
                ("dir", true, false),
                ("inside", true, false),
                ("outside", true, false),
            ]
        );
        let inside = list_tree_level(root, "inside", 2).unwrap();
        assert_eq!(inside.entries[0].path, "inside/child.txt");
        assert!(list_tree_level(root, "outside", 3).is_err());

        let mention = list_files(root, "", 4).unwrap();
        assert!(mention.entries.iter().all(|entry| entry.name != ".env"));
    }

    #[test]
    fn paths_use_portable_slashes() {
        assert_eq!(slash_path(Path::new("apps/aster/src")), "apps/aster/src");
        assert_eq!(slash_path(Path::new("")), "");
    }
}
