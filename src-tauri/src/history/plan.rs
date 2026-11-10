//! Recover a submitted plan from its immutable Session tool invocation.

use super::{
    dsh_home,
    export::{find_session_log, read_session_log},
    submitted_plan::{PlanDocument, SubmittedPlan},
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

#[tauri::command]
pub(crate) async fn read_session_plans(
    webview: tauri::Webview,
    session_id: String,
    cwd: String,
    call_ids: Vec<String>,
) -> Result<HashMap<String, PlanDocument>, String> {
    if webview.label() != "main"
        || session_id.is_empty()
        || call_ids.is_empty()
        || call_ids.iter().any(String::is_empty)
    {
        return Err("计划地址无效".into());
    }
    tokio::task::spawn_blocking(move || {
        read_plans(&dsh_home().join("sessions"), &session_id, &cwd, &call_ids)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn read_plans(
    sessions: &Path,
    session_id: &str,
    cwd: &str,
    call_ids: &[String],
) -> Result<HashMap<String, PlanDocument>, String> {
    let Some(path) = find_session_log(sessions, session_id, cwd)? else {
        return Ok(HashMap::new());
    };
    let log = read_session_log(&path, session_id)?;
    let mut missing: HashSet<&str> = call_ids.iter().map(String::as_str).collect();
    let mut found = HashMap::new();
    for event in &log.events {
        if let Some(plan) = SubmittedPlan::from_event(event)
            && missing.remove(plan.call_id.as_str())
        {
            found.insert(plan.call_id, plan.document);
            if missing.is_empty() {
                break;
            }
        }
    }
    Ok(found)
}

/// Annotate the view's turn/end before pagination so a plan whose invocation
/// fell on an older page still appears at the end of its own turn.
pub(super) fn index_turn_plans(events: &mut [Value]) {
    let mut plans = indexmap::IndexMap::new();
    for event in events {
        match event["type"].as_str() {
            Some("turn/start") => plans.clear(),
            Some("turn/end") if !plans.is_empty() => {
                let ordered: Vec<SubmittedPlan> =
                    std::mem::take(&mut plans).into_values().collect();
                event["gpuiSubmittedPlans"] = serde_json::json!(ordered);
            }
            _ => {
                if let Some(plan) = SubmittedPlan::from_event(event) {
                    plans.entry(plan.call_id.clone()).or_insert(plan);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn history_page_keeps_plan_cards_at_the_ended_turn_in_invocation_order() {
        let events = [
            json!({"seq":0,"type":"turn/start","data":{"turn":1}}),
            json!({"seq":1,"type":"tool/call","data":{
                "callId":"native","name":"exit_plan_mode","arguments":"{\"plan\":\"# Native\"}"
            }}),
            json!({"seq":2,"type":"tool/ptc-dispatch-start","data":{
                "subCallId":"ptc:1","name":"exit_plan_mode","arguments":{"plan":"# Nested"}
            }}),
            json!({"seq":3,"type":"tool/ptc-dispatch","data":{
                "subCallId":"ptc:1","name":"exit_plan_mode","arguments":{"plan":"# Nested"}
            }}),
            json!({"seq":4,"type":"turn/end","data":{"turn":1}}),
            json!({"seq":5,"type":"turn/start","data":{"turn":2}}),
            json!({"seq":6,"type":"turn/end","data":{"turn":2}}),
        ];
        let text = events
            .iter()
            .map(|event| format!("{event}\n"))
            .collect::<String>();
        let (page, more) = super::super::parse_events(text.as_bytes(), Some(5), 1).unwrap();
        assert!(more);
        assert_eq!(page[0]["seq"], 4);
        assert_eq!(
            page[0]["gpuiSubmittedPlans"],
            json!([
                {"callId":"native","title":"Native","markdown":"# Native"},
                {"callId":"ptc:1","title":"Nested","markdown":"# Nested"},
            ])
        );
        let (next, _) = super::super::parse_events(text.as_bytes(), None, 1).unwrap();
        assert!(next[0]["gpuiSubmittedPlans"].is_null());
    }

    #[test]
    fn recovers_exact_invocations_from_a_committed_log_beyond_the_history_window() {
        let sessions = tempfile::tempdir().unwrap();
        let directory = sessions.path().join("--work--").join("session-1");
        std::fs::create_dir_all(&directory).unwrap();
        let mut lines = vec![
            json!({"type":"session","version":4,"id":"session-1","isSeeded":false}),
            json!({"seq":0,"type":"tool/call","data":{
                "callId":"native", "name":"exit_plan_mode",
                "arguments":json!({"plan":"# Native\n\n**Full text.**"}).to_string()
            }}),
            json!({"seq":1,"type":"tool/ptc-dispatch-start","data":{
                "rootCallId":"program", "subCallId":"program:ptc:1","name":"exit_plan_mode",
                "arguments":{"plan":"# Nested\n\n- Keep this line"}
            }}),
            json!({"seq":2,"type":"tool/ptc-dispatch","data":{
                "rootCallId":"program", "subCallId":"program:ptc:1","name":"exit_plan_mode",
                "arguments":{"plan":"# Nested\n\n- Keep this line"}, "isError":false
            }}),
        ];
        lines.extend((3..2505).map(|seq| json!({"seq":seq,"type":"turn/end","data":{"turn":seq}})));
        let text = lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>();
        let header_end = text.find('\n').unwrap() + 1;
        let mut compressed = zstd::stream::encode_all(&text.as_bytes()[..header_end], 0).unwrap();
        compressed.extend(zstd::stream::encode_all(&text.as_bytes()[header_end..], 0).unwrap());
        std::fs::write(directory.join("session.v4.jsonl.zstd"), compressed).unwrap();
        let requested = [
            "native".to_owned(),
            "program:ptc:1".to_owned(),
            "program".to_owned(),
        ];
        let found = read_plans(sessions.path(), "session-1", "/work", &requested).unwrap();
        for (id, title, markdown) in [
            ("native", "Native", "# Native\n\n**Full text.**"),
            ("program:ptc:1", "Nested", "# Nested\n\n- Keep this line"),
        ] {
            assert_eq!(
                found.get(id).unwrap(),
                &PlanDocument {
                    title: title.into(),
                    markdown: markdown.into()
                },
            );
        }
        assert!(!found.contains_key("program"));
        assert!(
            read_plans(sessions.path(), "another-session", "/work", &requested)
                .unwrap()
                .is_empty()
        );
    }
}
