//! Official Files-search exclusion rules and preference defaults.
//!
//! A rule names a file or directory the workspace search leaves out: a path
//! without `/` matches an entry of that name at any depth, a path with `/` is
//! relative to the workspace root. An excluded directory's contents are left out
//! with it. The list is editable in the Files search settings panel.

/// One exclusion rule as the settings list shows and stores it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SearchRule {
    /// Normalized name or `/`-separated workspace-relative path.
    pub path: String,
    /// Whether the search leaves this path out now.
    pub enabled: bool,
}

/// Why a typed rule path was refused; each reason has its own settings copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleInvalidReason {
    Empty,
    Nul,
    Absolute,
    Parent,
    Glob,
}

impl RuleInvalidReason {
    /// Official `settings.invalid.*` copy.
    pub fn label(self) -> &'static str {
        match self {
            Self::Empty => "请输入名称或相对路径。",
            Self::Nul => "路径不能包含 NUL 字符。",
            Self::Absolute => "请使用相对于工作区的路径。",
            Self::Parent => "路径不能包含 . 或 .. 段。",
            Self::Glob => "路径不能包含通配符 * ? [ ] { } !。",
        }
    }
}

/// Names excluded until the reader changes the list, in restore order.
pub const DEFAULT_EXCLUDED_PATHS: &[&str] = &[
    ".git",
    ".hg",
    ".next",
    ".pnpm-store",
    ".svn",
    ".turbo",
    ".yarn",
    "build",
    "coverage",
    "dist",
    "node_modules",
    ".DS_Store",
    ".Spotlight-V100",
    ".Trashes",
    ".fseventsd",
    "Thumbs.db",
    "desktop.ini",
    "$RECYCLE.BIN",
    "System Volume Information",
];

/// The default rule list, every rule enabled.
pub fn default_rules() -> Vec<SearchRule> {
    DEFAULT_EXCLUDED_PATHS
        .iter()
        .map(|path| SearchRule {
            path: (*path).to_owned(),
            enabled: true,
        })
        .collect()
}

/// Normalize a typed rule path, refusing anything the official editor refuses.
pub fn normalize_rule_path(input: &str) -> Result<String, RuleInvalidReason> {
    let slashed = input.trim().replace('\\', "/");
    if slashed.is_empty() {
        return Err(RuleInvalidReason::Empty);
    }
    if slashed.contains('\0') {
        return Err(RuleInvalidReason::Nul);
    }
    let relative = {
        let mut text = slashed.as_str();
        while text.starts_with(char::is_whitespace) || text.starts_with("./") {
            text = if let Some(rest) = text.strip_prefix("./") {
                rest
            } else {
                text.trim_start()
            };
        }
        text
    };
    let collapsed = relative.split('/').collect::<Vec<_>>().join("/");
    let collapsed = collapsed.replace("//", "/");
    if slashed.starts_with('/')
        || collapsed.starts_with('/')
        || collapsed.starts_with('~')
        || (collapsed.len() > 1
            && collapsed.as_bytes()[1] == b':'
            && collapsed.as_bytes()[0].is_ascii_alphabetic())
    {
        return Err(RuleInvalidReason::Absolute);
    }
    let trimmed = collapsed.trim_matches(|ch: char| ch == '/' || ch.is_whitespace());
    if trimmed.is_empty() {
        return Err(RuleInvalidReason::Empty);
    }
    if trimmed
        .chars()
        .any(|ch| matches!(ch, '*' | '?' | '[' | ']' | '{' | '}' | '!'))
    {
        return Err(RuleInvalidReason::Glob);
    }
    if trimmed
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Err(RuleInvalidReason::Parent);
    }
    Ok(trimmed.to_owned())
}

/// Paths the enabled rules pass to the search.
pub fn enabled_paths(rules: &[SearchRule]) -> Vec<String> {
    rules
        .iter()
        .filter(|rule| rule.enabled)
        .map(|rule| rule.path.clone())
        .collect()
}

/// Add one normalized path, or re-enable an existing rule. New rules go first.
pub fn add_rule(rules: &mut Vec<SearchRule>, path: String) {
    if rules.iter().any(|rule| rule.path == path) {
        remove_rule(rules, &path);
    }
    rules.insert(
        0,
        SearchRule {
            path,
            enabled: true,
        },
    );
}

/// Remove one rule.
pub fn remove_rule(rules: &mut Vec<SearchRule>, path: &str) {
    rules.retain(|rule| rule.path != path);
}

/// Toggle one rule's enabled flag.
pub fn toggle_rule(rules: &mut [SearchRule], path: &str) {
    if let Some(rule) = rules.iter_mut().find(|rule| rule.path == path) {
        rule.enabled = !rule.enabled;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_matches_the_official_editor() {
        assert_eq!(
            normalize_rule_path("  node_modules  ").unwrap(),
            "node_modules"
        );
        // Leading `./` is dropped, then the `..` segment is refused.
        assert_eq!(
            normalize_rule_path("./src/../x"),
            Err(RuleInvalidReason::Parent)
        );
        assert_eq!(normalize_rule_path("src//nested/").unwrap(), "src/nested");
        // Backslashes become separators first, so a leading one is absolute.
        assert_eq!(
            normalize_rule_path("\\build\\out"),
            Err(RuleInvalidReason::Absolute)
        );
        assert_eq!(normalize_rule_path("build\\out").unwrap(), "build/out");
        assert_eq!(normalize_rule_path(""), Err(RuleInvalidReason::Empty));
        assert_eq!(normalize_rule_path("a\0b"), Err(RuleInvalidReason::Nul));
        assert_eq!(
            normalize_rule_path("/abs/path"),
            Err(RuleInvalidReason::Absolute)
        );
        assert_eq!(
            normalize_rule_path("C:/Windows"),
            Err(RuleInvalidReason::Absolute)
        );
        assert_eq!(
            normalize_rule_path("~/home"),
            Err(RuleInvalidReason::Absolute)
        );
        assert_eq!(
            normalize_rule_path("src/../x"),
            Err(RuleInvalidReason::Parent)
        );
        assert_eq!(normalize_rule_path("*.log"), Err(RuleInvalidReason::Glob));
        assert_eq!(
            RuleInvalidReason::Glob.label(),
            "路径不能包含通配符 * ? [ ] { } !。"
        );
    }

    #[test]
    fn rules_add_toggle_remove_and_restore() {
        let mut rules = default_rules();
        assert!(rules.iter().all(|rule| rule.enabled));
        assert!(enabled_paths(&rules).contains(&"node_modules".to_owned()));
        toggle_rule(&mut rules, "node_modules");
        assert!(!enabled_paths(&rules).contains(&"node_modules".to_owned()));
        add_rule(&mut rules, "node_modules".to_owned());
        assert_eq!(rules[0].path, "node_modules");
        assert!(rules[0].enabled);
        add_rule(&mut rules, "custom-dir".to_owned());
        assert_eq!(rules[0].path, "custom-dir");
        remove_rule(&mut rules, "custom-dir");
        assert!(!rules.iter().any(|rule| rule.path == "custom-dir"));
        let restored = default_rules();
        assert_eq!(restored.len(), DEFAULT_EXCLUDED_PATHS.len());
    }
}
