//! Direct-child subagent catalog of one session.
//!
//! A parent session records every spawned child as a durable `subagent/catalog`
//! fact (`{version, childId, childCreatedAt, mode, label, name?}`); the official
//! client projects them into the session-header lineage so a parent run can be
//! inspected. Activity is live-only: it comes from the agent status feed, not
//! from the durable catalog.
//!
//! Parsing and merge rules are pure so they stay host-testable.

use serde_json::Value;

/// How one child was created (`SubagentDescriptor.mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentMode {
    /// Can receive follow-up messages.
    Continuable,
    /// One-shot run; its transcript is read-only forever.
    OneShot,
    /// Ran through an external provider without a local child session.
    External,
}

impl SubagentMode {
    /// Official `mode.*` label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Continuable => "可继续",
            Self::OneShot => "一次性",
            Self::External => "外部任务",
        }
    }

    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("continuable") => Self::Continuable,
            Some("external") => Self::External,
            _ => Self::OneShot,
        }
    }
}

/// One durable catalog row: a direct child discovered by this session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentRow {
    pub child_id: String,
    pub label: String,
    pub mode: SubagentMode,
    pub created_at_ms: Option<u64>,
}

impl SubagentRow {
    /// Read one `subagent/catalog` payload.
    pub fn from_event_data(data: &Value) -> Option<Self> {
        let child_id = data["childId"].as_str().filter(|id| !id.is_empty())?;
        let label = data["label"]
            .as_str()
            .filter(|label| !label.trim().is_empty())
            .unwrap_or("子智能体");
        Some(Self {
            child_id: child_id.to_owned(),
            label: label.to_owned(),
            mode: SubagentMode::parse(data["mode"].as_str()),
            created_at_ms: data["childCreatedAt"].as_u64(),
        })
    }

    /// Whether this child has a locally readable transcript.
    pub fn has_local_session(&self) -> bool {
        self.mode != SubagentMode::External
    }
}

/// Insert or replace one catalog row, keeping discovery order.
///
/// The catalog is append-only per child, but reopening a session can merge an
/// older page with newer rows; the last observation of an identity wins.
pub fn upsert_subagent(rows: &mut Vec<SubagentRow>, row: SubagentRow) {
    if let Some(existing) = rows.iter_mut().find(|item| item.child_id == row.child_id) {
        *existing = row;
    } else {
        rows.push(row);
    }
}

/// Live activity label for one catalog row.
pub fn activity_label(row: &SubagentRow, running: bool) -> &'static str {
    if !row.has_local_session() {
        "外部任务"
    } else if running {
        "正在运行"
    } else {
        "已完成"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_catalog_fact_keeps_identity_label_and_mode() {
        let row = SubagentRow::from_event_data(&json!({
            "version": 0,
            "childId": "3ee01e83-40f7-4abf-b1e4-03c4bc0d85ef",
            "childCreatedAt": 1_794_454_553_990u64,
            "mode": "continuable",
            "label": "研究 goal 官方 UI 呈现",
        }))
        .expect("catalog row");
        assert_eq!(row.child_id, "3ee01e83-40f7-4abf-b1e4-03c4bc0d85ef");
        assert_eq!(row.label, "研究 goal 官方 UI 呈现");
        assert_eq!(row.mode, SubagentMode::Continuable);
        assert_eq!(row.created_at_ms, Some(1_794_454_553_990));
        assert_eq!(row.mode.label(), "可继续");
    }

    #[test]
    fn missing_labels_and_unknown_modes_fall_back() {
        let row = SubagentRow::from_event_data(&json!({
            "childId": "child-1", "mode": "mystery", "label": "   ",
        }))
        .expect("catalog row");
        assert_eq!(row.label, "子智能体");
        assert_eq!(row.mode, SubagentMode::OneShot);
        assert!(SubagentRow::from_event_data(&json!({"label": "x"})).is_none());
        assert!(SubagentRow::from_event_data(&json!(null)).is_none());
    }

    #[test]
    fn the_same_child_is_never_listed_twice() {
        let mut rows = Vec::new();
        upsert_subagent(
            &mut rows,
            SubagentRow::from_event_data(&json!({
                "childId": "a", "label": "第一版", "mode": "continuable",
            }))
            .unwrap(),
        );
        upsert_subagent(
            &mut rows,
            SubagentRow::from_event_data(&json!({
                "childId": "b", "label": "另一个", "mode": "one-shot",
            }))
            .unwrap(),
        );
        upsert_subagent(
            &mut rows,
            SubagentRow::from_event_data(&json!({
                "childId": "a", "label": "改名", "mode": "continuable",
            }))
            .unwrap(),
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "改名");
        assert_eq!(rows[1].child_id, "b");
    }

    #[test]
    fn activity_distinguishes_running_finished_and_external() {
        let local = SubagentRow::from_event_data(&json!({
            "childId": "a", "label": "本地", "mode": "continuable",
        }))
        .unwrap();
        assert_eq!(activity_label(&local, true), "正在运行");
        assert_eq!(activity_label(&local, false), "已完成");
        let external = SubagentRow::from_event_data(&json!({
            "childId": "b", "label": "外部", "mode": "external",
        }))
        .unwrap();
        assert_eq!(activity_label(&external, true), "外部任务");
        assert!(!external.has_local_session());
    }
}
