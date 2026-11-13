//! Official chat step-process activity titles (ui-chat `processTitle` / `activity`).

/// Ranked category for a closed or live process group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProcessKind {
    Thinking,
    Read,
    ReadImage,
    Write,
    Search,
    Edit,
    Commands,
    Code,
    WebSearch,
    WebFetch,
    Subagents,
    Plan,
    Questions,
    Tools,
}

impl ProcessKind {
    pub fn done_label(self) -> &'static str {
        match self {
            Self::Thinking => "已完成分析",
            Self::Read => "已读取文件",
            Self::ReadImage => "已读取图片",
            Self::Write => "已写入文件",
            Self::Search => "已搜索代码",
            Self::Edit => "修改了文件",
            Self::Commands => "执行了命令",
            Self::Code => "运行了代码",
            Self::WebSearch => "已搜索网页",
            Self::WebFetch => "已访问网页",
            Self::Subagents => "已协调子智能体",
            Self::Plan => "更新了计划",
            Self::Questions => "向用户提出了问题",
            Self::Tools => "已调用工具",
        }
    }

    /// Label while the model is still streaming a tool call's arguments
    /// (official `message.stepProcess.prepare.*`).
    pub fn prepare_label(self) -> &'static str {
        match self {
            Self::Thinking => "准备调用工具",
            Self::Read => "准备读取文件",
            Self::ReadImage => "准备读取图片",
            Self::Write => "准备写入文件",
            Self::Search => "准备搜索代码",
            Self::Edit => "准备编辑文件",
            Self::Commands => "准备运行命令",
            Self::Code => "准备运行代码",
            Self::WebSearch => "准备搜索网页",
            Self::WebFetch => "准备访问网页",
            Self::Subagents => "准备协调子智能体",
            Self::Plan => "准备更新计划",
            Self::Questions => "准备提问",
            Self::Tools => "准备调用工具",
        }
    }

    pub fn live_label(self) -> &'static str {
        match self {
            Self::Thinking => "正在分析请求",
            Self::Read => "正在读取文件",
            Self::ReadImage => "正在读取图片",
            Self::Write => "正在写入文件",
            Self::Search => "正在搜索代码",
            Self::Edit => "正在编辑文件",
            Self::Commands => "正在运行命令",
            Self::Code => "正在运行代码",
            Self::WebSearch => "正在搜索网页",
            Self::WebFetch => "正在访问网页",
            Self::Subagents => "正在协调子智能体",
            Self::Plan => "正在更新计划",
            Self::Questions => "等待你的操作",
            Self::Tools => "正在调用工具",
        }
    }
}

/// Map a tool name (ACP `title` / durable call name) to an activity kind.
pub fn activity(name: &str) -> ProcessKind {
    match name {
        "read" => ProcessKind::Read,
        "read_image" => ProcessKind::ReadImage,
        "grep" | "glob" => ProcessKind::Search,
        "write" => ProcessKind::Write,
        "edit" | "apply_patch" => ProcessKind::Edit,
        "bash" | "pwsh" | "exec_command" | "write_stdin" => ProcessKind::Commands,
        "run_code" => ProcessKind::Code,
        "web_search" => ProcessKind::WebSearch,
        "web_fetch" => ProcessKind::WebFetch,
        "subagent" => ProcessKind::Subagents,
        "todo_write" | "create_goal" | "update_goal" | "get_goal" => ProcessKind::Plan,
        "ask_user_question" | "request_user_input" => ProcessKind::Questions,
        other if other.ends_with("_inspect") => ProcessKind::Search,
        other if other.starts_with("terminal_") => ProcessKind::Commands,
        other if other.starts_with("subagent_") => ProcessKind::Subagents,
        _ => ProcessKind::Tools,
    }
}

#[cfg(any(target_family = "wasm", test))]
pub(crate) fn live_tool_detail(name: &str, arguments: &str) -> String {
    // The todo row summarizes the whole list (`3/4 已完成 · 当前任务`) rather than
    // naming one argument, so it is derived before the generic key scan.
    if name == "todo_write"
        && let Some(summary) = todo_summary(arguments)
    {
        return todo_summary_text(&summary);
    }
    const KEYS: &[&str] = &[
        "title",
        "description",
        "objective",
        "task",
        "task_name",
        "name",
        "question",
        "questions",
        "prompt",
        "message",
        "command",
        "cmd",
        "queries",
        "query",
        "pattern",
        "url",
        "uri",
        "file_path",
        "path",
        "target",
        "action",
        "status",
    ];
    if let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments) {
        for &key in KEYS {
            let value = &args[key];
            let detail = if key == "questions" {
                value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|item| argument_detail(&item["question"]))
                    .find(|detail| !detail.is_empty())
                    .unwrap_or_default()
            } else {
                argument_detail(value)
            };
            if !detail.is_empty() {
                return detail;
            }
        }
    }
    normalize_live_detail(name)
}

/// 官方行内摘要：`{done}/{total} 已完成`，有当前任务时再接 ` · 任务名`。
///
/// 并行数量（`+N`）在官方里是**不收缩的独立片段**；这里是纯文本摘要，所以接在
/// 任务名之后，与官方的可见文案一致。
pub fn todo_summary_text(summary: &PlanSummary) -> String {
    let head = format!("{}/{} 已完成", summary.done, summary.total);
    match summary.active_content.as_deref() {
        Some(content) => {
            let extra = if summary.active_extra > 0 {
                format!(" +{}", summary.active_extra)
            } else {
                String::new()
            };
            format!("{head} · {content}{extra}")
        }
        None => head,
    }
}

/// 官方 `planSummary` 的结果：计数与摘要的两半。
///
/// 两半故意不预先拼接：行会截断摘要文本，而拼在任务名末尾的计数正是窄行最先
/// 被裁掉的部分——偏偏那时它最有信息量。调用方把 `active_extra` 放在不收缩的
/// 独立片段里。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanSummary {
    pub done: usize,
    pub total: usize,
    /// 第一个 `in_progress` 的内容；首个活动项不可用时为 `None`。
    pub active_content: Option<String>,
    /// 首个之外仍在进行的数量；没有 `active_content` 时为 0。
    pub active_extra: usize,
}

/// 官方 `planSummary`：从整份待办快照推导计数与当前任务。
///
/// 可能有多个 `in_progress`（并行工作会同时跑多个任务），因此只取一项会静默丢掉
/// 其余；这里点名第一项并数出其余并行数量。首个活动项的内容缺失、类型不对或去空白
/// 后为空时 `active_content` 为 `None`——被拒绝的调用会原样保留参数，所以模型给的
/// 字段任何一项都可能不可用。
pub fn plan_summary(todos: &[serde_json::Value]) -> PlanSummary {
    let is = |item: &serde_json::Value, status: &str| item["status"].as_str() == Some(status);
    let active: Vec<&serde_json::Value> = todos
        .iter()
        .filter(|item| is(item, "in_progress"))
        .collect();
    let named = active
        .first()
        .and_then(|item| item["content"].as_str())
        .map(str::trim)
        .filter(|content| !content.is_empty())
        .map(str::to_owned);
    PlanSummary {
        done: todos.iter().filter(|item| is(item, "completed")).count(),
        total: todos.len(),
        active_extra: if named.is_some() {
            active.len().saturating_sub(1)
        } else {
            0
        },
        active_content: named,
    }
}

/// 待办工具行的一行摘要：官方 `todo.completed` + 当前任务。
///
/// 参数不是合法 JSON（流式截断或模型给坏 JSON）时返回 `None`，由调用方回退到
/// 通用摘要——与官方 `summarize` 的降级一致。
pub fn todo_summary(arguments: &str) -> Option<PlanSummary> {
    let parsed: serde_json::Value = serde_json::from_str(arguments).ok()?;
    let todos = parsed.get("todos")?.as_array()?;
    // 官方要求每一项都是对象；出现 null 元素时整条回退。
    if todos.iter().any(|item| !item.is_object()) {
        return None;
    }
    Some(plan_summary(todos))
}

#[cfg(any(target_family = "wasm", test))]
fn argument_detail(value: &serde_json::Value) -> String {
    if let Some(text) = value.as_str() {
        return normalize_live_detail(text);
    }
    value
        .as_array()
        .and_then(|values| {
            values
                .iter()
                .map(serde_json::Value::as_str)
                .collect::<Option<Vec<_>>>()
        })
        .map(|values| normalize_live_detail(&values.join(", ")))
        .unwrap_or_default()
}

#[cfg(any(target_family = "wasm", test))]
fn normalize_live_detail(text: &str) -> String {
    use unicode_segmentation::UnicodeSegmentation;

    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((end, _)) = normalized.grapheme_indices(true).nth(159)
        && normalized[end..].graphemes(true).nth(1).is_some()
    {
        return format!("{}…", normalized[..end].trim_end());
    }
    normalized
}

#[cfg(any(target_family = "wasm", test))]
pub(crate) fn live_reasoning_detail<'a>(latest_first: impl Iterator<Item = &'a str>) -> String {
    const SIGNATURE_START: &str = "<signature>";
    const SIGNATURE_END: &str = "</signature>";
    for text in latest_first {
        let text = if let Some(signature) = text.strip_prefix(SIGNATURE_START) {
            signature
                .split_once(SIGNATURE_END)
                .map_or("", |(_, reasoning)| reasoning)
        } else if SIGNATURE_START.starts_with(text) {
            ""
        } else {
            text
        };
        let lines: Vec<_> = text.lines().collect();
        for paragraph in lines.rsplit(|line| line.trim().is_empty()) {
            let detail = normalize_live_detail(&paragraph.join(" ").replace("**", ""));
            if !detail.is_empty() {
                return detail;
            }
        }
    }
    String::new()
}

/// Compose a closed group's localized title from its top three categories (official `processTitle`).
pub fn process_title(kinds: &[ProcessKind]) -> String {
    if kinds.is_empty() {
        return ProcessKind::Thinking.done_label().to_string();
    }
    let labels: Vec<&'static str> = kinds.iter().take(3).map(|kind| kind.done_label()).collect();
    let first = labels[0];
    if labels.len() == 1 {
        return first.to_string();
    }
    let continuation = |label: &str| {
        let mut chars = label.chars();
        match chars.next() {
            Some(first_char) => {
                let lowered: String = first_char.to_lowercase().collect();
                format!("{lowered}{}", chars.as_str())
            }
            None => String::new(),
        }
    };
    let strip_shared = |label: &str| {
        if first.starts_with('已') && label.starts_with('已') {
            continuation(&label[label.char_indices().nth(1).map(|(i, _)| i).unwrap_or(0)..])
        } else {
            continuation(label)
        }
    };
    if labels.len() == 2 {
        let second = strip_shared(labels[1]);
        return format!("{first}并{second}");
    }
    let mut title = first.to_string();
    for label in labels.iter().skip(1) {
        title.push('，');
        title.push_str(&continuation(label));
    }
    if kinds.len() > 3 {
        format!("{title}等")
    } else {
        title
    }
}

/// Rank tool names by distinct kind count (ties keep first-seen order).
pub fn ranked_kinds(tool_names: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<ProcessKind> {
    use std::collections::HashMap;
    let mut counts: HashMap<ProcessKind, usize> = HashMap::new();
    let mut order: Vec<ProcessKind> = Vec::new();
    for name in tool_names {
        let kind = activity(name.as_ref());
        if let Some(count) = counts.get_mut(&kind) {
            *count += 1;
        } else {
            counts.insert(kind, 1);
            order.push(kind);
        }
    }
    let mut ranked: Vec<(ProcessKind, usize, usize)> = order
        .into_iter()
        .enumerate()
        .map(|(index, kind)| (kind, counts.get(&kind).copied().unwrap_or(0), index))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
    ranked.into_iter().map(|(kind, _, _)| kind).collect()
}

/// Official `formatCacheHitPercent`: cache-read share of billed prompt tokens.
/// Never rounds a partial hit up to 100.
pub fn format_cache_hit_percent(cache_read: u64, prompt_tokens: u64) -> Option<String> {
    if prompt_tokens == 0 {
        return None;
    }
    if prompt_tokens == cache_read {
        return Some("100".to_string());
    }
    let missed = prompt_tokens.saturating_sub(cache_read);
    if missed == 0 {
        return Some("100".to_string());
    }
    // Integer percent that stays strictly below 100 for partial hits.
    let pct = ((cache_read as u128 * 100) / prompt_tokens as u128) as u64;
    if pct < 100 {
        return Some(pct.to_string());
    }
    // Need finer precision so a near-miss never displays as 100.
    let mut places = 1u32;
    let mut scale = 1000u128;
    while (cache_read as u128 * scale) / prompt_tokens as u128 >= scale && places < 6 {
        places += 1;
        scale *= 10;
    }
    let units = (cache_read as u128 * scale) / prompt_tokens as u128;
    let whole = units / 10u128.pow(places);
    let frac = units % 10u128.pow(places);
    if frac == 0 {
        Some(whole.to_string())
    } else {
        let frac_str = format!("{:0width$}", frac, width = places as usize)
            .trim_end_matches('0')
            .to_string();
        if frac_str.is_empty() {
            Some(whole.to_string())
        } else {
            Some(format!("{whole}.{frac_str}"))
        }
    }
}

/// Compact token display: 120 / 3.2K / 3.2M.
pub fn format_tokens(value: u64) -> String {
    if value < 1_000 {
        return value.to_string();
    }
    if value < 1_000_000 {
        let scaled = value as f64 / 1_000.0;
        let text = if scaled >= 100.0 {
            format!("{}", scaled.round() as u64)
        } else {
            let rounded = (scaled * 10.0).round() / 10.0;
            if (rounded - rounded.floor()).abs() < f64::EPSILON {
                format!("{}", rounded as u64)
            } else {
                format!("{rounded}")
            }
        };
        return format!("{text}K");
    }
    let scaled = value as f64 / 1_000_000.0;
    let text = if scaled >= 100.0 {
        format!("{}", scaled.round() as u64)
    } else {
        let rounded = (scaled * 10.0).round() / 10.0;
        if (rounded - rounded.floor()).abs() < f64::EPSILON {
            format!("{}", rounded as u64)
        } else {
            format!("{rounded}")
        }
    };
    format!("{text}M")
}

pub fn format_tokens_per_second(tps: f64) -> String {
    let clamped = tps.max(0.);
    if clamped >= 10. {
        format!("{}", clamped.round() as u64)
    } else {
        let rounded = (clamped * 10.0).round() / 10.0;
        if (rounded - rounded.floor()).abs() < f64::EPSILON {
            format!("{}", rounded as u64)
        } else {
            format!("{rounded}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_todo_row_summarizes_the_whole_list() {
        // Copied from a real `todo_write` call so the row stays pinned to the
        // payload the model actually sends.
        let args = r#"{"todos": [
            {"content": "扫描磁盘与主目录整体占用，定位大文件/目录", "status": "in_progress"},
            {"content": "检查废纸篓、缓存、日志、临时文件等典型残留", "status": "pending"},
            {"content": "检查开发工具缓存（Xcode/npm/cargo/pip/docker 等）", "status": "pending"}
        ]}"#;
        assert_eq!(
            live_tool_detail("todo_write", args),
            "0/3 已完成 · 扫描磁盘与主目录整体占用，定位大文件/目录"
        );
    }

    #[test]
    fn a_finished_list_reports_counts_without_a_current_task() {
        let args = r#"{"todos": [
            {"content": "一", "status": "completed"},
            {"content": "二", "status": "completed"}
        ]}"#;
        assert_eq!(live_tool_detail("todo_write", args), "2/2 已完成");
    }

    #[test]
    fn parallel_active_items_report_the_extra_count() {
        // Several items may be in progress at once; naming only the first would
        // silently drop the rest.
        let args = r#"{"todos": [
            {"content": "主任务", "status": "in_progress"},
            {"content": "并行任务", "status": "in_progress"},
            {"content": "另一个", "status": "in_progress"},
            {"content": "已完成项", "status": "completed"}
        ]}"#;
        assert_eq!(
            live_tool_detail("todo_write", args),
            "1/4 已完成 · 主任务 +2"
        );
        let summary = todo_summary(args).unwrap();
        assert_eq!(summary.active_extra, 2);
        assert_eq!(summary.done, 1);
        assert_eq!(summary.total, 4);
    }

    #[test]
    fn malformed_or_unusable_todo_args_fall_back_to_the_generic_summary() {
        // Mid-stream truncation and rejected calls keep their args verbatim.
        assert!(todo_summary("{\"todos\": [").is_none());
        assert!(todo_summary("not json").is_none());
        assert!(todo_summary("null").is_none());
        assert!(todo_summary("{\"todos\": null}").is_none());
        assert!(todo_summary("{}").is_none());
        // A null item makes the whole list unusable, matching the official rule.
        assert!(todo_summary(r#"{"todos": [null]}"#).is_none());
        // The caller then falls through to the pre-existing generic detail, which
        // for unparseable args is the tool name itself.
        assert_eq!(
            live_tool_detail("todo_write", "{\"todos\": ["),
            "todo_write"
        );
    }

    #[test]
    fn active_items_without_usable_content_still_report_counts() {
        // `content` missing, mistyped or blank: counts are known-good, so the row
        // shows them and drops only the active-item clause.
        for args in [
            r#"{"todos": [{"status": "in_progress"}]}"#,
            r#"{"todos": [{"content": null, "status": "in_progress"}]}"#,
            r#"{"todos": [{"content": "   ", "status": "in_progress"}]}"#,
            r#"{"todos": [{"content": 42, "status": "in_progress"}]}"#,
        ] {
            assert_eq!(live_tool_detail("todo_write", args), "0/1 已完成", "{args}");
            let summary = todo_summary(args).unwrap();
            assert_eq!(summary.active_content, None);
            assert_eq!(summary.active_extra, 0);
        }
    }

    #[test]
    fn other_tools_keep_the_generic_argument_scan() {
        assert_eq!(
            live_tool_detail("read", r#"{"file_path": "/tmp/a"}"#),
            "/tmp/a"
        );
    }

    #[test]
    #[test]
    fn prepare_labels_follow_the_official_strings() {
        assert_eq!(ProcessKind::Read.prepare_label(), "准备读取文件");
        assert_eq!(ProcessKind::Commands.prepare_label(), "准备运行命令");
        assert_eq!(ProcessKind::Thinking.prepare_label(), "准备调用工具");
        assert_eq!(ProcessKind::Tools.prepare_label(), "准备调用工具");
        assert_eq!(
            crate::process::activity("bash").prepare_label(),
            "准备运行命令"
        );
        assert_eq!(
            crate::process::activity("str_replace_editor").prepare_label(),
            "准备调用工具"
        );
    }

    fn activity_maps_official_tool_names() {
        assert_eq!(activity("bash"), ProcessKind::Commands);
        assert_eq!(activity("read"), ProcessKind::Read);
        assert_eq!(activity("web_fetch"), ProcessKind::WebFetch);
        assert_eq!(activity("todo_write"), ProcessKind::Plan);
    }

    #[test]
    fn cache_hit_never_rounds_partial_to_100() {
        assert_eq!(format_cache_hit_percent(100, 100).as_deref(), Some("100"));
        assert_eq!(format_cache_hit_percent(99, 100).as_deref(), Some("99"));
        assert_eq!(format_cache_hit_percent(0, 0), None);
    }

    #[test]
    fn process_title_joins_like_official_zh() {
        assert_eq!(
            process_title(&[ProcessKind::Commands, ProcessKind::Read, ProcessKind::Tools]),
            "执行了命令，已读取文件，已调用工具"
        );
        assert_eq!(
            process_title(&[ProcessKind::Commands, ProcessKind::Read]),
            "执行了命令并已读取文件"
        );
        assert_eq!(
            process_title(&[ProcessKind::Read, ProcessKind::Tools]),
            "已读取文件并调用工具"
        );
        assert_eq!(
            process_title(&[
                ProcessKind::Commands,
                ProcessKind::Tools,
                ProcessKind::WebFetch,
                ProcessKind::Search
            ]),
            "执行了命令，已调用工具，已访问网页等"
        );
    }

    #[test]
    fn ranked_kinds_prefer_frequency() {
        let ranked = ranked_kinds(["bash", "bash", "read", "web_fetch"]);
        assert_eq!(ranked[0], ProcessKind::Commands);
        assert!(ranked.contains(&ProcessKind::Read));
        assert!(ranked.contains(&ProcessKind::WebFetch));
    }

    #[test]
    fn live_detail_prioritizes_tool_arguments_and_keeps_graphemes_intact() {
        let args = r#"{"command":"pwd","questions":[{},{"question":"Which    path?"}]}"#;
        assert_eq!(live_tool_detail("ask_user_question", args), "Which path?");
        let grapheme = "👩🏽‍💻";
        let long_detail = grapheme.repeat(161);
        let input = serde_json::json!({"title": long_detail}).to_string();
        assert_eq!(
            live_tool_detail("read", &input),
            format!("{}…", grapheme.repeat(159))
        );
        assert_eq!(
            live_reasoning_detail(
                ["<signature>private</signature>\nfirst\n\t\n**Latest**"].into_iter()
            ),
            "Latest"
        );
    }

    #[test]
    fn format_tokens_matches_compact_scale() {
        assert_eq!(format_tokens(120), "120");
        assert_eq!(format_tokens(3_200_000), "3.2M");
        assert_eq!(format_tokens_per_second(146.2), "146");
    }
}
