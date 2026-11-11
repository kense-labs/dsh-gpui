//! Temporary thought and answer rows sourced from the Harness assistant stream.

use crate::state::{ChatEntry, EntryRole};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct AssistantStream {
    active: Option<Attempt>,
}

struct Attempt {
    id: String,
    last_chunk: Option<u64>,
    message_id: Option<String>,
    blocks: BTreeMap<u64, ChatEntry>,
}

impl AssistantStream {
    pub(crate) fn update(&mut self, update: &Value) {
        let frame = &update["frame"];
        let Some(id) = frame["attemptId"].as_str() else {
            return;
        };
        if frame["type"] == "start" {
            self.active = Some(Attempt {
                id: id.to_owned(),
                last_chunk: None,
                message_id: None,
                blocks: BTreeMap::new(),
            });
            return;
        }
        let Some(attempt) = self.active.as_mut().filter(|attempt| attempt.id == id) else {
            return;
        };
        match frame["type"].as_str() {
            Some("chunk") if attempt.message_id.is_none() => attempt.chunk(frame),
            Some("end") => {
                if let Some(message_id) = update["messageId"].as_str() {
                    // Keep each preview until its ordinary ACP content arrives.
                    // Standard ACP still owns durable text and rich content.
                    attempt.message_id = Some(message_id.to_owned());
                    attempt.blocks.retain(|_, entry| match entry.role {
                        EntryRole::Thought => update["hasThought"] == true,
                        EntryRole::Assistant => update["hasText"] == true,
                        _ => false,
                    });
                } else {
                    self.clear();
                }
            }
            _ => {}
        }
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = &ChatEntry> {
        self.active
            .iter()
            .flat_map(|attempt| attempt.blocks.values())
            .filter(|entry| !entry.text.trim().is_empty())
    }

    pub(crate) fn reconcile(&mut self, message_id: Option<&str>, role: EntryRole) -> Vec<String> {
        let mut replaced = Vec::new();
        if let Some(attempt) = self.active.as_mut()
            && attempt.message_id.is_some()
            && attempt.message_id.as_deref() == message_id
        {
            attempt.blocks.retain(|_, entry| {
                if entry.role == role {
                    replaced.push(entry.key.clone());
                    return false;
                }
                true
            });
            if attempt.blocks.is_empty() {
                self.clear();
            }
        }
        replaced
    }

    pub(crate) fn clear(&mut self) {
        self.active = None;
    }
}

impl Attempt {
    fn chunk(&mut self, frame: &Value) {
        let (Some(sequence), Some(block_index)) =
            (frame["index"].as_u64(), frame["chunk"]["index"].as_u64())
        else {
            return;
        };
        if self.last_chunk.is_some_and(|last| sequence <= last) {
            return;
        }
        self.last_chunk = Some(sequence);
        let chunk = &frame["chunk"];
        let (role, text, replace) = match chunk["type"].as_str() {
            Some("reasoning-delta") => (EntryRole::Thought, &chunk["text"], false),
            Some("text-delta") => (EntryRole::Assistant, &chunk["text"], false),
            Some("block-end") => match chunk["block"]["type"].as_str() {
                Some("reasoning") => (EntryRole::Thought, &chunk["block"]["text"], true),
                Some("text") => (EntryRole::Assistant, &chunk["block"]["text"], true),
                _ => return,
            },
            _ => return,
        };
        let Some(text) = text.as_str() else {
            return;
        };
        let entry = self.blocks.entry(block_index).or_insert_with(|| {
            ChatEntry::local(role, format!("stream-{}-{block_index}", self.id), "")
        });
        if replace || entry.role != role {
            entry.role = role;
            entry.text.clear();
        }
        entry.text.push_str(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(id: &str, kind: &str) -> Value {
        json!({"frame":{"attemptId":id,"type":kind}})
    }

    fn chunk(id: &str, index: u64, chunk: Value) -> Value {
        json!({"frame":{"attemptId":id,"type":"chunk","index":index,"chunk":chunk}})
    }

    #[test]
    fn thought_streams_before_text_and_settles_without_duplicate_rows() {
        let mut stream = AssistantStream::default();
        stream.update(&frame("attempt", "start"));
        let thought = chunk(
            "attempt",
            0,
            json!({"type":"reasoning-delta","index":0,"text":"First"}),
        );
        stream.update(&thought);
        stream.update(&thought);
        assert_eq!(stream.entries().next().unwrap().text, "First");
        assert_eq!(stream.entries().count(), 1);
        stream.update(&chunk(
            "attempt",
            2,
            json!({"type":"reasoning-delta","index":0,"text":" thought"}),
        ));
        assert_eq!(stream.entries().next().unwrap().text, "First thought");
        stream.update(&chunk(
            "attempt",
            4,
            json!({"type":"text-delta","index":1,"text":"Answer"}),
        ));
        stream.update(&json!({"frame":{"attemptId":"attempt","type":"end"},
            "messageId":"committed","hasThought":true,"hasText":true}));
        stream.reconcile(Some("previous"), EntryRole::Thought);
        assert_eq!(stream.entries().count(), 2);
        assert_eq!(
            stream.reconcile(Some("committed"), EntryRole::Thought),
            ["stream-attempt-0"],
        );
        assert_eq!(stream.entries().next().unwrap().text, "Answer");
        stream.reconcile(Some("committed"), EntryRole::Assistant);
        assert_eq!(stream.entries().count(), 0);
    }

    #[test]
    fn abandoned_attempt_is_removed_before_retry() {
        let mut stream = AssistantStream::default();
        stream.update(&frame("first", "start"));
        let old = chunk(
            "first",
            1,
            json!({"type":"reasoning-delta","index":0,"text":"Discarded"}),
        );
        stream.update(&old);
        stream.update(&frame("first", "end"));
        stream.update(&frame("retry", "start"));
        stream.update(&old);
        assert_eq!(stream.entries().count(), 0);
        stream.update(&chunk(
            "retry",
            1,
            json!({"type":"text-delta","index":0,"text":"Draft"}),
        ));
        stream.update(&chunk(
            "retry",
            2,
            json!({"type":"block-end","index":0,
            "block":{"type":"text","text":"Final"}}),
        ));
        assert_eq!(stream.entries().next().unwrap().text, "Final");
        stream.clear();
        assert_eq!(stream.entries().count(), 0);
    }
}
