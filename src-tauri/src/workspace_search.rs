//! Official `workspaceFiles.search`: bounded shallow-first name/path search.
//!
//! The Host lists the workspace with ripgrep (`--files`) and filters in the
//! client-facing result: a query without `/` matches entry names, a query with
//! `/` matches workspace-relative paths, directories derive from listed files,
//! and results sort by depth then path. This module reproduces that contract in
//! process, honouring the same hidden-entry, `.gitignore`, and exclusion rules.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use serde::Serialize;
use serde_json::{Value, json};

/// Cap on returned matches (official `search.maxResults` default).
const MAX_RESULTS: usize = 512;
/// Cap on files considered from the walk (official `search.maxEntries` default).
const MAX_ENTRIES: usize = 100_000;

/// VCS metadata never contributes files or inferred directories.
const VCS_NAMES: &[&str] = &[".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

/// One matched entry: `/`-separated workspace-relative path plus its kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSearchMatch {
    pub path: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
}

/// A search's settled answer.
#[derive(Debug, Default)]
pub struct WorkspaceSearchAnswer {
    pub matches: Vec<WorkspaceSearchMatch>,
    pub truncated: bool,
    pub incomplete: bool,
}

/// Whether one relative path is left out by the exclusion rules.
///
/// A rule without `/` names an entry at any depth; a rule with `/` is a
/// workspace-relative file or directory prefix.
fn excluded(relative: &str, rules: &[String]) -> bool {
    rules.iter().any(|rule| {
        if rule.contains('/') {
            relative == rule || relative.starts_with(&format!("{rule}/"))
        } else {
            // A bare name matches that file or directory at any depth; an
            // excluded directory's contents are left out with it.
            relative.split('/').any(|segment| segment == rule)
        }
    })
}

/// Whether a path segment is VCS metadata.
fn vcs_name(relative: &str) -> bool {
    relative
        .split('/')
        .any(|segment| VCS_NAMES.contains(&segment))
}

/// Search one workspace. Errors only when the root cannot be walked at all.
pub fn search_workspace(
    root: &Path,
    query: &str,
    include_hidden: bool,
    respect_gitignore: bool,
    excluded_paths: &[String],
) -> Result<WorkspaceSearchAnswer, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(WorkspaceSearchAnswer::default());
    }
    let needle = query.to_lowercase();
    let match_path = query.contains('/');
    let mut walker = WalkBuilder::new(root);
    walker
        .hidden(!include_hidden)
        .ignore(respect_gitignore)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore)
        .parents(respect_gitignore)
        // The official search reads `.gitignore` even when the workspace is not
        // a git repository itself (matching ripgrep's `--no-ignore` toggle).
        .require_git(false)
        .follow_links(false)
        .sort_by_file_path(|a, b| a.cmp(b));
    let mut answers: std::collections::BTreeMap<String, &'static str> =
        std::collections::BTreeMap::new();
    let mut files = 0usize;
    let mut incomplete = false;
    for entry in walker.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                incomplete = true;
                continue;
            }
        };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if vcs_name(&relative) {
            continue;
        }
        if excluded(&relative, excluded_paths) {
            continue;
        }
        files += 1;
        if files > MAX_ENTRIES {
            incomplete = true;
            break;
        }
        // Directories derive from listed files. A directory can match the
        // query on its own name (or path) even when no file under it matches,
        // so every ancestor participates independently — the official
        // `workspaceFiles.search` admits those directories too.
        let mut prefix = String::new();
        let segments: Vec<&str> = relative.split('/').collect();
        for segment in &segments[..segments.len().saturating_sub(1)] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            let haystack = if match_path {
                prefix.as_str()
            } else {
                *segment
            };
            if haystack.to_lowercase().contains(&needle) {
                answers.entry(prefix.clone()).or_insert("directory");
            }
        }
        let name = relative.rsplit('/').next().unwrap_or(&relative);
        let haystack = if match_path { relative.as_str() } else { name };
        if !haystack.to_lowercase().contains(&needle) {
            continue;
        }
        answers.insert(relative.clone(), "file");
        let mut parent = relative.as_str();
        while let Some((head, _)) = parent.rsplit_once('/') {
            answers.entry(head.to_owned()).or_insert("directory");
            parent = head;
        }
    }
    let mut matches: Vec<WorkspaceSearchMatch> = answers
        .into_iter()
        .map(|(path, kind)| WorkspaceSearchMatch { path, kind })
        .collect();
    matches.sort_by(|a, b| {
        let depth = |path: &str| path.matches('/').count();
        depth(&a.path)
            .cmp(&depth(&b.path))
            .then_with(|| a.path.cmp(&b.path))
    });
    let truncated = matches.len() > MAX_RESULTS;
    matches.truncate(MAX_RESULTS);
    Ok(WorkspaceSearchAnswer {
        matches,
        truncated,
        incomplete,
    })
}

/// Official `workspaceFiles.search` command.
#[tauri::command]
pub async fn search_workspace_files(
    workspace: String,
    query: String,
    include_hidden: bool,
    respect_gitignore: bool,
    excluded_paths: Vec<String>,
    request_id: u64,
) -> Result<Value, String> {
    if workspace.trim().is_empty() {
        return Err("缺少工作区目录".to_string());
    }
    if query.trim().is_empty() {
        return Ok(json!({
            "requestId": request_id,
            "matches": [],
            "truncated": false,
            "incomplete": false,
        }));
    }
    let root = PathBuf::from(&workspace);
    if !root.is_dir() {
        return Err(format!("工作区目录不存在：{workspace}"));
    }
    let answer = tokio::task::spawn_blocking(move || {
        search_workspace(
            &root,
            &query,
            include_hidden,
            respect_gitignore,
            &excluded_paths,
        )
    })
    .await
    .map_err(|error| format!("搜索工作区失败：{error}"))??;
    Ok(json!({
        "requestId": request_id,
        "matches": answer.matches,
        "truncated": answer.truncated,
        "incomplete": answer.incomplete,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src/nested/helper.rs"), "").unwrap();
        fs::write(root.join("node_modules/pkg/index.js"), "").unwrap();
        fs::write(root.join(".hidden/secret.txt"), "").unwrap();
        fs::write(root.join("README.md"), "").unwrap();
        fs::write(root.join(".gitignore"), "build/\n*.log\n").unwrap();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join("build/out.txt"), "").unwrap();
        fs::write(root.join("debug.log"), "").unwrap();
        dir
    }

    fn paths(answer: &WorkspaceSearchAnswer) -> Vec<&str> {
        answer.matches.iter().map(|m| m.path.as_str()).collect()
    }

    #[test]
    fn names_match_without_case_and_directories_derive_from_files() {
        let dir = workspace();
        let answer = search_workspace(dir.path(), "RS", false, true, &[]).unwrap();
        // main.rs and helper.rs match by name; src and src/nested are inferred.
        assert!(paths(&answer).contains(&"src/main.rs"));
        assert!(paths(&answer).contains(&"src/nested/helper.rs"));
        assert!(paths(&answer).contains(&"src/nested"));
        let depths: Vec<usize> = answer
            .matches
            .iter()
            .map(|m| m.path.matches('/').count())
            .collect();
        assert!(
            depths.windows(2).all(|pair| pair[0] <= pair[1]),
            "shallow first"
        );
    }

    #[test]
    fn matching_directories_appear_without_a_matching_file() {
        let dir = workspace();
        fs::write(dir.path().join("src/lib.rs"), "").unwrap();
        // No file is named `src`, but the directory itself matches the query.
        let answer = search_workspace(dir.path(), "src", false, true, &[]).unwrap();
        assert!(paths(&answer).contains(&"src"));
        // `nested` does not contain the query, so it is not admitted on its own.
        assert!(!paths(&answer).contains(&"src/nested"));
        // A slashed query matches directory paths the same way.
        let answer = search_workspace(dir.path(), "src/nested", false, true, &[]).unwrap();
        assert!(paths(&answer).contains(&"src/nested"));
        assert!(paths(&answer).contains(&"src/nested/helper.rs"));
    }

    #[test]
    fn slashed_queries_match_paths_and_exclusions_apply() {
        let dir = workspace();
        let answer = search_workspace(dir.path(), "src/", false, true, &[]).unwrap();
        assert!(paths(&answer).contains(&"src/main.rs"));
        assert!(paths(&answer).contains(&"src/nested/helper.rs"));
        // node_modules pairs with an exclusion rule by name.
        let excluded_rules = vec!["node_modules".to_owned()];
        let answer = search_workspace(dir.path(), "js", false, true, &excluded_rules).unwrap();
        assert!(
            !paths(&answer)
                .iter()
                .any(|path| path.contains("node_modules"))
        );
        // A path rule excludes that exact directory tree.
        let path_rules = vec!["src/nested".to_owned()];
        let answer = search_workspace(dir.path(), "helper", false, true, &path_rules).unwrap();
        assert!(paths(&answer).is_empty());
    }

    #[test]
    fn hidden_and_gitignore_follow_the_preferences() {
        let dir = workspace();
        let hidden_off = search_workspace(dir.path(), "secret", false, true, &[]).unwrap();
        assert!(paths(&hidden_off).is_empty());
        let hidden_on = search_workspace(dir.path(), "secret", true, true, &[]).unwrap();
        assert!(paths(&hidden_on).contains(&".hidden/secret.txt"));
        let ignored_off = search_workspace(dir.path(), "debug.log", false, true, &[]).unwrap();
        assert!(paths(&ignored_off).is_empty());
        let ignored_on = search_workspace(dir.path(), "debug.log", false, false, &[]).unwrap();
        assert!(paths(&ignored_on).contains(&"debug.log"));
        // VCS metadata stays out even when hidden entries are included.
        let vcs = search_workspace(dir.path(), ".git", true, false, &[]).unwrap();
        assert!(!paths(&vcs).iter().any(|path| *path == ".git"));
    }
}
