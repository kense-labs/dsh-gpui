use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

#[derive(Default)]
struct Turn {
    end: Option<u64>,
    steps: HashMap<u64, (u64, Option<String>)>,
    closing: Option<String>,
    latest_other: Option<u64>,
    plan_tail: Option<String>,
}

struct ClosingAnswer {
    branch_boundary: Option<u64>,
    plan_tail: Option<String>,
}

/// Match the closing finalized text answer for each ended turn. Sequence
/// numbers let older history pages fill in a turn across a page boundary.
#[derive(Default)]
pub(crate) struct TurnTails {
    turns: BTreeMap<u64, Turn>,
    answers: HashMap<String, ClosingAnswer>,
    plan_answers: HashMap<String, String>,
}

impl TurnTails {
    pub(crate) fn contains(&self, message: &str) -> bool {
        self.answers.contains_key(message)
    }

    pub(crate) fn branch_boundary(&self, message: &str) -> Option<u64> {
        self.answers.get(message)?.branch_boundary
    }

    pub(crate) fn follows_plan(&self, message: &str) -> bool {
        self.answers
            .get(message)
            .is_some_and(|answer| answer.plan_tail.is_some())
    }

    pub(crate) fn answer_after_plan(&self, call_id: &str) -> Option<&str> {
        self.plan_answers.get(call_id).map(String::as_str)
    }

    pub(crate) fn note_plan_tail(&mut self, turn: u64, call_id: &str) {
        self.turns.entry(turn).or_default().plan_tail = Some(call_id.to_owned());
    }

    pub(crate) fn latest(&self) -> Option<&str> {
        self.turns
            .values()
            .rfind(|turn| turn.end.is_some())?
            .closing
            .as_deref()
    }

    pub(crate) fn observe_history(&mut self, event: &Value) {
        let kind = event["type"].as_str().unwrap_or_default();
        let data = &event["data"];
        if matches!(kind, "assistant/message" | "tool/result") && event["surfaceOp"] != "append" {
            return;
        }
        if kind == "turn/end"
            && let (Some(turn), Some(call_id)) = (
                data["turn"].as_u64(),
                event["gpuiSubmittedPlans"]
                    .as_array()
                    .and_then(|plans| plans.last())
                    .and_then(|plan| plan["callId"].as_str()),
            )
        {
            self.note_plan_tail(turn, call_id);
        }
        let message = if kind == "assistant/message" {
            let message = &data["message"];
            let has_text = message["content"].as_array().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block["type"] == "text"
                        && block["text"].as_str().is_some_and(|text| {
                            !text
                                .trim_matches(|ch: char| {
                                    ch == '\u{feff}' || (ch != '\u{0085}' && ch.is_whitespace())
                                })
                                .is_empty()
                        })
                })
            });
            if has_text {
                message["id"].as_str()
            } else {
                None
            }
        } else {
            None
        };
        self.observe(
            kind,
            &event["seq"],
            data,
            message,
            data["reason"]["kind"] == "error",
        );
    }

    pub(crate) fn observe_live(&mut self, update: &Value) {
        self.observe(
            update["type"].as_str().unwrap_or_default(),
            &update["seq"],
            update,
            update["messageId"].as_str(),
            update["reason"] == "error",
        );
    }

    fn observe(
        &mut self,
        kind: &str,
        seq: &Value,
        coords: &Value,
        message: Option<&str>,
        error: bool,
    ) {
        let (Some(seq), Some(number)) = (seq.as_u64(), coords["turn"].as_u64()) else {
            return;
        };
        if !matches!(
            kind,
            "turn/end" | "assistant/message" | "llm/retry" | "tool/call" | "tool/result"
        ) {
            return;
        }
        let turn = self.turns.entry(number).or_default();
        if matches!(kind, "llm/retry" | "tool/call" | "tool/result") || error {
            turn.latest_other = turn.latest_other.max(Some(seq));
        }
        match kind {
            "turn/end" => turn.end = turn.end.max(Some(seq)),
            "assistant/message" | "llm/retry" => {
                let Some(step) = coords["step"].as_u64() else {
                    return;
                };
                let previous = turn.steps.entry(step);
                previous
                    .and_modify(|current| {
                        if seq > current.0 {
                            *current = (seq, message.map(str::to_owned));
                        }
                    })
                    .or_insert_with(|| (seq, message.map(str::to_owned)));
            }
            _ => {}
        }
        if let Some(end) = turn.end {
            if let Some(previous) = turn.closing.take()
                && let Some(answer) = self.answers.remove(&previous)
                && let Some(call_id) = answer.plan_tail
            {
                self.plan_answers.remove(&call_id);
            }
            let closing = turn
                .steps
                .values()
                .filter(|(_, message)| message.is_some())
                .max_by_key(|(seq, _)| seq);
            if let Some((seq, Some(message))) = closing {
                let latest = turn
                    .steps
                    .values()
                    .map(|(seq, _)| *seq)
                    .max()
                    .max(turn.latest_other);
                self.answers.insert(
                    message.clone(),
                    ClosingAnswer {
                        branch_boundary: (latest == Some(*seq)).then_some(end),
                        plan_tail: turn.plan_tail.clone(),
                    },
                );
                if let Some(call_id) = &turn.plan_tail {
                    self.plan_answers.insert(call_id.clone(), message.clone());
                }
                turn.closing = Some(message.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paged_history_only_rates_the_closing_text_of_an_ended_turn() {
        let mut tails = TurnTails::default();
        tails.observe_history(&json!({"type":"turn/end","seq":9,"data":{"turn":1}}));
        let assistant = |seq, turn, step, id, text, surface| {
            json!({
                "type":"assistant/message","seq":seq,"surfaceOp":surface,
                "data":{"turn":turn,"step":step,"message":{"id":id,"content":[{"type":"text","text":text}]}}
            })
        };
        tails.observe_history(&assistant(3, 1, 1, "intermediate", "first step", "append"));
        tails.observe_history(&assistant(5, 1, 2, "closing", "finished answer", "append"));
        tails.observe_history(&assistant(6, 1, 3, "blank", " \t", "append"));
        tails.observe_history(&assistant(7, 1, 3, "replacement", "internal", "replace"));
        tails.observe_history(&assistant(11, 2, 1, "in-progress", "not settled", "append"));

        assert_eq!(tails.latest(), Some("closing"));
        assert!(tails.contains("closing"));
        assert_eq!(tails.branch_boundary("closing"), None);
        assert!(!tails.contains("intermediate"));
        assert!(!tails.contains("blank"));
        assert!(!tails.contains("replacement"));
        assert!(!tails.contains("in-progress"));
    }

    #[test]
    fn retry_drops_the_preceding_attempt_and_live_end_keeps_earlier_turns() {
        let mut tails = TurnTails::default();
        for update in [
            json!({"type":"assistant/message","turn":1,"step":1,"seq":2,"messageId":"first"}),
            json!({"type":"turn/end","turn":1,"seq":3}),
            json!({"type":"assistant/message","turn":2,"step":1,"seq":5,"messageId":"retried"}),
            json!({"type":"llm/retry","turn":2,"step":1,"seq":6}),
            json!({"type":"turn/end","turn":2,"seq":7}),
        ] {
            tails.observe_live(&update);
        }
        assert_eq!(tails.latest(), None);
        assert!(tails.contains("first"));
        assert_eq!(tails.branch_boundary("first"), Some(3));
        assert!(!tails.contains("retried"));
    }

    #[test]
    fn an_earlier_page_anchors_actions_after_only_its_own_final_plan() {
        let mut tails = TurnTails::default();
        tails.observe_history(&json!({"type":"turn/end","seq":8,"data":{"turn":1},
            "gpuiSubmittedPlans":[{"callId":"plan-a"},{"callId":"plan-b"}]}));
        tails.observe_history(&json!({"type":"assistant/message","seq":7,"surfaceOp":"append",
            "data":{"turn":1,"step":2,"message":{"id":"closing","content":[{"type":"text","text":"done"}]}}}));
        assert!(tails.follows_plan("closing"));
        assert_eq!(tails.answer_after_plan("plan-b"), Some("closing"));
        assert_eq!(tails.answer_after_plan("plan-a"), None);
        assert_eq!(tails.branch_boundary("closing"), Some(8));
    }

    #[test]
    fn branch_uses_the_turn_end_and_rejects_later_tools_or_errors() {
        let mut tails = TurnTails::default();
        for update in [
            json!({"type":"assistant/message","turn":1,"step":1,"seq":2,"messageId":"first"}),
            json!({"type":"turn/end","turn":1,"seq":3}),
            json!({"type":"assistant/message","turn":2,"step":1,"seq":5,"messageId":"tool-tail"}),
            json!({"type":"tool/call","turn":2,"step":1,"seq":6}),
            json!({"type":"turn/end","turn":2,"seq":7}),
            json!({"type":"assistant/message","turn":3,"step":1,"seq":9,"messageId":"error-tail"}),
            json!({"type":"turn/end","turn":3,"seq":10,"reason":"error"}),
        ] {
            tails.observe_live(&update);
        }
        assert_eq!(tails.branch_boundary("first"), Some(3));
        for message in ["tool-tail", "error-tail"] {
            assert!(tails.contains(message));
            assert_eq!(tails.branch_boundary(message), None);
        }
    }
}
