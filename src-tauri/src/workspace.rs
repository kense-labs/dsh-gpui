//! Bounded, workspace-local directory listings for composer file references.

use serde::Serialize;
use std::fs;
use std::path::{Component, Path, PathBuf};

const ENTRY_LIMIT: usize = 200;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileList {
    pub request_id: u64,
    pub directory: String,
    pub entries: Vec<WorkspaceFileEntry>,
    pub truncated: bool,
}

#[tauri::command]
pub async fn list_workspace_files(
    workspace: String,
    directory: String,
    request_id: u64,
) -> Result<WorkspaceFileList, String> {
    tokio::task::spawn_blocking(move || list_files(&workspace, &directory, request_id))
        .await
        .map_err(|error| format!("读取工作区任务失败：{error}"))?
}

#[tauri::command]
pub async fn open_start_tool(workspace: String, tool: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || open_tool(&workspace, &tool))
        .await
        .map_err(|error| format!("打开工作区工具失败：{error}"))?
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

    #[test]
    fn paths_use_portable_slashes() {
        assert_eq!(slash_path(Path::new("apps/aster/src")), "apps/aster/src");
        assert_eq!(slash_path(Path::new("")), "");
    }
}
