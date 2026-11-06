//! Persistent sidebar-only workspace titles, expansion and visibility.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspacePreferences {
    pub aliases: HashMap<String, String>,
    pub hidden: Vec<String>,
    pub collapsed: Vec<String>,
}

#[tauri::command]
pub async fn load_workspace_preferences(app: AppHandle) -> Result<WorkspacePreferences, String> {
    let path = preferences_path(&app)?;
    tokio::task::spawn_blocking(move || read_preferences(&path))
        .await
        .map_err(|error| format!("读取工作区显示设置失败：{error}"))?
}

#[tauri::command]
pub async fn save_workspace_preferences(
    app: AppHandle,
    preferences: WorkspacePreferences,
) -> Result<(), String> {
    let path = preferences_path(&app)?;
    tokio::task::spawn_blocking(move || write_preferences(&path, &preferences))
        .await
        .map_err(|error| format!("保存工作区显示设置失败：{error}"))?
}

fn preferences_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("workspaces.json"))
        .map_err(|error| format!("无法确定工作区显示设置目录：{error}"))
}

fn read_preferences(path: &PathBuf) -> Result<WorkspacePreferences, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("工作区显示设置格式无效：{error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(WorkspacePreferences::default())
        }
        Err(error) => Err(format!("无法读取工作区显示设置：{error}")),
    }
}

fn write_preferences(path: &PathBuf, preferences: &WorkspacePreferences) -> Result<(), String> {
    let directory = path.parent().ok_or("工作区显示设置路径无效")?;
    std::fs::create_dir_all(directory).map_err(|error| format!("无法创建设置目录：{error}"))?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(preferences)
        .map_err(|error| format!("无法编码工作区显示设置：{error}"))?;
    std::fs::write(&temporary, bytes).map_err(|error| format!("无法保存显示设置：{error}"))?;
    std::fs::rename(temporary, path).map_err(|error| format!("无法更新显示设置：{error}"))
}
