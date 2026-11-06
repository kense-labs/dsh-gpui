//! GPUI application, WASM lifecycle, and Tauri event handling.

use crate::{
    bridge,
    files::{
        FileToken, WorkspaceFile, active_file_token, file_mention, replace_token, utf16_byte_index,
    },
    sessions::{SessionSummary, merge_session_page, short_session_title},
    state::{ChatEntry, EntryRole, apply_update},
    welcome::{Welcome, WelcomeEvent},
};
use gpui::{
    App, AsyncApp, Context, FontWeight, ScrollHandle, Subscription, Window, WindowHandle,
    WindowOptions, div, prelude::*, px,
};
use kui::components::button::Button;
use kui::components::select::Select;
use kui::foundation::{Theme, ThemeMode};
use kui::primitives::control::UiLanguage;
use kui::primitives::editor::{EditEvent, Editor, EditorShortcutEvent, InputMode};
use kui::primitives::family::Paint;
use kui::primitives::glyph::{Glyph, icon};
use kui::primitives::ops_surface::{column, editor, input, note, replace, row};
use kui::primitives::picker::{PickerOption, SelectionChanged};
use serde_json::{Value, json};
use std::cell::RefCell;
use wasm_bindgen::prelude::*;

struct Host {
    app: AsyncApp,
    window: WindowHandle<DshApp>,
    _event_listener: Closure<dyn FnMut(String)>,
    _key_listener: Closure<dyn FnMut(String) -> bool>,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// Start the GPUI WebAssembly application inside Tauri's webview.
#[wasm_bindgen]
pub fn start() {
    console_error_panic_hook::set_once();
    gpui_platform::web_init();
    gpui_platform::application().run(|cx: &mut App| {
        cx.text_system()
            .add_fonts(vec![std::borrow::Cow::Borrowed(include_bytes!(
                "../assets/NotoSans-Regular.ttf"
            ))])
            .expect("load the Latin application font");
        cx.text_system()
            .add_fonts(vec![std::borrow::Cow::Borrowed(include_bytes!(
                "../assets/KUIInputSans.ttf"
            ))])
            .expect("load the Chinese application font");
        cx.set_global(UiLanguage::Chinese);
        kui::init(cx);
        Theme::global_mut(cx).set_mode(ThemeMode::Dark);
        let window = cx
            .open_window(
                WindowOptions {
                    inactive_frame_interval: None,
                    ..Default::default()
                },
                |w, cx| cx.new(|cx| DshApp::new(w, cx)),
            )
            .expect("open the DeepSeek Harness GPUI window");
        let listener = Closure::<dyn FnMut(String)>::new(|payload: String| {
            if let Ok(event) = serde_json::from_str::<Value>(&payload) {
                with_host(|host| {
                    host.app.update(|cx| {
                        let _ = host.window.update(cx, |app, w, cx| {
                            app.handle_event(event, w, cx);
                        });
                    });
                });
            }
        });
        let key_listener = Closure::<dyn FnMut(String) -> bool>::new(|key: String| {
            with_host(|host| {
                host.app.update(|cx| {
                    host.window
                        .update(cx, |app, w, cx| app.handle_file_key(&key, w, cx))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
        });
        HOST.set(Some(Host {
            app: cx.to_async(),
            window,
            _event_listener: listener,
            _key_listener: key_listener,
        }));
        with_host(|host| {
            bridge::listen_keys(host._key_listener.as_ref().unchecked_ref());
        });
        cx.activate(true);
        wasm_bindgen_futures::spawn_local(async {
            let registered = with_host(|host| {
                wasm_bindgen_futures::JsFuture::from(bridge::listen(
                    host._event_listener.as_ref().unchecked_ref(),
                ))
            });
            if let Some(registered) = registered {
                let _ = registered.await;
            }
            invoke("start_auth", json!({}));
            invoke("load_workspace_preferences", json!({}));
            invoke("start_service", json!({}));
        });
    });
}

fn with_host<R>(operation: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOST.with_borrow_mut(|host| host.as_mut().map(operation))
}

pub(crate) fn invoke(command: &'static str, args: Value) {
    wasm_bindgen_futures::spawn_local(async move {
        let result = bridge::invoke(command, args).await;
        with_host(|host| {
            host.app.update(|cx| {
                let _ = host.window.update(cx, |app, w, cx| {
                    app.handle_reply(command, result, w, cx);
                });
            });
        });
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceStatus {
    Starting,
    Connected,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspacePage {
    Conversation,
    Plugins,
    Automations,
    More,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceGroupBy {
    Workspace,
    WorkspaceTree,
    Flat,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceOrderBy {
    Manual,
    Updated,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceArchivedFilter {
    Hide,
    Show,
    Only,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    Account,
    General,
    Model,
    Plugins,
    AgentPresets,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerMenu {
    Model,
    ModelChoices,
    ReasoningChoices,
    Permission,
    Commands,
    Files,
}

#[derive(Clone)]
enum PendingAction {
    New,
    Resume(SessionSummary),
}

pub(crate) struct PermissionRequest {
    pub(crate) request_id: Value,
    pub(crate) tool_call: Value,
    pub(crate) options: Vec<(String, String)>,
}

pub(crate) struct DshApp {
    pub(crate) status: ServiceStatus,
    service_starting: bool,
    pub(crate) agent_name: String,
    pub(crate) workspace: String,
    pub(crate) service_path: String,
    pub(crate) error: Option<String>,
    pub(crate) sessions: Vec<SessionSummary>,
    pub(crate) active_session: Option<String>,
    pub(crate) page: WorkspacePage,
    requested_session_id: Option<String>,
    pending_action: Option<PendingAction>,
    pub(crate) creating_session: bool,
    pub(crate) running: bool,
    pub(crate) resumed_session: bool,
    pub(crate) entries: Vec<ChatEntry>,
    pub(crate) usage: Option<(u64, u64)>,
    pub(crate) permission: Option<PermissionRequest>,
    pub(crate) config_options: Vec<Value>,
    pub(crate) model_select: Option<gpui::Entity<Select>>,
    pub(crate) reasoning_select: Option<gpui::Entity<Select>>,
    pub(crate) composer: gpui::Entity<Editor>,
    pub(crate) workspace_editor: gpui::Entity<Editor>,
    pub(crate) workspace_rename_editor: gpui::Entity<Editor>,
    pub(crate) service_editor: gpui::Entity<Editor>,
    pub(crate) search_editor: gpui::Entity<Editor>,
    pub(crate) scroll: ScrollHandle,
    pub(crate) search_expanded: bool,
    pub(crate) search_slide: kui::primitives::family::Slide,
    pub(crate) expanded_entries: std::collections::HashSet<String>,
    pub(crate) sidebar_collapsed: bool,
    pub(crate) sidebar_width: f32,
    pub(crate) sidebar_dragging: bool,
    pub(crate) sidebar_slide: kui::primitives::family::Slide,
    pub(crate) brand_hover: kui::primitives::family::Slide,
    pub(crate) show_account_menu: bool,
    pub(crate) show_settings: bool,
    pub(crate) settings_page: SettingsPage,
    pub(crate) show_workspace_menu: bool,
    pub(crate) show_workspace_view_options: bool,
    pub(crate) workspace_context_menu: Option<(String, f32)>,
    pub(crate) hovered_workspace: Option<String>,
    pub(crate) collapsed_workspaces: std::collections::HashSet<String>,
    pub(crate) hidden_workspaces: std::collections::HashSet<String>,
    pub(crate) workspace_aliases: std::collections::HashMap<String, String>,
    pub(crate) workspace_rename: Option<String>,
    pub(crate) workspace_delete: Option<String>,
    pub(crate) confirm_full_access: bool,
    pub(crate) full_access_acknowledged: bool,
    pub(crate) workspace_group_by: WorkspaceGroupBy,
    pub(crate) workspace_order_by: WorkspaceOrderBy,
    pub(crate) workspace_archived_filter: WorkspaceArchivedFilter,
    pub(crate) show_start_panel: bool,
    pub(crate) start_panel_expanded: bool,
    pub(crate) composer_menu: Option<ComposerMenu>,
    pub(crate) file_entries: Vec<WorkspaceFile>,
    pub(crate) file_token: Option<FileToken>,
    pub(crate) file_directory: String,
    pub(crate) file_filter: String,
    pub(crate) file_selected: usize,
    pub(crate) file_request_id: u64,
    pub(crate) file_loading: bool,
    pub(crate) file_error: Option<String>,
    pub(crate) file_truncated: bool,
    pub(crate) file_show_crumbs: bool,
    suppress_file_trigger: bool,
    sessions_loading: bool,
    pub(crate) show_welcome: bool,
    pub(crate) welcome: gpui::Entity<Welcome>,
    next_local_id: u64,
    _composer_subscriptions: Vec<Subscription>,
    _workspace_rename_subscription: Subscription,
    _search_subscription: Subscription,
    _config_subscriptions: Vec<Subscription>,
    _welcome_subscription: Subscription,
}

impl DshApp {
    fn new(w: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = editor(
            "发消息或创建任务, / 调用指令, @ 文件或对话".into(),
            InputMode::InlineMulti,
            w,
            cx,
        );
        composer.update(cx, |editor, _| editor.set_command_submit(true));
        let submit = cx.subscribe_in(&composer, w, |this, _, _: &EditorShortcutEvent, w, cx| {
            this.send_prompt(w, cx);
        });
        let submit_plain = cx.subscribe_in(&composer, w, |this, _, event: &EditEvent, w, cx| {
            if *event == EditEvent::Submitted {
                if this.composer_menu != Some(ComposerMenu::Files) {
                    this.send_prompt(w, cx);
                }
            } else if *event == EditEvent::Changed {
                this.sync_file_trigger(w, cx);
                cx.notify();
            }
        });
        let workspace_editor = editor(
            "工作区绝对路径（留空使用当前目录）".into(),
            InputMode::Single,
            w,
            cx,
        );
        let workspace_rename_editor = editor("工作区名称".into(), InputMode::Single, w, cx);
        let workspace_rename_subscription = cx.subscribe_in(
            &workspace_rename_editor,
            w,
            |this, _, event: &EditEvent, _, cx| match event {
                EditEvent::Submitted => {
                    this.confirm_workspace_rename(cx);
                    cx.notify();
                }
                EditEvent::Escape => {
                    this.close_workspace_dialog();
                    cx.notify();
                }
                EditEvent::Changed => cx.notify(),
                _ => {}
            },
        );
        let service_editor = editor(
            "dsh-acp 路径（留空自动寻找 ./dsh-acp）".into(),
            InputMode::Single,
            w,
            cx,
        );
        let search_editor = editor("搜索会话".into(), InputMode::Single, w, cx);
        let search_subscription =
            cx.subscribe_in(&search_editor, w, |this, _, event: &EditEvent, w, cx| {
                if *event == EditEvent::Escape {
                    replace(&this.search_editor, "", w, cx);
                    this.search_expanded = false;
                }
                cx.notify();
            });
        let welcome = cx.new(|cx| Welcome::new(w, cx));
        let welcome_subscription = cx.subscribe(&welcome, |this, _, event: &WelcomeEvent, cx| {
            match event {
                WelcomeEvent::Changed => {
                    this.show_welcome = !this.welcome.read(cx).signed_in();
                }
            }
            cx.notify();
        });
        Self {
            status: ServiceStatus::Starting,
            service_starting: false,
            agent_name: "deepseek-harness-acp".to_string(),
            workspace: String::new(),
            service_path: String::new(),
            error: None,
            sessions: Vec::new(),
            active_session: None,
            page: WorkspacePage::Conversation,
            requested_session_id: None,
            pending_action: None,
            creating_session: false,
            running: false,
            resumed_session: false,
            entries: Vec::new(),
            usage: None,
            permission: None,
            config_options: Vec::new(),
            model_select: None,
            reasoning_select: None,
            composer,
            workspace_editor,
            workspace_rename_editor,
            service_editor,
            search_editor,
            scroll: ScrollHandle::new(),
            search_expanded: false,
            search_slide: kui::primitives::family::Slide::new(0.),
            expanded_entries: std::collections::HashSet::new(),
            sidebar_collapsed: false,
            sidebar_width: 280.,
            sidebar_dragging: false,
            sidebar_slide: kui::primitives::family::Slide::new(1.),
            brand_hover: kui::primitives::family::Slide::snappy(0.),
            show_account_menu: false,
            show_settings: false,
            settings_page: SettingsPage::Account,
            show_workspace_menu: false,
            show_workspace_view_options: false,
            workspace_context_menu: None,
            hovered_workspace: None,
            collapsed_workspaces: std::collections::HashSet::new(),
            hidden_workspaces: std::collections::HashSet::new(),
            workspace_aliases: std::collections::HashMap::new(),
            workspace_rename: None,
            workspace_delete: None,
            confirm_full_access: false,
            full_access_acknowledged: false,
            workspace_group_by: WorkspaceGroupBy::Workspace,
            workspace_order_by: WorkspaceOrderBy::Updated,
            workspace_archived_filter: WorkspaceArchivedFilter::Hide,
            show_start_panel: true,
            start_panel_expanded: false,
            composer_menu: None,
            file_entries: Vec::new(),
            file_token: None,
            file_directory: String::new(),
            file_filter: String::new(),
            file_selected: 0,
            file_request_id: 0,
            file_loading: false,
            file_error: None,
            file_truncated: false,
            file_show_crumbs: false,
            suppress_file_trigger: false,
            sessions_loading: false,
            show_welcome: true,
            welcome,
            next_local_id: 0,
            _composer_subscriptions: vec![submit, submit_plain],
            _workspace_rename_subscription: workspace_rename_subscription,
            _search_subscription: search_subscription,
            _config_subscriptions: Vec::new(),
            _welcome_subscription: welcome_subscription,
        }
    }

    pub(crate) fn connected(&self) -> bool {
        self.status == ServiceStatus::Connected
    }

    fn refresh_sessions(&mut self) {
        if self.sessions_loading {
            return;
        }
        self.sessions_loading = true;
        invoke(
            "list_sessions",
            json!({"cwd": Value::Null, "cursor": Value::Null}),
        );
    }

    fn activate_session(&mut self, session: SessionSummary) {
        self.sessions
            .retain(|known| !known.blank && known.id != session.id);
        self.sessions.insert(0, session.clone());
        self.active_session = Some(session.id);
    }

    fn apply_session_page(&mut self, reply: &Value) {
        let incoming = reply
            .get("sessions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|session| {
                Some(SessionSummary {
                    id: session.get("sessionId")?.as_str()?.to_string(),
                    cwd: session.get("cwd")?.as_str()?.to_string(),
                    title: session
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    blank: false,
                })
            });
        merge_session_page(&mut self.sessions, incoming, self.active_session.as_deref());
        let next = reply
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|cursor| !cursor.is_empty())
            .map(str::to_string);
        if let Some(cursor) = next {
            invoke(
                "list_sessions",
                json!({"cwd": Value::Null, "cursor": cursor}),
            );
        } else {
            self.sessions_loading = false;
        }
    }

    fn toggle_composer_menu(&mut self, menu: ComposerMenu) {
        self.composer_menu = if self.composer_menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
        self.show_workspace_menu = false;
        self.show_workspace_view_options = false;
    }

    fn start_service(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.running || self.service_starting || self.creating_session {
            return;
        }
        let workspace = self.workspace_editor.read(cx).value().trim().to_string();
        let service_path = self.service_editor.read(cx).value().trim().to_string();
        self.service_starting = true;
        self.creating_session = false;
        self.pending_action = None;
        self.status = ServiceStatus::Starting;
        self.error = None;
        self.active_session = None;
        self.page = WorkspacePage::Conversation;
        self.resumed_session = false;
        self.entries.clear();
        self.sessions.clear();
        self.sessions_loading = false;
        self.composer_menu = None;
        self.config_options.clear();
        self.rebuild_selectors(w, cx);
        invoke(
            "start_service",
            json!({"workspace": workspace, "servicePath": service_path}),
        );
        cx.notify();
    }

    pub(crate) fn open_page(&mut self, page: WorkspacePage, cx: &mut Context<Self>) {
        self.page = page;
        self.composer_menu = None;
        self.show_workspace_menu = false;
        self.show_workspace_view_options = false;
        self.show_account_menu = false;
        cx.notify();
    }

    fn new_session(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.connected() || self.running || self.creating_session {
            return;
        }
        self.page = WorkspacePage::Conversation;
        self.begin_action(PendingAction::New, w, cx);
    }

    fn start_assisted_session(
        &mut self,
        prompt: &'static str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.connected() || self.running || self.creating_session {
            return;
        }
        replace(&self.composer, prompt, w, cx);
        self.new_session(w, cx);
    }

    fn resume(&mut self, session: SessionSummary, w: &mut Window, cx: &mut Context<Self>) {
        if !self.connected() || self.running || self.creating_session {
            return;
        }
        self.page = WorkspacePage::Conversation;
        self.begin_action(PendingAction::Resume(session), w, cx);
    }

    fn begin_action(&mut self, action: PendingAction, w: &mut Window, cx: &mut Context<Self>) {
        self.creating_session = true;
        self.error = None;
        self.permission = None;
        self.usage = None;
        self.resumed_session = false;
        self.entries.clear();
        self.config_options.clear();
        self.composer_menu = None;
        self.rebuild_selectors(w, cx);
        if let Some(previous) = self.active_session.take() {
            self.pending_action = Some(action);
            invoke("close_session", json!({"sessionId": previous}));
        } else {
            self.perform_action(action);
        }
        cx.notify();
    }

    fn perform_action(&mut self, action: PendingAction) {
        match action {
            PendingAction::New => {
                self.requested_session_id = None;
                invoke("new_session", json!({"cwd": self.workspace}));
            }
            PendingAction::Resume(session) => {
                self.requested_session_id = Some(session.id.clone());
                self.workspace = session.cwd.clone();
                invoke(
                    "resume_session",
                    json!({"sessionId": session.id, "cwd": session.cwd}),
                );
            }
        }
    }

    fn send_prompt(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.connected() || self.running || self.creating_session {
            return;
        }
        let Some(session_id) = self.active_session.clone() else {
            self.error = Some("会话尚未就绪，请稍后重试。".to_string());
            cx.notify();
            return;
        };
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        replace(&self.composer, "", w, cx);
        self.page = WorkspacePage::Conversation;
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id && session.title.is_none())
        {
            session.title = Some(short_session_title(&text));
            session.blank = false;
        }
        self.push_local(EntryRole::User, text.clone());
        self.running = true;
        self.error = None;
        self.scroll.scroll_to_bottom();
        invoke("prompt", json!({"sessionId": session_id, "text": text}));
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(session_id) = self.active_session.clone() {
            invoke("cancel_session", json!({"sessionId": session_id}));
            cx.notify();
        }
    }

    fn set_config(&mut self, config_id: String, value: String) {
        if let Some(session_id) = self.active_session.clone() {
            invoke(
                "set_config_option",
                json!({"sessionId": session_id, "configId": config_id, "value": value}),
            );
        }
    }

    fn ensure_account_model(&mut self, cx: &mut Context<Self>) {
        let welcome = self.welcome.read(cx);
        let has_key = welcome
            .auth
            .as_ref()
            .and_then(|auth| auth.pointer("/apiKey/configured"))
            .and_then(Value::as_bool)
            == Some(true);
        if !welcome.signed_in() || has_key {
            return;
        }
        let Some(option) = self
            .config_options
            .iter()
            .find(|option| option.get("id").and_then(Value::as_str) == Some("model"))
        else {
            return;
        };
        let current = option.get("currentValue").and_then(Value::as_str);
        let account_value = option
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|group| group.get("group").and_then(Value::as_str) == Some("deepseek-account"))
            .and_then(|group| group.get("options"))
            .and_then(Value::as_array)
            .and_then(|options| options.first())
            .and_then(|option| option.get("value"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(value) = account_value
            && current != Some(value.as_str())
        {
            self.set_config("model".to_string(), value);
        }
    }

    fn respond_permission(&mut self, option_id: String, cx: &mut Context<Self>) {
        if let Some(permission) = self.permission.take() {
            invoke(
                "respond_permission",
                json!({"requestId": permission.request_id, "optionId": option_id}),
            );
            cx.notify();
        }
    }

    fn push_local(&mut self, role: EntryRole, text: String) {
        let key = format!("local-{}", self.next_local_id);
        self.next_local_id += 1;
        self.entries.push(ChatEntry::local(role, key, text));
    }

    fn handle_reply(
        &mut self,
        command: &'static str,
        result: Result<Value, String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            command,
            "start_auth"
                | "auth_status"
                | "auth_profile"
                | "auth_avatar"
                | "save_api_key"
                | "remove_api_key"
                | "start_sign_in"
                | "cancel_sign_in"
                | "sign_out"
                | "open_sign_in"
                | "open_account_page"
        ) {
            let was_configured = self.welcome.read(cx).configured();
            self.welcome.update(cx, |welcome, cx| {
                welcome.handle_reply(command, result, w, cx);
            });
            if !was_configured
                && self.welcome.read(cx).configured()
                && self.connected()
                && !self.running
                && !self.creating_session
                && self.entries.is_empty()
            {
                self.start_service(w, cx);
            }
            cx.notify();
            return;
        }
        match command {
            "start_service" => {
                self.service_starting = false;
                match result {
                    Ok(reply) => {
                        self.status = ServiceStatus::Connected;
                        self.workspace = string_at(&reply, "workspace");
                        self.service_path = string_at(&reply, "servicePath");
                        self.agent_name = reply
                            .pointer("/agent/agentInfo/name")
                            .and_then(Value::as_str)
                            .unwrap_or("deepseek-harness-acp")
                            .to_string();
                        replace(&self.workspace_editor, self.workspace.clone(), w, cx);
                        replace(&self.service_editor, self.service_path.clone(), w, cx);
                        self.error = None;
                        self.new_session(w, cx);
                    }
                    Err(error) => {
                        self.status = ServiceStatus::Error;
                        self.error = Some(error);
                    }
                }
            }
            "new_session" | "resume_session" => {
                self.creating_session = false;
                let requested_session_id = self.requested_session_id.take();
                match result {
                    Ok(reply) => {
                        let active_id = reply
                            .get("sessionId")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .or(requested_session_id);
                        if let Some(id) = active_id {
                            let existing = self
                                .sessions
                                .iter()
                                .find(|session| session.id == id)
                                .cloned();
                            let summary = existing.unwrap_or(SessionSummary {
                                id,
                                cwd: self.workspace.clone(),
                                title: None,
                                blank: command == "new_session",
                            });
                            self.activate_session(summary);
                        }
                        self.resumed_session = command == "resume_session";
                        self.config_options = reply
                            .get("configOptions")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        self.rebuild_selectors(w, cx);
                        self.ensure_account_model(cx);
                        self.refresh_sessions();
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            "close_session" => match result {
                Ok(_) => {
                    if let Some(action) = self.pending_action.take() {
                        self.perform_action(action);
                    }
                }
                Err(error) => {
                    self.pending_action = None;
                    self.creating_session = false;
                    self.error = Some(error);
                }
            },
            "list_sessions" => match result {
                Ok(reply) => self.apply_session_page(&reply),
                Err(error) => {
                    self.sessions_loading = false;
                    self.error = Some(format!("会话列表加载失败：{error}"));
                }
            },
            "list_workspace_files" => match result {
                Ok(reply) => self.apply_workspace_files(&reply),
                Err(error) if self.file_loading => self.fail_workspace_files(error),
                Err(_) => {}
            },
            "load_workspace_preferences" => match result {
                Ok(reply) => self.apply_workspace_preferences(reply),
                Err(error) => self.error = Some(error),
            },
            "save_workspace_preferences" => {
                if let Err(error) = result {
                    self.error = Some(error);
                }
            }
            "prompt" => {
                self.running = false;
                match result {
                    Ok(reply) => {
                        if reply.get("stopReason").and_then(Value::as_str) == Some("max_tokens") {
                            self.push_local(
                                EntryRole::System,
                                "输出达到 token 上限，可以继续发送消息。".to_string(),
                            );
                        }
                    }
                    Err(error) => {
                        let draft = self
                            .entries
                            .last()
                            .filter(|entry| {
                                entry.role == EntryRole::User && entry.external_id.is_none()
                            })
                            .map(|entry| entry.text.clone());
                        if let Some(draft) = draft {
                            self.entries.pop();
                            replace(&self.composer, draft, w, cx);
                        }
                        self.error = Some(format!("发送失败：{error}。草稿已保留，请重试。"));
                    }
                }
                self.scroll.scroll_to_bottom();
            }
            "set_config_option" => match result {
                Ok(reply) => {
                    self.config_options = reply
                        .get("configOptions")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    self.rebuild_selectors(w, cx);
                    self.ensure_account_model(cx);
                }
                Err(error) => self.error = Some(error),
            },
            "cancel_session" | "respond_permission" | "stop_service" | "open_start_tool" => {
                if let Err(error) = result {
                    self.error = Some(error);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn handle_event(&mut self, event: Value, w: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            event.get("kind").and_then(Value::as_str),
            Some("account" | "auth")
        ) {
            let was_configured = self.welcome.read(cx).configured();
            self.welcome
                .update(cx, |welcome, cx| welcome.handle_event(&event, cx));
            if !was_configured
                && self.welcome.read(cx).configured()
                && self.connected()
                && !self.running
                && !self.creating_session
                && self.entries.is_empty()
            {
                self.start_service(w, cx);
            }
            cx.notify();
            return;
        }
        match event.get("kind").and_then(Value::as_str) {
            Some("service") => match event.get("status").and_then(Value::as_str) {
                Some("starting") => self.status = ServiceStatus::Starting,
                Some("running") => self.status = ServiceStatus::Connected,
                Some("exited") => {
                    self.status = ServiceStatus::Error;
                    self.service_starting = false;
                    self.running = false;
                    self.error = event
                        .get("detail")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| Some("dsh-acp 已退出".to_string()));
                }
                _ => {}
            },
            Some("session_update") => {
                let session_id = event.get("sessionId").and_then(Value::as_str);
                if session_id != self.active_session.as_deref() {
                    return;
                }
                if let Some(update) = event.get("update") {
                    let effect = apply_update(&mut self.entries, update);
                    if effect.usage.is_some() {
                        self.usage = effect.usage;
                    }
                    if let Some(options) = effect.config_options {
                        self.config_options = options;
                        self.rebuild_selectors(w, cx);
                    }
                    self.scroll.scroll_to_bottom();
                }
            }
            Some("permission") => {
                let session_id = event.get("sessionId").and_then(Value::as_str);
                if session_id != self.active_session.as_deref() {
                    return;
                }
                let tool_call = event.get("toolCall").cloned().unwrap_or_else(|| json!({}));
                if let Some(call_id) = tool_call.get("toolCallId").and_then(Value::as_str)
                    && let Some(entry) = self.entries.iter_mut().find(|entry| {
                        entry.role == EntryRole::Tool
                            && entry.external_id.as_deref() == Some(call_id)
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
                self.scroll.scroll_to_bottom();
            }
            _ => {}
        }
        cx.notify();
    }

    fn rebuild_selectors(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self._config_subscriptions.clear();
        self.model_select = self.build_selector("model", "模型", w, cx);
        self.reasoning_select = self.build_selector("reasoning_effort", "推理强度", w, cx);
    }

    fn build_selector(
        &mut self,
        config_id: &str,
        label: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Entity<Select>> {
        let option = self
            .config_options
            .iter()
            .find(|option| option.get("id").and_then(Value::as_str) == Some(config_id))?;
        let choices = config_choices(option);
        if choices.is_empty() {
            return None;
        }
        let current = option
            .get("currentValue")
            .and_then(Value::as_str)
            .unwrap_or("");
        let select = cx.new(|cx| {
            Select::new(
                format!("{config_id}-selector"),
                label.to_string(),
                choices,
                w,
                cx,
            )
            .with_value(current)
        });
        let id = config_id.to_string();
        self._config_subscriptions.push(cx.subscribe(
            &select,
            move |this, _, event: &SelectionChanged, _cx| {
                if let Some(value) = &event.0 {
                    this.set_config(id.clone(), value.to_string());
                }
            },
        ));
        Some(select)
    }

    fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        let theme = Theme::global_mut(cx);
        let next = if theme.mode == ThemeMode::Dark {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        theme.set_mode(next);
        cx.refresh_windows();
    }
}

mod files;
mod panels;
mod permission_dialog;
mod start_panel;
mod workspace;
mod workspace_rows;
mod workspace_search;

impl Render for DshApp {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_workspace(w, cx)
    }
}

fn config_choices(option: &Value) -> Vec<PickerOption> {
    let mut choices = Vec::new();
    for item in option
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(group) = item.get("options").and_then(Value::as_array) {
            for choice in group {
                if let Some(choice) = picker_choice(choice) {
                    choices.push(choice);
                }
            }
        } else if let Some(choice) = picker_choice(item) {
            choices.push(choice);
        }
    }
    choices
}

fn picker_choice(value: &Value) -> Option<PickerOption> {
    Some(PickerOption::new(
        value.get("value")?.as_str()?.to_string(),
        value.get("name")?.as_str()?.to_string(),
    ))
}

fn string_at(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
