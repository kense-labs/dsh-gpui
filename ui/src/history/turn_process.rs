//! Durable turn coordinates for the transcript's whole-turn disclosure.

use crate::state::{ChatEntry, EntryRole};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Copy)]
struct Anchor {
    turn: u64,
    seq: u64,
    time: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndKind {
    Completed,
    Stopped,
    Failed,
}

impl EndKind {
    fn from_reason(reason: &str) -> Self {
        match reason {
            "aborted" | "interrupted" | "cancelled" | "user" => Self::Stopped,
            "error" => Self::Failed,
            _ => Self::Completed,
        }
    }
}

struct End {
    seq: u64,
    time: Option<u64>,
    kind: EndKind,
}

struct Assistant {
    step: u64,
    seq: u64,
    is_answer: bool,
}

#[derive(Default)]
struct Turn {
    start_time: Option<u64>,
    end: Option<End>,
    first_evidence: Option<u64>,
    last_step: u64,
    assistant: Option<Assistant>,
}

impl Turn {
    fn answer(&self) -> Option<&Assistant> {
        self.assistant
            .as_ref()
            .filter(|answer| answer.step == self.last_step && answer.is_answer)
    }
}

#[derive(Default)]
pub(crate) struct TurnProcess {
    turns: BTreeMap<u64, Turn>,
    assistants: HashMap<String, Anchor>,
    tools: HashMap<String, Anchor>,
    retries: HashMap<String, Anchor>,
    inputs: BTreeSet<u64>,
}

fn has_reply(content: &[Value]) -> bool {
    content.iter().any(|block| match block["type"].as_str() {
        Some("tool-call" | "reasoning") | None => false,
        Some("text") => block["text"]
            .as_str()
            .is_some_and(|text| !text.trim().is_empty()),
        _ => true,
    })
}

pub(crate) fn retry_id(data: &Value, seq: Option<u64>) -> Option<String> {
    data["retryId"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| seq.map(|seq| format!("retry-{seq}")))
}

impl TurnProcess {
    pub(crate) fn observe_history(&mut self, event: &Value) {
        let Some(kind) = event["type"].as_str() else {
            return;
        };
        let Some(seq) = event["seq"].as_u64() else {
            return;
        };
        let data = &event["data"];
        if kind == "user/message" && visible_input(data) {
            self.inputs.insert(seq);
        }
        if matches!(kind, "assistant/message" | "tool/result") && event["surfaceOp"] != "append" {
            return;
        }
        let Some(turn) = data["turn"].as_u64() else {
            return;
        };
        let step = data["step"].as_u64();
        let time = event["time"].as_u64();
        let anchor = Anchor { turn, seq, time };
        match kind {
            "turn/start" | "step/start" | "turn/end" => {
                self.observe_boundary(kind, anchor, step, time, data["reason"]["kind"].as_str())
            }
            "assistant/message" => {
                let message = &data["message"];
                let content = message["content"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let reply = has_reply(content);
                let reasoning = content.iter().any(|block| {
                    block["type"] == "reasoning"
                        && block["text"]
                            .as_str()
                            .is_some_and(|text| !text.trim().is_empty())
                });
                let calls = content.iter().any(|block| block["type"] == "tool-call");
                self.observe_assistant(
                    anchor,
                    step,
                    message["id"].as_str(),
                    reply || reasoning,
                    reply && !calls,
                );
            }
            "tool/call" => self.observe_tool(anchor, step, data["callId"].as_str()),
            "tool/result" => self.observe_tool(
                anchor,
                step,
                data["message"]
                    .pointer("/source/callId")
                    .or_else(|| data["message"].get("toolCallId"))
                    .and_then(Value::as_str),
            ),
            "llm/retry" => self.observe_retry(anchor, step, retry_id(data, Some(seq))),
            _ => {}
        }
    }

    pub(crate) fn observe_live(&mut self, update: &Value) {
        let Some(kind) = update["type"].as_str() else {
            return;
        };
        let Some(seq) = update["seq"].as_u64() else {
            return;
        };
        if kind == "user/message" {
            self.inputs.insert(seq);
        }
        let Some(turn) = update["turn"].as_u64() else {
            return;
        };
        let step = update["step"].as_u64();
        let time = update["time"]
            .as_u64()
            .or_else(|| update["toolCall"]["time"].as_u64());
        let anchor = Anchor { turn, seq, time };
        match kind {
            "turn/start" | "step/start" | "turn/end" => self.observe_boundary(
                kind,
                anchor,
                step,
                update["time"].as_u64(),
                update["reason"].as_str(),
            ),
            "assistant/message" => self.observe_assistant(
                anchor,
                step,
                update["assistantId"].as_str(),
                update["hasVisibleContent"] == true,
                update["hasReply"] == true && update["hasToolCalls"] != true,
            ),
            "tool/call" | "tool/result" => self.observe_tool(
                anchor,
                step,
                update["callId"]
                    .as_str()
                    .or_else(|| update["toolCall"]["toolCallId"].as_str()),
            ),
            "llm/retry" => self.observe_retry(anchor, step, retry_id(update, Some(seq))),
            _ => {}
        }
    }

    fn observe_boundary(
        &mut self,
        kind: &str,
        anchor: Anchor,
        step: Option<u64>,
        time: Option<u64>,
        reason: Option<&str>,
    ) {
        let turn = self.turns.entry(anchor.turn).or_default();
        match kind {
            "turn/start" => turn.start_time = time.or(turn.start_time),
            "step/start" => turn.last_step = turn.last_step.max(step.unwrap_or_default()),
            "turn/end" if turn.end.as_ref().is_none_or(|end| end.seq <= anchor.seq) => {
                turn.end = Some(End {
                    seq: anchor.seq,
                    time,
                    kind: EndKind::from_reason(reason.unwrap_or_default()),
                });
            }
            _ => {}
        }
    }

    fn observe_assistant(
        &mut self,
        anchor: Anchor,
        step: Option<u64>,
        id: Option<&str>,
        visible: bool,
        is_answer: bool,
    ) {
        if let Some(id) = id {
            record_anchor(&mut self.assistants, id, anchor);
        }
        self.observe_evidence(anchor, step, visible);
        let step = step.unwrap_or_default();
        let turn = self.turns.entry(anchor.turn).or_default();
        if turn
            .assistant
            .as_ref()
            .is_none_or(|previous| (step, anchor.seq) >= (previous.step, previous.seq))
        {
            turn.assistant = Some(Assistant {
                step,
                seq: anchor.seq,
                is_answer,
            });
        }
    }

    fn observe_tool(&mut self, anchor: Anchor, step: Option<u64>, id: Option<&str>) {
        if let Some(id) = id {
            record_anchor(&mut self.tools, id, anchor);
        }
        self.observe_evidence(anchor, step, true);
    }

    fn observe_retry(&mut self, anchor: Anchor, step: Option<u64>, id: Option<String>) {
        if let Some(id) = id {
            record_anchor(&mut self.retries, &id, anchor);
        }
        self.observe_evidence(anchor, step, true);
    }

    fn observe_evidence(&mut self, anchor: Anchor, step: Option<u64>, visible: bool) {
        let turn = self.turns.entry(anchor.turn).or_default();
        turn.last_step = turn.last_step.max(step.unwrap_or_default());
        if visible {
            turn.first_evidence = Some(
                turn.first_evidence
                    .map_or(anchor.seq, |seq| seq.min(anchor.seq)),
            );
        }
    }

    pub(crate) fn end_kind(&self, turn: u64) -> Option<EndKind> {
        Some(self.turns.get(&turn)?.end.as_ref()?.kind)
    }

    pub(crate) fn tool_time(&self, id: &str) -> Option<u64> {
        self.tools.get(id)?.time
    }

    fn anchor(&self, entry: &ChatEntry) -> Option<Anchor> {
        let id = entry.external_id.as_deref()?;
        match entry.role {
            EntryRole::Assistant | EntryRole::Thought => self.assistants.get(id).copied(),
            EntryRole::Tool => self.tools.get(id).copied(),
            EntryRole::Retry => self.retries.get(id).copied(),
            EntryRole::System if entry.title == "turn-end" => {
                let turn = id.parse().ok()?;
                let end = self.turns.get(&turn)?.end.as_ref()?;
                Some(Anchor {
                    turn,
                    seq: end.seq,
                    time: end.time,
                })
            }
            _ => None,
        }
    }

    pub(crate) fn layout(
        &self,
        entries: &[&ChatEntry],
        choices: &HashMap<u64, (u64, bool)>,
        options: Options,
    ) -> Layout {
        let mut rows = Vec::with_capacity(entries.len());
        let mut spans: HashMap<u64, Span> = HashMap::new();
        let last_input = entries
            .iter()
            .rposition(|entry| entry.role == EntryRole::User);
        for (index, entry) in entries.iter().enumerate() {
            let anchor = self.anchor(entry);
            rows.push(Row {
                turn: anchor.map(|anchor| anchor.turn),
                ..Row::default()
            });
            let Some(anchor) = anchor else { continue };
            let turn = &self.turns[&anchor.turn];
            let answer = turn.answer();
            let member = match entry.role {
                EntryRole::Assistant => {
                    !options.keep_replies && answer.is_none_or(|answer| anchor.seq < answer.seq)
                }
                EntryRole::Thought => answer.is_none_or(|answer| anchor.seq <= answer.seq),
                EntryRole::System => false,
                _ => answer.is_none_or(|answer| anchor.seq < answer.seq),
            };
            rows[index].member = member;
            rows[index].duplicate_stop = entry.role == EntryRole::System
                && turn
                    .end
                    .as_ref()
                    .is_some_and(|end| end.kind == EndKind::Stopped);
            let span = spans.entry(anchor.turn).or_insert(Span {
                first: index,
                last: index,
                has_content: false,
            });
            span.last = index;
            span.has_content |= member;
        }

        let latest = self.turns.last_key_value().map(|(number, _)| *number);
        let mut controls = HashMap::new();
        let mut hidden_turns = BTreeSet::new();
        let mut flat_turns = BTreeSet::new();
        for (number, span) in spans {
            let turn = &self.turns[&number];
            let Some(end) = &turn.end else {
                flat_turns.insert(number);
                continue;
            };
            let after = last_input.is_some_and(|index| index > span.last)
                || self.inputs.last().is_some_and(|seq| *seq > end.seq);
            let deferred = options.defer && latest == Some(number) && !after;
            if deferred {
                flat_turns.insert(number);
            }
            let interleaved = turn.first_evidence.is_some_and(|first| {
                first < end.seq
                    && self
                        .inputs
                        .range((
                            std::ops::Bound::Excluded(first),
                            std::ops::Bound::Excluded(end.seq),
                        ))
                        .next()
                        .is_some()
            });
            let can_toggle =
                options.fold && end.kind == EndKind::Completed && span.has_content && !interleaved;
            let answer_step = turn.answer().map_or(0, |answer| answer.step);
            let open = !can_toggle
                || choices
                    .get(&number)
                    .filter(|(step, _)| *step == answer_step)
                    .map_or(deferred, |(_, open)| *open);
            if !open {
                hidden_turns.insert(number);
            }
            let seconds = turn
                .start_time
                .zip(end.time)
                .map(|(start, end)| (end.saturating_sub(start) / 1000).max(1));
            controls.insert(
                span.first,
                Control {
                    number,
                    answer_step,
                    kind: end.kind,
                    seconds,
                    can_toggle,
                    open,
                },
            );
        }
        let has_latest_control = controls
            .values()
            .any(|control| Some(control.number) == latest);
        Layout {
            has_latest_control,
            rows,
            controls,
            hidden_turns,
            flat_turns,
        }
    }
}

fn record_anchor(map: &mut HashMap<String, Anchor>, id: &str, anchor: Anchor) {
    if let Some(previous) = map.get_mut(id) {
        if anchor.seq < previous.seq {
            *previous = anchor;
        }
    } else {
        map.insert(id.to_owned(), anchor);
    }
}

fn visible_input(data: &Value) -> bool {
    let source = &data["source"];
    if source["kind"] == "user-question-reply" {
        return source["outcome"] == "answered";
    }
    source["kind"]
        .as_str()
        .and_then(super::user_visible_source)
        .is_some()
}

#[derive(Clone, Copy)]
pub(crate) struct Options {
    pub(crate) fold: bool,
    pub(crate) defer: bool,
    pub(crate) keep_replies: bool,
}

struct Span {
    first: usize,
    last: usize,
    has_content: bool,
}

#[derive(Default)]
struct Row {
    turn: Option<u64>,
    member: bool,
    duplicate_stop: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct Control {
    pub(crate) number: u64,
    pub(crate) answer_step: u64,
    pub(crate) kind: EndKind,
    pub(crate) seconds: Option<u64>,
    pub(crate) can_toggle: bool,
    pub(crate) open: bool,
}

pub(crate) struct Layout {
    pub(crate) has_latest_control: bool,
    rows: Vec<Row>,
    pub(crate) controls: HashMap<usize, Control>,
    hidden_turns: BTreeSet<u64>,
    flat_turns: BTreeSet<u64>,
}

impl Layout {
    pub(crate) fn turn(&self, index: usize) -> Option<u64> {
        self.rows[index].turn
    }

    pub(crate) fn process_member(&self, index: usize) -> bool {
        self.rows[index].member
    }

    pub(crate) fn hidden(&self, index: usize) -> bool {
        self.rows[index].duplicate_stop
            || self.rows[index].member
                && self
                    .turn(index)
                    .is_some_and(|turn| self.hidden_turns.contains(&turn))
    }

    pub(crate) fn flat(&self, index: usize) -> Option<bool> {
        self.turn(index).map(|turn| self.flat_turns.contains(&turn))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(reason: &str) -> Vec<Value> {
        vec![
            json!({"type":"turn/start","seq":1,"time":10000,"data":{"turn":1}}),
            json!({"type":"user/message","seq":2,"data":{"id":"input","source":{"kind":"user"},
                "content":[{"type":"text","text":"Find the version"}]}}),
            json!({"type":"assistant/message","seq":3,"surfaceOp":"append","data":{"turn":1,"step":1,
                "message":{"id":"interim","content":[{"type":"reasoning","text":"Inspect sources"},
                {"type":"text","text":"Looking up the version."},{"type":"tool-call","name":"read"}]}}}),
            json!({"type":"tool/call","seq":4,"time":10600,"data":{"turn":1,"step":1,"callId":"read","name":"read"}}),
            json!({"type":"tool/result","seq":5,"surfaceOp":"append","data":{"turn":1,"step":1,
                "message":{"source":{"callId":"read"},"content":[{"type":"text","text":"1.0"}]}}}),
            json!({"type":"step/start","seq":6,"data":{"turn":1,"step":2}}),
            json!({"type":"assistant/message","seq":7,"surfaceOp":"append","data":{"turn":1,"step":2,
                "message":{"id":"final","content":[{"type":"reasoning","text":"Summarize"},
                {"type":"text","text":"The version is 1.0."}]}}}),
            json!({"type":"turn/end","seq":8,"time":12670,"data":{"turn":1,"reason":{"kind":reason}}}),
        ]
    }

    fn default_options() -> Options {
        Options {
            fold: true,
            defer: false,
            keep_replies: true,
        }
    }

    #[test]
    fn completed_turn_obeys_reply_policy_and_manual_answer_generation() {
        let events = fixture("completed");
        let projected = crate::history::project_session_events(&events);
        let entries: Vec<_> = projected.entries.iter().collect();
        let mut process = TurnProcess::default();
        for event in &events {
            process.observe_history(event);
        }
        let mut choices = HashMap::new();
        let all = process.layout(&entries, &choices, default_options());
        let control = all.controls[&1];
        assert!(all.has_latest_control && control.can_toggle && !control.open);
        assert_eq!(control.seconds, Some(2));
        assert_eq!(control.answer_step, 2);
        assert!(!all.hidden(0) && all.hidden(1) && !all.hidden(2) && all.hidden(3));
        assert!(all.hidden(4) && !all.hidden(5));

        let final_only = Options {
            keep_replies: false,
            ..default_options()
        };
        assert!(process.layout(&entries, &choices, final_only).hidden(2));
        choices.insert(1, (1, true));
        assert!(process.layout(&entries, &choices, final_only).hidden(2));
        choices.insert(1, (2, true));
        assert!(!process.layout(&entries, &choices, final_only).hidden(2));

        let verbose = Options {
            fold: false,
            ..default_options()
        };
        let ungrouped = process.layout(&entries, &choices, verbose);
        assert!(!ungrouped.controls[&1].can_toggle && !ungrouped.hidden(3));
    }

    #[test]
    fn latest_turn_defers_until_input_and_partial_history_fills_in_duration() {
        let events = fixture("completed");
        let projected = crate::history::project_session_events(&events[2..]);
        let mut entries: Vec<_> = projected.entries.iter().collect();
        let mut process = TurnProcess::default();
        for event in &events[2..] {
            process.observe_history(event);
        }
        let options = Options {
            defer: true,
            ..default_options()
        };
        let choices = HashMap::new();
        let recent = process.layout(&entries, &choices, options);
        assert!(recent.controls[&0].open && !recent.hidden(0));
        assert_eq!(recent.flat(0), Some(true));
        assert_eq!(recent.controls[&0].seconds, None);
        for event in &events[..2] {
            process.observe_history(event);
        }
        assert_eq!(
            process.layout(&entries, &choices, options).controls[&0].seconds,
            Some(2)
        );
        let pending = ChatEntry::local(EntryRole::User, "next", "Continue");
        entries.push(&pending);
        let older = process.layout(&entries, &choices, options);
        assert!(!older.controls[&0].open && older.hidden(0));
        assert_eq!(older.flat(0), Some(false));
    }

    #[test]
    fn stopped_and_steered_turns_keep_process_visible() {
        for reason in ["aborted", "interrupted", "error", "completed"] {
            let mut events = fixture(reason);
            if reason == "completed" {
                for event in &mut events[5..] {
                    event["seq"] = json!(event["seq"].as_u64().unwrap() + 1);
                }
                events.insert(5, json!({"type":"user/message","seq":6,"data":{
                    "id":"steer","source":{"kind":"user"},"content":[{"type":"text","text":"Check again"}]}}));
            }
            let projected = crate::history::project_session_events(&events);
            let entries: Vec<_> = projected.entries.iter().collect();
            let mut process = TurnProcess::default();
            for event in &events {
                process.observe_history(event);
            }
            let layout = process.layout(&entries, &HashMap::new(), default_options());
            let control = layout.controls[&1];
            assert!(control.open && !control.can_toggle && !layout.hidden(3));
            if matches!(reason, "aborted" | "interrupted") {
                assert_eq!(control.kind, EndKind::Stopped);
                assert!(layout.hidden(entries.len() - 1));
            }
        }
    }

    #[test]
    fn live_anchors_can_arrive_before_the_acp_content() {
        let mut process = TurnProcess::default();
        for update in [
            json!({"type":"turn/start","turn":1,"seq":1,"time":10000}),
            json!({"type":"tool/call","turn":1,"step":1,"seq":2,
                "toolCall":{"toolCallId":"read","time":10480}}),
            json!({"type":"assistant/message","turn":1,"step":2,"seq":4,
                "assistantId":"final","hasReply":true,"hasVisibleContent":true}),
            json!({"type":"turn/end","turn":1,"seq":5,"time":12670,"reason":"completed"}),
        ] {
            process.observe_live(&update);
        }
        let mut entries = Vec::new();
        for update in [
            json!({"sessionUpdate":"tool_call","toolCallId":"read","title":"read","status":"in_progress"}),
            json!({"sessionUpdate":"agent_message_chunk","messageId":"final",
                "content":{"type":"text","text":"This is the final answer."}}),
        ] {
            crate::state::apply_update(&mut entries, &update);
        }
        let layout = process.layout(
            &entries.iter().collect::<Vec<_>>(),
            &HashMap::new(),
            default_options(),
        );
        assert_eq!(process.tool_time("read"), Some(10480));
        assert_eq!(process.end_kind(1), Some(EndKind::Completed));
        assert_eq!(layout.turn(0), Some(1));
        assert_eq!(layout.controls[&0].answer_step, 2);
        assert!(layout.hidden(0) && !layout.hidden(1));
    }
}
