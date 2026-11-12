//! ACP conversation state shared by the selected view and background sessions.

use super::*;

#[derive(Default)]
pub(super) struct Conversation {
    pub(super) acp_open: bool,
    next_local_id: u64,
    pub(super) draft: String,
    pub(super) draft_workspace: Option<String>,
    pub(super) error: Option<String>,
    pub(super) running: bool,
    pub(super) prompt_inflight: bool,
    pub(super) turn_started: Option<f64>,
    pub(super) turn_summary: Option<TurnSummary>,
    pub(super) turn_usage_base: Option<u64>,
    pub(super) queued_prompts: Vec<QueuedPrompt>,
    pub(super) wrapped_code: std::collections::HashSet<String>,
    pub(super) turn_process: crate::history::turn_process::TurnProcess,
    pub(super) turn_disclosures: HashMap<u64, (u64, bool)>,
    pub(super) turn_tails: crate::history::turn_tails::TurnTails,
    pub(super) resumed_session: bool,
    pub(super) entries: Vec<ChatEntry>,
    pub(super) assistant_stream: crate::assistant_stream::AssistantStream,
    pub(super) usage: Option<(u64, u64)>,
    pub(super) permission: Option<PermissionRequest>,
    pub(super) permission_mode: super::permission_picker::PermissionMode,
    pub(super) plan_reviews: Vec<plan::PlanReview>,
    pub(super) questions: Vec<question_panel::PendingQuestions>,
    pub(super) settled_questions: HashMap<String, Vec<crate::questions::RecordedAnswer>>,
    pub(super) queued_question_replies: HashMap<String, Vec<crate::questions::RecordedAnswer>>,
    pub(super) submitted_plans: HashMap<u64, Vec<crate::submitted_plan::SubmittedPlan>>,
    pub(super) plan_active: Option<bool>,
    pub(super) plan_mode_busy: bool,
    pub(super) command_busy: bool,
    pub(super) config_options: Vec<Value>,
    pub(super) command_model_pending: bool,
    pub(super) model_menu_pending: Option<(ComposerMenu, String)>,
    pub(super) scroll: ScrollHandle,
    pub(super) follow_tail: bool,
    pub(super) flow_motion: super::flow_motion::FlowMotion,
    pub(super) flow_scroll: super::flow_scroll::FlowScroll,
    pub(super) last_turn_settled: Option<f64>,
    pub(super) process_viewports: HashMap<String, super::process_scroll::ProcessViewport>,
    pub(super) expanded_entries: std::collections::HashSet<String>,
    pub(super) decode_tokens: u64,
    pub(super) decode_ms: f64,
    pub(super) uncached_input_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) cache_write_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) stats_turns: u64,
    pub(super) stats_steps: u64,
    pub(super) stats_llm_ms: f64,
    pub(super) stats_tool_ms: f64,
    pub(super) stats_ttft_ms: f64,
    pub(super) stats_ttft_steps: u64,
    pub(super) history_has_more: bool,
    pub(super) history_loading: bool,
    pub(super) history_prepend_pending: bool,
    pub(super) history_oldest_seq: Option<u64>,
    pub(super) draft_attachments: Vec<DraftAttachment>,
}

impl Conversation {
    pub(super) fn fresh() -> Self {
        Self {
            follow_tail: true,
            ..Self::default()
        }
    }

    pub(super) fn begin_turn(&mut self) {
        self.running = true;
        self.turn_started = Some(bridge::monotonic_ms());
        self.turn_summary = None;
        self.turn_usage_base = self.usage.map(|(used, _)| used);
    }

    pub(super) fn finish_turn(&mut self, end: TurnEnd) {
        let seconds = self
            .turn_started
            .map(|started| ((bridge::monotonic_ms() - started) / 1000.) as f32)
            .unwrap_or(0.);
        let used = self
            .usage
            .map(|(used, _)| used.saturating_sub(self.turn_usage_base.unwrap_or(0)))
            .filter(|used| *used > 0);
        if let Some(tokens) = used {
            if seconds > 0.05 {
                self.decode_tokens = self.decode_tokens.saturating_add(tokens);
                self.decode_ms += f64::from(seconds) * 1000.;
            }
        }
        self.turn_started = None;
        self.last_turn_settled = Some(bridge::monotonic_ms());
        self.turn_summary = Some(TurnSummary { end, seconds, used });
        self.running = false;
        self.assistant_stream.clear();
        self.permission = None;
    }

    pub(super) fn push_local(&mut self, role: EntryRole, text: String) {
        let key = format!("local-{}", self.next_local_id);
        self.next_local_id += 1;
        let mut entry = ChatEntry::local(role, key, text);
        entry.at_ms = Some(bridge::now_ms());
        self.entries.push(entry);
    }

    pub(super) fn reconcile_assistant_stream(
        &mut self,
        message_id: Option<&str>,
        role: EntryRole,
        committed_key: &str,
    ) {
        for key in self.assistant_stream.reconcile(message_id, role) {
            if self.expanded_entries.remove(&key) {
                self.expanded_entries.insert(committed_key.to_owned());
            }
            if self.expanded_entries.remove(&format!("process-{key}")) {
                self.expanded_entries
                    .insert(format!("process-{committed_key}"));
            }
        }
    }

    pub(super) fn ingest_draft_items(&mut self, items: &[Value]) {
        for item in items {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("未命名")
                .to_string();
            if id.is_empty() || name.is_empty() {
                continue;
            }
            let size = item.get("size").and_then(Value::as_u64).unwrap_or(0);
            let mime = item
                .get("mime")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let kind = if item.get("kind").and_then(Value::as_str) == Some("image")
                || mime.starts_with("image/")
            {
                AttachmentKind::Image
            } else {
                AttachmentKind::File
            };
            let preview = item
                .get("preview")
                .and_then(Value::as_str)
                .map(str::to_string);
            if self
                .draft_attachments
                .iter()
                .any(|existing| existing.name == name && existing.size == size)
            {
                continue;
            }
            self.draft_attachments.push(DraftAttachment {
                id,
                name,
                size,
                kind,
                mime,
                preview,
            });
        }
    }

    pub(super) fn apply_permission(&mut self, event: &Value) {
        let tool_call = event.get("toolCall").cloned().unwrap_or_else(|| json!({}));
        if let Some(call_id) = tool_call.get("toolCallId").and_then(Value::as_str)
            && let Some(entry) = self.entries.iter_mut().find(|entry| {
                entry.role == EntryRole::Tool && entry.external_id.as_deref() == Some(call_id)
            })
        {
            entry.status = "approval".to_string();
        }
        let options = event
            .get("options")
            .and_then(Value::as_array)
            .map(|options| {
                options
                    .iter()
                    .filter_map(|option| {
                        Some((
                            option.get("optionId")?.as_str()?.to_string(),
                            option.get("name")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.permission = Some(PermissionRequest {
            request_id: event.get("requestId").cloned().unwrap_or(Value::Null),
            tool_call,
            options,
        });
    }
}
