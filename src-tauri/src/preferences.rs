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
    pub added_workspaces: Vec<String>,
    pub collapsed: Vec<String>,
    pub sidebar_collapsed: Option<bool>,
    pub sidebar_width: Option<f32>,
    pub group_by: Option<String>,
    pub order_by: Option<String>,
    pub archived_filter: Option<String>,
    /// 置顶的会话 ID（排序时置顶组内最前）。
    pub pinned: Vec<String>,
    /// Legacy session aliases from builds that saved renames in sidebar preferences.
    pub session_titles: HashMap<String, String>,
    /// 已归档的会话 ID。
    pub archived: Vec<String>,
    /// 外观：dark / light。
    pub theme_mode: Option<String>,
    /// 正文字号步进（官方 --dsh-content-font-delta，约 -2..=4）。
    pub content_font_delta: Option<i32>,
    /// 工作详情折叠：compact / standard / detailed / verbose。
    pub work_details: Option<String>,
    pub collapse_timing: Option<String>,
    pub folded_replies: Option<String>,
    pub workspace_open_with: Option<String>,
    /// Id of the blank session the client opened at startup, so the next launch
    /// reuses it instead of leaving one unused session on disk per launch.
    pub scratch_session: Option<String>,
    /// Chat HTTP(S) link destination: `sidebar` (built-in browser) or `new-tab`.
    pub link_open: Option<String>,
    /// Composer statistics detail level: `compact` or `detailed`.
    pub performance_usage: Option<String>,
    /// Files-search preferences by workspace root (official per-workspace
    /// sidebar-files search storage).
    pub files_search: HashMap<String, FilesSearchPreference>,
}

/// One Files-search exclusion rule as persisted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FilesSearchRulePreference {
    pub path: String,
    pub enabled: bool,
}

/// One workspace root's persisted Files-search preferences.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FilesSearchPreference {
    /// Exclusion rules, newest first.
    pub rules: Vec<FilesSearchRulePreference>,
    /// Whether hidden entries participate; defaults to hidden.
    pub show_hidden: Option<bool>,
    /// Whether `.gitignore` rules apply; defaults to on.
    pub respect_gitignore: Option<bool>,
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

#[tauri::command]
pub async fn load_developer_tools(webview: tauri::Webview) -> Result<bool, String> {
    let path = developer_tools_path(&webview)?;
    tokio::task::spawn_blocking(move || match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn save_developer_tools(webview: tauri::Webview, enabled: bool) -> Result<bool, String> {
    let path = developer_tools_path(&webview)?;
    tokio::task::spawn_blocking(move || {
        write_preferences(&path, &enabled)?;
        Ok(enabled)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn developer_tools_path(webview: &tauri::Webview) -> Result<PathBuf, String> {
    if webview.label() != "main" {
        return Err("只有主界面可以访问代码工作视图设置。".into());
    }
    Ok(preferences_path(webview.app_handle())?.with_file_name("developer-tools.json"))
}

pub(crate) fn preferences_path(app: &AppHandle) -> Result<PathBuf, String> {
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

fn write_preferences(path: &PathBuf, preferences: &impl Serialize) -> Result<(), String> {
    let directory = path.parent().ok_or("工作区显示设置路径无效")?;
    std::fs::create_dir_all(directory).map_err(|error| format!("无法创建设置目录：{error}"))?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(preferences)
        .map_err(|error| format!("无法编码工作区显示设置：{error}"))?;
    std::fs::write(&temporary, bytes).map_err(|error| format!("无法保存显示设置：{error}"))?;
    std::fs::rename(temporary, path).map_err(|error| format!("无法更新显示设置：{error}"))
}

#[cfg(test)]
mod tests {
    use super::{FilesSearchPreference, FilesSearchRulePreference, WorkspacePreferences};

    #[test]
    fn link_open_preference_round_trips_as_camel_case() {
        let preferences = WorkspacePreferences {
            link_open: Some("new-tab".to_string()),
            performance_usage: Some("compact".to_string()),
            ..WorkspacePreferences::default()
        };
        let encoded = serde_json::to_value(&preferences).expect("serialize preferences");
        assert_eq!(encoded["linkOpen"], "new-tab");
        assert_eq!(encoded["performanceUsage"], "compact");
        let decoded: WorkspacePreferences =
            serde_json::from_value(encoded).expect("deserialize preferences");
        assert_eq!(decoded.link_open.as_deref(), Some("new-tab"));
        assert_eq!(decoded.performance_usage.as_deref(), Some("compact"));
    }

    #[test]
    fn files_search_preferences_round_trip_as_camel_case() {
        let mut preferences = WorkspacePreferences::default();
        preferences.files_search.insert(
            "/Users/leon/Documents/KWork".to_string(),
            FilesSearchPreference {
                rules: vec![FilesSearchRulePreference {
                    path: "node_modules".to_string(),
                    enabled: false,
                }],
                show_hidden: Some(true),
                respect_gitignore: Some(false),
            },
        );
        preferences
            .files_search
            .insert("/tmp/other".to_string(), FilesSearchPreference::default());
        let encoded = serde_json::to_value(&preferences).expect("encode");
        let entry = &encoded["filesSearch"]["/Users/leon/Documents/KWork"];
        assert_eq!(entry["rules"][0]["path"], "node_modules");
        assert_eq!(entry["rules"][0]["enabled"], false);
        assert_eq!(entry["showHidden"], true);
        assert_eq!(entry["respectGitignore"], false);
        let decoded: WorkspacePreferences = serde_json::from_value(encoded).expect("decode");
        assert_eq!(
            decoded.files_search.get("/Users/leon/Documents/KWork"),
            preferences.files_search.get("/Users/leon/Documents/KWork")
        );
        // A file written before the search preferences existed still loads.
        let older: WorkspacePreferences =
            serde_json::from_str(r#"{"aliases":{}}"#).expect("older file");
        assert!(older.files_search.is_empty());
    }

    #[test]
    fn older_files_without_the_link_preference_still_load() {
        let decoded: WorkspacePreferences =
            serde_json::from_str(r#"{"workspaceOpenWith":"cursor"}"#)
                .expect("legacy preferences load");
        assert_eq!(decoded.link_open, None);
        assert_eq!(decoded.workspace_open_with.as_deref(), Some("cursor"));
    }
}
