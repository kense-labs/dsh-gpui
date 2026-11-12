//! “/ 调用指令”面板目录，WASM UI 与测试共享。

use kui::primitives::glyph::Glyph;
use serde_json::Value;
use std::collections::BTreeSet;

/// 指令面板条目。`host=true` 的条目按会话的 ACP 命令目录决定可用性，
/// 与 Harness 的动态目录一致，不再把「暂不支持」写死在界面里。
#[derive(Clone, Copy)]
pub(crate) struct CommandSpec {
    pub(crate) section: &'static str,
    pub(crate) key: &'static str,
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
    pub(crate) glyph: Glyph,
    pub(crate) host: bool,
    pub(crate) enabled: bool,
}

fn command(
    section: &'static str,
    key: &'static str,
    title: &'static str,
    description: &'static str,
    glyph: Glyph,
) -> CommandSpec {
    CommandSpec {
        section,
        key,
        title,
        description,
        glyph,
        host: false,
        enabled: true,
    }
}

fn host_command(
    section: &'static str,
    key: &'static str,
    title: &'static str,
    description: &'static str,
    glyph: Glyph,
) -> CommandSpec {
    CommandSpec {
        host: true,
        ..command(section, key, title, description, glyph)
    }
}

pub(crate) fn command_catalog() -> Vec<CommandSpec> {
    vec![
        command("添加", "file", "文件", "从本机选择文件", Glyph::Link),
        command(
            "添加",
            "attach",
            "附件",
            "上传图片或文档到草稿轨",
            Glyph::Download,
        ),
        host_command(
            "添加",
            "goal",
            "目标",
            "设置或查看长期任务目标",
            Glyph::Check,
        ),
        host_command("添加", "plan", "计划", "进入或退出计划模式", Glyph::Card),
        command(
            "添加",
            "feedback",
            "反馈",
            "发送关于当前会话的反馈",
            Glyph::Comment,
        ),
        host_command("指令", "compact", "压缩", "压缩以上对话内容", Glyph::Redo),
        command(
            "指令",
            "permission",
            "权限",
            "切换权限预设（沙箱模式与审批策略）",
            Glyph::Lock,
        ),
        command("指令", "model", "模型", "选择本会话使用的模型", Glyph::Card),
        command(
            "指令",
            "export",
            "下载日志",
            "将当前会话内容导出为 ZIP",
            Glyph::Download,
        ),
    ]
}

/// 会话级 ACP 命令目录缓存。
#[derive(Default)]
pub(crate) struct CommandCatalog {
    available: BTreeSet<String>,
    leading_input: BTreeSet<String>,
    attachments: BTreeSet<String>,
}

impl CommandCatalog {
    pub(crate) fn clear(&mut self) {
        self.available.clear();
        self.leading_input.clear();
        self.attachments.clear();
    }

    /// 接收 `_dsh/gpui/session/commands` 返回的命令描述符。
    pub(crate) fn replace(&mut self, reply: &Value) {
        self.clear();
        for descriptor in reply.as_array().into_iter().flatten() {
            let Some(name) = descriptor["name"].as_str() else {
                continue;
            };
            self.available.insert(name.to_owned());
            if descriptor["inputHint"].is_string() || descriptor["input"].is_object() {
                self.leading_input.insert(name.to_owned());
            }
            if descriptor["attachments"] == true || descriptor["input"]["attachments"] == true {
                self.attachments.insert(name.to_owned());
            }
        }
    }

    pub(crate) fn catalog(&self) -> Vec<CommandSpec> {
        command_catalog()
            .into_iter()
            .map(|mut spec| {
                if spec.host {
                    spec.enabled = self.available.contains(spec.key);
                }
                spec
            })
            .collect()
    }

    pub(crate) fn supports(&self, name: &str) -> bool {
        self.available.contains(name)
    }

    pub(crate) fn has_leading_input(&self, name: &str) -> bool {
        self.leading_input.contains(name)
    }

    pub(crate) fn accepts_attachments(&self, name: &str) -> bool {
        self.attachments.contains(name)
    }
}

/// Harness 的 claimed command chip 使用本地化 built-in token，
/// 而不是宿主注册的英文命令名（提交时才归一成 canonical name）。
pub(crate) fn localized_command_token(name: &'static str) -> &'static str {
    match name {
        "goal" => "目标",
        "plan" => "计划",
        "compact" => "压缩",
        "feedback" => "反馈",
        "permission" => "权限",
        "export" => "导出",
        _ => name,
    }
}

/// 参数为空时，Harness 在 claimed chip 后显示一段灰色 ghost 提示；
/// 这些文案来自 dsh-client-ui-conversation 的中文 locale，不是 input.hint。
pub(crate) fn command_claim_hint(draft: &str) -> Option<&'static str> {
    let trimmed = draft.trim();
    let token = trimmed.split_whitespace().next()?;
    if trimmed != token {
        return None;
    }
    match token {
        "/目标" | "/goal" => Some("输入目标，智能体将持续执行"),
        "/计划" | "/plan" => Some("描述你的任务以生成计划"),
        _ => None,
    }
}

/// 把中英文输入写法归一成 ACP 注册的 canonical 命令名。
pub(crate) fn canonical_host_command(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let token = &trimmed[1..end];
    let name = match token {
        "goal" | "目标" => "goal",
        "plan" | "计划" => "plan",
        "compact" | "压缩" => "compact",
        _ => return None,
    };
    Some((name.to_owned(), format!("/{name}{}", &trimmed[end..])))
}

/// 按斜杠过滤词筛选可见指令。
/// 官方：对命令名与标题做大小写不敏感的子序列模糊匹配，前缀优先，
/// 其次按匹配得分，最后保留目录原始顺序（即使用频次顺序）。
pub(crate) fn visible_commands<'a>(
    catalog: &'a [CommandSpec],
    filter: &str,
) -> Vec<&'a CommandSpec> {
    let query = filter.trim().to_lowercase();
    if query.is_empty() {
        return catalog.iter().collect();
    }
    let mut matches = catalog
        .iter()
        .enumerate()
        .filter_map(|(index, spec)| {
            let key = fuzzy_name_rank(&query, &spec.key.to_lowercase());
            let title = fuzzy_name_rank(&query, &spec.title.to_lowercase());
            let best = match (key, title) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (Some(a), None) | (None, Some(a)) => Some(a),
                (None, None) => None,
            };
            best.map(|rank| (rank, index, spec))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    matches.into_iter().map(|(_, _, spec)| spec).collect()
}

/// Rank a display name by the Harness prefix and subsequence alignment rules.
#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) fn fuzzy_name_rank(query: &str, label: &str) -> Option<(bool, i64)> {
    if query.is_empty() {
        return Some((true, 0));
    }
    let label = label.to_lowercase();
    let name = label.encode_utf16().collect::<Vec<_>>();
    let boundary = |index: usize| {
        if index == 0 || matches!(name[index - 1], 45 | 95) {
            8
        } else {
            0
        }
    };
    let missing = i64::MIN / 2;
    let mut query_units = query.encode_utf16();
    let first = query_units.next()?;
    let mut scores = name
        .iter()
        .enumerate()
        .map(|(index, unit)| {
            if *unit == first {
                1 + boundary(index) - index as i64
            } else {
                missing
            }
        })
        .collect::<Vec<_>>();
    let mut next = vec![missing; name.len()];
    for unit in query_units {
        let mut adjacent = missing;
        let mut two_back = missing;
        let mut gapped = missing;
        for (index, previous) in scores.iter().copied().enumerate() {
            if two_back != missing {
                gapped = gapped.max(two_back + index as i64 - 2);
            }
            next[index] = if name[index] == unit {
                let bonus = 1 + boundary(index);
                let joined = if adjacent == missing {
                    missing
                } else {
                    adjacent + bonus + 4
                };
                let skipped = if gapped == missing {
                    missing
                } else {
                    gapped + bonus + 1 - index as i64
                };
                joined.max(skipped)
            } else {
                missing
            };
            two_back = adjacent;
            adjacent = previous;
        }
        std::mem::swap(&mut scores, &mut next);
    }
    let score = scores.into_iter().max()?;
    (score != missing).then_some((label.starts_with(query), score))
}

#[cfg(test)]
mod tests {
    use super::{
        CommandCatalog, canonical_host_command, command_claim_hint, fuzzy_name_rank,
        localized_command_token, visible_commands,
    };
    use serde_json::json;

    #[test]
    fn command_filter_matches_key_and_title() {
        let catalog = CommandCatalog::default().catalog();
        assert_eq!(visible_commands(&catalog, "").len(), catalog.len());
        let keys = visible_commands(&catalog, "pe")
            .into_iter()
            .map(|spec| spec.key)
            .collect::<Vec<_>>();
        assert!(keys.contains(&"permission"));
        let titles = visible_commands(&catalog, "文件")
            .into_iter()
            .map(|spec| spec.key)
            .collect::<Vec<_>>();
        assert_eq!(titles, vec!["file"]);
        assert!(visible_commands(&catalog, "不存在").is_empty());
    }

    #[test]
    fn command_filter_ranks_prefixes_before_subsequence_matches() {
        let catalog = CommandCatalog::default().catalog();
        // 官方：前缀优先于仅靠子序列命中的行。
        let keys = visible_commands(&catalog, "co")
            .into_iter()
            .map(|spec| spec.key)
            .collect::<Vec<_>>();
        assert_eq!(keys.first().copied(), Some("compact"));
        // 非连续子序列同样命中指令名。
        let keys = visible_commands(&catalog, "cm")
            .into_iter()
            .map(|spec| spec.key)
            .collect::<Vec<_>>();
        assert!(keys.contains(&"compact"));
    }

    #[test]
    fn runtime_catalog_controls_host_commands() {
        let mut runtime = CommandCatalog::default();
        assert!(
            runtime
                .catalog()
                .iter()
                .any(|spec| spec.key == "goal" && !spec.enabled)
        );
        runtime.replace(&json!([
            {"name": "goal", "inputHint": "<objective>", "attachments": true},
            {"name": "plan", "inputHint": "[off|message]", "attachments": true},
            {"name": "compact"},
        ]));
        let catalog = runtime.catalog();
        assert!(
            catalog
                .iter()
                .filter(|spec| spec.host)
                .all(|spec| spec.enabled)
        );
        assert!(runtime.has_leading_input("goal"));
        assert!(runtime.accepts_attachments("plan"));
        assert!(!runtime.has_leading_input("compact"));
        assert!(!runtime.accepts_attachments("compact"));
        runtime.clear();
        assert!(!runtime.supports("goal"));
    }

    #[test]
    fn claimed_commands_use_localized_token_and_ghost_hint() {
        assert_eq!(localized_command_token("goal"), "目标");
        assert_eq!(localized_command_token("plan"), "计划");
        assert_eq!(localized_command_token("compact"), "压缩");
        assert_eq!(
            command_claim_hint("/目标 "),
            Some("输入目标，智能体将持续执行")
        );
        assert_eq!(command_claim_hint("/计划 "), Some("描述你的任务以生成计划"));
        assert_eq!(command_claim_hint("/目标 完成发布"), None);
        assert_eq!(command_claim_hint("普通消息"), None);
    }

    #[test]
    fn canonical_host_command_translates_localized_tokens() {
        assert_eq!(
            canonical_host_command("/目标 完成发布"),
            Some(("goal".to_owned(), "/goal 完成发布".to_owned()))
        );
        assert_eq!(
            canonical_host_command("/计划 off"),
            Some(("plan".to_owned(), "/plan off".to_owned()))
        );
        assert_eq!(
            canonical_host_command("/压缩"),
            Some(("compact".to_owned(), "/compact".to_owned()))
        );
        assert_eq!(canonical_host_command("普通消息"), None);
        assert_eq!(canonical_host_command("/unknown"), None);
    }

    #[test]
    fn model_search_ranks_prefixes_and_ordered_matches() {
        assert!(fuzzy_name_rank("", "DeepSeek-V3.2").is_some());
        assert!(fuzzy_name_rank("dsv3", "DeepSeek-V3.2").is_some());
        assert!(fuzzy_name_rank("deep", "DeepSeek Chat").is_some());
        assert!(
            fuzzy_name_rank("deep", "DeepSeek Chat") > fuzzy_name_rank("deep", "A DeepSeek Chat")
        );
        assert!(fuzzy_name_rank("v3deep", "DeepSeek-V3.2").is_none());
        assert!(fuzzy_name_rank("xyz", "DeepSeek-V3.2").is_none());
    }
}
