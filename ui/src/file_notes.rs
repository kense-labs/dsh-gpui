//! Official Files-pane notes: the copy for tree read failures and hidden rows.
//!
//! `sidebarFiles` names one code per failure because a directory that is gone,
//! one outside the workspace, and a path that is not a directory each suggest a
//! different next step. This module maps the Host's raw message onto that copy.

/// Official `error.*` line for one failed directory read.
pub(crate) fn file_error_note(error: &str) -> String {
    let lower = error.to_lowercase();
    if lower.contains("not found") || lower.contains("enoent") {
        "这个目录不在了。可能已被移动或删除。".to_owned()
    } else if lower.contains("outside") {
        "这个目录在工作区之外，侧栏不会读取它。".to_owned()
    } else if lower.contains("not a directory") || lower.contains("enotdir") {
        "这不是一个目录。".to_owned()
    } else {
        format!("读取失败：{error}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_use_the_official_lines() {
        assert_eq!(
            file_error_note("ENOENT: no such file or directory"),
            "这个目录不在了。可能已被移动或删除。"
        );
        assert_eq!(
            file_error_note("path is outside the workspace"),
            "这个目录在工作区之外，侧栏不会读取它。"
        );
        assert_eq!(
            file_error_note("ENOTDIR: not a directory"),
            "这不是一个目录。"
        );
        assert_eq!(
            file_error_note("permission denied"),
            "读取失败：permission denied"
        );
    }
}
