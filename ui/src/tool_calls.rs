//! Durable tool identities and the parent edges carried by PTC dispatches.

use crate::state::{ChatEntry, EntryRole};
use serde_json::Value;
#[cfg(any(target_family = "wasm", test))]
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolCall {
    pub(crate) name: String,
    pub(crate) parent: Option<ParentCall>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParentCall {
    id: String,
    root: String,
}

impl ToolCall {
    pub(crate) fn root(name: String) -> Self {
        Self { name, parent: None }
    }
}

impl ChatEntry {
    #[cfg(any(target_family = "wasm", test))]
    pub(crate) fn tool_name(&self) -> &str {
        self.tool.as_ref().map_or(&self.title, |call| &call.name)
    }

    pub(crate) fn is_subcall(&self) -> bool {
        self.tool.as_ref().is_some_and(|call| call.parent.is_some())
    }

    pub(crate) fn is_running_tool(&self) -> bool {
        self.role == EntryRole::Tool
            && matches!(
                self.status.as_str(),
                "in_progress" | "pending" | "preparing" | "approval"
            )
    }
}

pub(crate) fn result_status(data: &Value, result: &Value) -> &'static str {
    match data.pointer("/error/code").and_then(Value::as_str) {
        Some("ASK_CANCELLED") => "ASK_CANCELLED",
        Some("ASK_ABORTED") => "ASK_ABORTED",
        _ if result["isError"] == true => "failed",
        _ => "completed",
    }
}

pub(crate) fn result_text(message: &Value) -> String {
    message
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| match block.get("type").and_then(Value::as_str) {
                    Some("text") => block.get("text").and_then(Value::as_str),
                    _ => block
                        .get("content")
                        .and_then(Value::as_str)
                        .or_else(|| block.get("text").and_then(Value::as_str)),
                })
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

pub(crate) fn apply_dispatch(entries: &mut Vec<ChatEntry>, event: &Value) {
    let data = &event["data"];
    let (Some(id), Some(parent), Some(root), Some(name)) = (
        data["subCallId"].as_str(),
        data["parentCallId"].as_str(),
        data["rootCallId"].as_str(),
        data["name"].as_str(),
    ) else {
        return;
    };
    if id.is_empty()
        || parent.is_empty()
        || root.is_empty()
        || name.is_empty()
        || id == root
        || id == parent
    {
        return;
    }
    let key = format!("ptc-{id}");
    let mut entry = ChatEntry::local(EntryRole::Tool, &key, "");
    entry.external_id = Some(id.to_owned());
    entry.title = name.to_owned();
    entry.tool = Some(ToolCall {
        name: name.to_owned(),
        parent: Some(ParentCall {
            id: parent.to_owned(),
            root: root.to_owned(),
        }),
    });
    entry.at_ms = event["time"].as_f64();
    entry.input = data["arguments"].to_string();
    let ended = event["type"] == "tool/ptc-dispatch";
    if ended {
        entry.status = result_status(data, data).to_owned();
        entry.text = result_text(data);
        if entry.text.is_empty()
            && let Some(reason) = data.pointer("/error/reason").and_then(Value::as_str)
        {
            entry.text = reason.to_owned();
        }
    } else {
        entry.status = "in_progress".to_owned();
    }

    if let Some(previous) = entries.iter_mut().find(|row| row.key == key) {
        if previous.tool != entry.tool {
            return;
        }
        if ended || previous.is_running_tool() {
            entry.at_ms = previous.at_ms.or(entry.at_ms);
            *previous = entry;
        } else {
            previous.at_ms = entry.at_ms.or(previous.at_ms);
        }
    } else {
        entries.push(entry);
    }
}

/// Merge a newer observation into the first position of the same call. The
/// paged log may hold the call's start on one page and its result on another.
#[cfg(any(target_family = "wasm", test))]
pub(crate) fn merge_tool_observation(original: &mut ChatEntry, recent: ChatEntry) {
    if original.tool.is_some()
        && recent.tool.is_some()
        && original.tool != recent.tool
        && (original.is_subcall() || recent.is_subcall())
    {
        return;
    }
    if original.tool.is_none() && recent.tool.is_some() {
        original.tool = recent.tool.clone();
        original.title = recent.title.clone();
        original.input = recent.input.clone();
    }
    original.at_ms = original.at_ms.or(recent.at_ms);
    if !recent.is_running_tool() || original.is_running_tool() {
        original.status = recent.status;
        original.text = recent.text;
    }
}

/// Only attach edges that lead to the declared, loaded root. A later history
/// prepend can supply a missing ancestor without moving the original events.
#[cfg(any(target_family = "wasm", test))]
pub(crate) struct ToolTree<'a> {
    children: HashMap<&'a str, Vec<&'a ChatEntry>>,
    visible: HashSet<&'a str>,
}

#[cfg(any(target_family = "wasm", test))]
impl<'a> ToolTree<'a> {
    pub(crate) fn new(entries: &'a [ChatEntry]) -> Self {
        let mut roots = HashSet::new();
        let mut subcalls = HashMap::new();
        for entry in entries.iter().filter(|entry| entry.role == EntryRole::Tool) {
            let Some(id) = entry.external_id.as_deref() else {
                continue;
            };
            if let Some(parent) = entry.tool.as_ref().and_then(|call| call.parent.as_ref()) {
                subcalls.entry(id).or_insert(parent);
            } else {
                roots.insert(id);
            }
        }

        let mut tree = Self {
            children: HashMap::new(),
            visible: roots,
        };
        for entry in entries.iter().filter(|entry| entry.is_subcall()) {
            let Some(id) = entry.external_id.as_deref() else {
                continue;
            };
            let Some(parent) = subcalls.get(id) else {
                continue;
            };
            if roots_include_path(&tree.visible, &subcalls, id, parent) && tree.visible.insert(id) {
                tree.children.entry(&parent.id).or_default().push(entry);
            }
        }
        tree
    }

    pub(crate) fn contains(&self, id: &str) -> bool {
        self.visible.contains(id)
    }

    pub(crate) fn children(&self, id: Option<&str>) -> &[&'a ChatEntry] {
        id.and_then(|id| self.children.get(id))
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn append_branch(&self, root: &'a ChatEntry, rows: &mut Vec<&'a ChatEntry>) {
        rows.push(root);
        for child in self.children(root.external_id.as_deref()) {
            self.append_branch(child, rows);
        }
    }
}

#[cfg(any(target_family = "wasm", test))]
fn roots_include_path(
    visible: &HashSet<&str>,
    subcalls: &HashMap<&str, &ParentCall>,
    id: &str,
    parent: &ParentCall,
) -> bool {
    let root = parent.root.as_str();
    if !visible.contains(root) || subcalls.contains_key(root) {
        return false;
    }
    let mut next = parent.id.as_str();
    // The official renderer limits root + descendants to 256 levels.
    for _ in 1..256 {
        if next == root {
            return true;
        }
        if next == id {
            return false;
        }
        let Some(ancestor) = subcalls.get(next).filter(|ancestor| ancestor.root == root) else {
            return false;
        };
        next = &ancestor.id;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dispatch(kind: &str, parent: &str, id: &str, name: &str, time: i32) -> Value {
        json!({
            "type": kind, "time": time,
            "data": {
                "subCallId": id, "parentCallId": parent, "rootCallId": "root",
                "name": name, "arguments": {"path": "src/lib.rs"},
                "content": [{"type": "text", "text": id}]
            }
        })
    }

    #[test]
    fn live_and_replayed_dispatches_preserve_the_nested_tree_and_results() {
        let root = json!({"type":"tool/call","time":0,"data":{"callId":"root","name":"run_code",
            "arguments":"{\"description\":\"Review source\"}"}});
        let parent = dispatch("tool/ptc-dispatch-start", "root", "parent", "run_code", 1);
        let child = dispatch("tool/ptc-dispatch-start", "parent", "child", "read", 2);
        let sibling = dispatch("tool/ptc-dispatch-start", "root", "sibling", "write", 3);
        let mut failed = dispatch("tool/ptc-dispatch", "parent", "child", "read", 4);
        failed["data"]["isError"] = json!(true);
        let events = [root, parent, child, sibling, failed];
        let stored = crate::history::project_session_events(&events);
        let mut live = Vec::new();
        crate::state::apply_update(
            &mut live,
            &json!({
                "sessionUpdate":"tool_call","toolCallId":"root","title":"run_code","status":"in_progress"
            }),
        );
        crate::state::apply_update(
            &mut live,
            &json!({
                "sessionUpdate":"tool_call","toolCallId":"root","title":"run_code","status":"in_progress","time":0
            }),
        );
        for event in &events[1..] {
            crate::state::apply_update(
                &mut live,
                &json!({"sessionUpdate":"gpui_ptc_dispatch","event":event}),
            );
        }
        for entries in [&stored.entries, &live] {
            let tree = ToolTree::new(entries);
            let mut branch = Vec::new();
            tree.append_branch(&entries[0], &mut branch);
            let calls: Vec<_> = branch
                .iter()
                .map(|entry| entry.external_id.as_deref().unwrap())
                .collect();
            assert_eq!(calls, ["root", "parent", "child", "sibling"]);
            assert_eq!(branch[0].tool_name(), "run_code");
            assert_eq!(branch[0].at_ms, Some(0.));
            assert_eq!(branch[2].at_ms, Some(2.));
            assert_eq!(branch[2].status, "failed");
            assert_eq!(branch[2].text, "child");
            assert!(branch[3].is_running_tool());
        }
    }

    #[test]
    fn attaching_an_earlier_page_retains_the_result_and_ignores_invalid_edges() {
        let start = dispatch("tool/ptc-dispatch-start", "root", "child", "read", 2);
        let result = dispatch("tool/ptc-dispatch", "root", "child", "read", 4);
        let root = json!({"type":"tool/call","data":{"callId":"root","name":"run_code"}});
        let cycle_a = dispatch("tool/ptc-dispatch-start", "b", "a", "read", 5);
        let cycle_b = dispatch("tool/ptc-dispatch-start", "a", "b", "read", 6);
        let mut recent =
            crate::history::project_session_events(&[result, cycle_a, cycle_b]).entries;
        assert!(!ToolTree::new(&recent).contains("child"));
        let mut earlier = crate::history::project_session_events(&[root, start]).entries;
        merge_tool_observation(&mut earlier[1], recent.remove(0));
        earlier.append(&mut recent);
        let tree = ToolTree::new(&earlier);
        let child = tree.children(Some("root"))[0];
        assert_eq!(child.at_ms, Some(2.));
        assert_eq!(child.status, "completed");
        assert_eq!(child.text, "child");
        assert!(tree.contains("child"));
        assert!(!tree.contains("a"));
        assert!(!tree.contains("b"));
    }
}
