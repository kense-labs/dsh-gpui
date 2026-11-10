//! Draft and countdown state for the Harness question composer.

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Question {
    pub(crate) id: String,
    pub(crate) question: String,
    pub(crate) header: Option<String>,
    pub(crate) detail: Option<String>,
    #[serde(default)]
    pub(crate) options: Vec<Choice>,
    #[serde(default, alias = "multi_select")]
    pub(crate) multi_select: bool,
}

#[derive(Deserialize)]
pub(crate) struct Choice {
    pub(crate) label: String,
    pub(crate) description: Option<String>,
}

impl Choice {
    pub(crate) fn display(&self) -> (&str, bool) {
        let trimmed = self.label.trim_end();
        let suffix = trimmed
            .strip_suffix(')')
            .and_then(|text| text.rsplit_once('('))
            .or_else(|| {
                trimmed
                    .strip_suffix('）')
                    .and_then(|text| text.rsplit_once('（'))
            });
        if let Some((label, marker)) = suffix
            && (marker.eq_ignore_ascii_case("recommended") || marker == "推荐")
        {
            return (label.trim_end(), true);
        }
        (&self.label, false)
    }
}

#[derive(Default)]
pub(crate) struct Draft {
    pub(crate) selected: Vec<String>,
    pub(crate) custom: String,
    pub(crate) skipped: bool,
}

impl Draft {
    pub(crate) fn new(question: &Question) -> Self {
        let choices = if question.multi_select {
            question.options.len()
        } else {
            1
        };
        let selected = question
            .options
            .iter()
            .take(choices)
            .filter(|choice| choice.display().1)
            .map(|choice| choice.label.clone())
            .collect();
        Self {
            selected,
            ..Self::default()
        }
    }

    pub(crate) fn choose(&mut self, label: String, multi_select: bool) {
        self.skipped = false;
        if multi_select {
            if let Some(index) = self.selected.iter().position(|choice| *choice == label) {
                self.selected.remove(index);
            } else {
                self.selected.push(label);
            }
        } else {
            self.selected = vec![label];
            self.custom.clear();
        }
    }

    pub(crate) fn type_answer(&mut self, text: String, multi_select: bool) {
        if !multi_select {
            self.selected.clear();
        }
        self.custom = text;
        self.skipped = false;
    }

    pub(crate) fn skip(&mut self) {
        *self = Self {
            skipped: true,
            ..Self::default()
        };
    }

    pub(crate) fn answered(&self) -> bool {
        !self.selected.is_empty() || !self.custom.trim().is_empty()
    }

    pub(crate) fn payload(&self, question: &Question) -> Value {
        let mut answer = json!({"id": question.id, "selected": self.selected});
        if !self.skipped && !self.custom.trim().is_empty() {
            answer["custom"] = self.custom.trim().into();
        }
        answer
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Clock {
    Counting { deadline: f64 },
    Paused { remaining: f64 },
    Held,
}

impl Clock {
    pub(crate) fn step(self, now: f64, holding: bool) -> Self {
        match self {
            Self::Counting { deadline } if holding => Self::Paused {
                remaining: (deadline - now).max(0.),
            },
            Self::Paused { remaining } if !holding => Self::Counting {
                deadline: now + remaining,
            },
            _ => self,
        }
    }

    pub(crate) fn remaining(self, now: f64) -> Option<f64> {
        match self {
            Self::Counting { deadline } => Some((deadline - now).max(0.)),
            Self::Paused { remaining } => Some(remaining),
            Self::Held => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommended_display_preserves_the_labels_sent_to_the_host() {
        let mut question: Question = serde_json::from_value(json!({
            "id":"one","question":"Where?",
            "options":[{"label":"Local （推荐）"},{"label":"Remote (RECOMMENDED)"}]
        }))
        .unwrap();
        let mut draft = Draft::new(&question);
        assert_eq!(question.options[0].display(), ("Local", true));
        assert_eq!(
            draft.payload(&question),
            json!({"id":"one","selected":["Local （推荐）"]})
        );
        draft.type_answer(" manual ".into(), false);
        assert_eq!(
            draft.payload(&question),
            json!({"id":"one","selected":[],"custom":"manual"})
        );
        question.multi_select = true;
        let mut draft = Draft::new(&question);
        draft.type_answer("also this".into(), true);
        draft.choose(question.options[0].label.clone(), true);
        assert_eq!(
            draft.payload(&question),
            json!({"id":"one","selected":["Remote (RECOMMENDED)"],"custom":"also this"})
        );
        draft.skip();
        assert_eq!(draft.payload(&question), json!({"id":"one","selected":[]}));
    }

    #[test]
    fn focus_holds_remaining_time_and_release_resumes_it() {
        let clock = Clock::Counting { deadline: 1500. };
        let paused = clock.step(700., true);
        assert_eq!(paused.remaining(3000.), Some(800.));
        assert_eq!(paused.step(3000., false).remaining(3800.), Some(0.));
        assert_eq!(Clock::Held.step(5000., false).remaining(8000.), None);
    }
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub(crate) struct RecordedAnswer {
    pub(crate) id: String,
    pub(crate) selected: Vec<String>,
    pub(crate) custom: Option<String>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
pub(crate) struct ReplyQuestion {
    pub(crate) id: String,
    pub(crate) question: String,
    pub(crate) header: Option<String>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
pub(crate) struct QuestionReply {
    pub(crate) questions: Vec<ReplyQuestion>,
    pub(crate) answers: Vec<RecordedAnswer>,
}

impl QuestionReply {
    pub(crate) fn answer_values(&self, id: &str) -> Vec<&str> {
        self.answers
            .iter()
            .find(|answer| answer.id == id)
            .map(|answer| {
                answer
                    .selected
                    .iter()
                    .map(String::as_str)
                    .chain(
                        answer
                            .custom
                            .as_deref()
                            .map(str::trim)
                            .filter(|text| !text.is_empty()),
                    )
                    .collect()
            })
            .unwrap_or_default()
    }

    fn clipboard(&self, raw: &str) -> String {
        if self.questions.is_empty() {
            return raw.to_owned();
        }
        self.questions
            .iter()
            .map(|question| {
                let heading = question
                    .header
                    .as_deref()
                    .filter(|header| !header.is_empty() && *header != question.question)
                    .map_or_else(
                        || question.question.clone(),
                        |header| format!("{header} — {}", question.question),
                    );
                let values = self.answer_values(&question.id);
                let answer = if values.is_empty() {
                    "已跳过".to_owned()
                } else {
                    format!("回答：{}", values.join(", "))
                };
                format!("{heading}\n{answer}")
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

pub(crate) fn reply_entry(
    event: &Value,
    arguments: Option<&Value>,
) -> Option<crate::state::ChatEntry> {
    use crate::state::{ChatEntry, EntryRole};
    let data = &event["data"];
    let kind = event["type"].as_str()?;
    let (id, raw, reply) = if kind == "user/message" {
        let source = &data["source"];
        if event["surfaceOp"] != "append"
            || source["kind"] != "user-question-reply"
            || source["outcome"] != "answered"
            || !source["callId"].is_string()
        {
            return None;
        }
        let raw = data["content"]
            .as_array()?
            .iter()
            .find(|block| block["type"] == "text")
            .and_then(|block| block["text"].as_str())
            .unwrap_or_default();
        let value: Value = serde_json::from_str(raw).unwrap_or_default();
        let questions = value["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|question| serde_json::from_value(question.clone()).ok())
            .collect();
        let answers = value["answers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|answer| {
                Some(RecordedAnswer {
                    id: answer["id"].as_str()?.to_owned(),
                    selected: answer["selected"]
                        .as_array()?
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                    custom: answer["custom"].as_str().map(str::to_owned),
                })
            })
            .collect();
        (
            format!("late-{}", data["id"].as_str()?),
            raw,
            QuestionReply { questions, answers },
        )
    } else {
        let (id, result) = match kind {
            "tool/result" if event["surfaceOp"] == "append" => {
                let message = &data["message"];
                (
                    message
                        .pointer("/source/callId")
                        .or_else(|| message.get("toolCallId"))?
                        .as_str()?,
                    message,
                )
            }
            "tool/ptc-dispatch" if data["name"] == "ask_user_question" => {
                (data["subCallId"].as_str()?, data)
            }
            _ => return None,
        };
        if result["isError"] == true {
            return None;
        }
        let [block] = result["content"].as_array()?.as_slice() else {
            return None;
        };
        if block["type"] != "text" {
            return None;
        }
        let raw = block["text"].as_str()?;
        let value: Value = serde_json::from_str(raw).ok()?;
        if value["pending"] == true {
            return None;
        }
        let arguments = arguments.or_else(|| data.get("arguments"))?;
        let parsed: Value;
        let arguments = if let Some(raw) = arguments.as_str() {
            parsed = serde_json::from_str(raw).ok()?;
            &parsed
        } else {
            arguments
        };
        let mut ids = std::collections::HashSet::new();
        let questions = arguments["questions"]
            .as_array()?
            .iter()
            .map(|question| {
                let item: ReplyQuestion = serde_json::from_value(question.clone()).ok()?;
                if question.pointer("/intent/kind").and_then(Value::as_str) == Some("plan-review")
                    || !ids.insert(item.id.clone())
                {
                    return None;
                }
                Some(item)
            })
            .collect::<Option<Vec<_>>>()?;
        let answers: Vec<RecordedAnswer> = serde_json::from_value(value["answers"].clone()).ok()?;
        if questions.is_empty()
            || answers.len() != questions.len()
            || !answers.iter().all(|answer| ids.remove(&answer.id))
        {
            return None;
        }
        (
            format!("result-{id}"),
            raw,
            QuestionReply { questions, answers },
        )
    };
    let mut entry = ChatEntry::local(
        EntryRole::QuestionReply,
        format!("question-{id}"),
        reply.clipboard(raw),
    );
    entry.at_ms = event["time"].as_f64();
    entry.external_id = Some(id);
    entry.input = serde_json::to_string(&reply).ok()?;
    Some(entry)
}

/// ACP emits native tool rows itself. The durable event supplies exact question verdicts.
pub(crate) fn apply_question_call_event(entries: &mut [crate::state::ChatEntry], event: &Value) {
    use crate::state::EntryRole;
    if event["type"] != "tool/result" || event.get("questionArguments").is_none() {
        return;
    }
    let data = &event["data"];
    let status = crate::tool_calls::result_status(data, &data["message"]);
    if matches!(status, "ASK_CANCELLED" | "ASK_ABORTED")
        && let Some(id) = data["message"]
            .pointer("/source/callId")
            .or_else(|| data["message"].get("toolCallId"))
            .and_then(Value::as_str)
        && let Some(entry) = entries.iter_mut().find(|entry| {
            entry.role == EntryRole::Tool
                && !entry.is_subcall()
                && entry.external_id.as_deref() == Some(id)
        })
    {
        entry.status = status.to_owned();
    }
}
