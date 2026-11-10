//! “/ 调用指令”面板目录，WASM UI 与测试共享。

use kui::primitives::glyph::Glyph;

/// 指令面板条目。enabled=false 表示 ACP 尚未支持该指令。
#[derive(Clone, Copy)]
pub(crate) struct CommandSpec {
    pub(crate) section: &'static str,
    pub(crate) key: &'static str,
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
    pub(crate) glyph: Glyph,
    pub(crate) enabled: bool,
}

pub(crate) fn command_catalog() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            section: "添加",
            key: "file",
            title: "文件",
            description: "引用工作区文件",
            glyph: Glyph::Link,
            enabled: true,
        },
        CommandSpec {
            section: "添加",
            key: "attach",
            title: "附件",
            description: "上传图片或文档到草稿轨",
            glyph: Glyph::Download,
            enabled: true,
        },
        CommandSpec {
            section: "添加",
            key: "goal",
            title: "目标",
            description: "设置或查看长期任务目标",
            glyph: Glyph::Check,
            enabled: false,
        },
        CommandSpec {
            section: "添加",
            key: "plan",
            title: "计划",
            description: "进入或退出计划模式",
            glyph: Glyph::Card,
            enabled: false,
        },
        CommandSpec {
            section: "添加",
            key: "feedback",
            title: "反馈",
            description: "发送关于当前会话的反馈",
            glyph: Glyph::Comment,
            enabled: true,
        },
        CommandSpec {
            section: "指令",
            key: "compact",
            title: "压缩",
            description: "压缩以上对话内容",
            glyph: Glyph::Redo,
            enabled: false,
        },
        CommandSpec {
            section: "指令",
            key: "permission",
            title: "权限",
            description: "切换权限预设（沙箱模式与审批策略）",
            glyph: Glyph::Lock,
            enabled: true,
        },
        CommandSpec {
            section: "指令",
            key: "model",
            title: "模型",
            description: "选择本会话使用的模型",
            glyph: Glyph::Card,
            enabled: true,
        },
        CommandSpec {
            section: "指令",
            key: "export",
            title: "下载日志",
            description: "将当前会话内容导出为 ZIP",
            glyph: Glyph::Download,
            enabled: true,
        },
    ]
}

/// 按斜杠过滤词筛选可见指令（匹配指令名或拉丁键名，大小写不敏感）。
pub(crate) fn visible_commands<'a>(
    catalog: &'a [CommandSpec],
    filter: &str,
) -> Vec<&'a CommandSpec> {
    let query = filter.trim().to_lowercase();
    catalog
        .iter()
        .filter(|spec| {
            query.is_empty()
                || spec.key.to_lowercase().contains(&query)
                || spec.title.to_lowercase().contains(&query)
        })
        .collect()
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
    use super::{command_catalog, fuzzy_name_rank, visible_commands};

    #[test]
    fn command_filter_matches_key_and_title() {
        let catalog = command_catalog();
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
