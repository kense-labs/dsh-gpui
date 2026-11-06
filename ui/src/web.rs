//! GPUI application, WASM lifecycle, and Tauri event handling.

use crate::{
    bridge,
    state::{ChatEntry, EntryRole, apply_update},
};
use gpui::{
    App, AsyncApp, Bounds, Context, FontWeight, ScrollHandle, Subscription, Window, WindowHandle,
    WindowOptions, div, prelude::*, px, size,
};
use kui::components::button::Button;
use kui::components::select::Select;
use kui::foundation::{Theme, ThemeMode};
use kui::primitives::control::UiLanguage;
use kui::primitives::editor::{EditEvent, Editor, EditorShortcutEvent, InputMode};
use kui::primitives::family::Paint;
use kui::primitives::glyph::{Glyph, icon};
use kui::primitives::ops_surface::{badge, column, editor, input, note, replace, row, title};
use kui::primitives::picker::{PickerOption, SelectionChanged};
use serde_json::{Value, json};
use std::cell::RefCell;
use wasm_bindgen::prelude::*;

struct Host {
    app: AsyncApp,
    window: WindowHandle<DshApp>,
    _event_listener: Closure<dyn FnMut(String)>,
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
        let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
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
        HOST.set(Some(Host {
            app: cx.to_async(),
            window,
            _event_listener: listener,
        }));
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
            invoke("start_service", json!({}));
        });
    });
}

fn with_host<R>(operation: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOST.with_borrow_mut(|host| host.as_mut().map(operation))
}

fn invoke(command: &'static str, args: Value) {
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
enum ServiceStatus {
    Starting,
    Connected,
    Error,
}

#[derive(Clone)]
struct SessionSummary {
    id: String,
    cwd: String,
}

#[derive(Clone)]
enum PendingAction {
    New,
    Resume(SessionSummary),
}

struct PermissionRequest {
    request_id: Value,
    tool_call: Value,
    options: Vec<(String, String)>,
}

struct DshApp {
    status: ServiceStatus,
    agent_name: String,
    workspace: String,
    service_path: String,
    error: Option<String>,
    sessions: Vec<SessionSummary>,
    active_session: Option<String>,
    requested_session_id: Option<String>,
    pending_action: Option<PendingAction>,
    creating_session: bool,
    running: bool,
    entries: Vec<ChatEntry>,
    usage: Option<(u64, u64)>,
    permission: Option<PermissionRequest>,
    config_options: Vec<Value>,
    model_select: Option<gpui::Entity<Select>>,
    reasoning_select: Option<gpui::Entity<Select>>,
    composer: gpui::Entity<Editor>,
    workspace_editor: gpui::Entity<Editor>,
    service_editor: gpui::Entity<Editor>,
    scroll: ScrollHandle,
    show_settings: bool,
    next_local_id: u64,
    _composer_subscriptions: Vec<Subscription>,
    _config_subscriptions: Vec<Subscription>,
}

impl DshApp {
    fn new(w: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = editor(
            "描述任务，⌘/Ctrl + Enter 发送…".into(),
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
                this.send_prompt(w, cx);
            }
        });
        let workspace_editor = editor(
            "工作区绝对路径（留空使用当前目录）".into(),
            InputMode::Single,
            w,
            cx,
        );
        let service_editor = editor(
            "dsh-acp 路径（留空自动寻找 ./dsh-acp）".into(),
            InputMode::Single,
            w,
            cx,
        );
        Self {
            status: ServiceStatus::Starting,
            agent_name: "deepseek-harness-acp".to_string(),
            workspace: String::new(),
            service_path: String::new(),
            error: None,
            sessions: Vec::new(),
            active_session: None,
            requested_session_id: None,
            pending_action: None,
            creating_session: false,
            running: false,
            entries: Vec::new(),
            usage: None,
            permission: None,
            config_options: Vec::new(),
            model_select: None,
            reasoning_select: None,
            composer,
            workspace_editor,
            service_editor,
            scroll: ScrollHandle::new(),
            show_settings: false,
            next_local_id: 0,
            _composer_subscriptions: vec![submit, submit_plain],
            _config_subscriptions: Vec::new(),
        }
    }

    fn connected(&self) -> bool {
        self.status == ServiceStatus::Connected
    }

    fn start_service(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let workspace = self.workspace_editor.read(cx).value().trim().to_string();
        let service_path = self.service_editor.read(cx).value().trim().to_string();
        self.status = ServiceStatus::Starting;
        self.error = None;
        self.active_session = None;
        self.entries.clear();
        self.sessions.clear();
        self.config_options.clear();
        self.rebuild_selectors(w, cx);
        invoke(
            "start_service",
            json!({"workspace": workspace, "servicePath": service_path}),
        );
        cx.notify();
    }

    fn new_session(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.connected() || self.running || self.creating_session {
            return;
        }
        self.begin_action(PendingAction::New, w, cx);
    }

    fn resume(&mut self, session: SessionSummary, w: &mut Window, cx: &mut Context<Self>) {
        if !self.connected() || self.running || self.creating_session {
            return;
        }
        self.begin_action(PendingAction::Resume(session), w, cx);
    }

    fn begin_action(&mut self, action: PendingAction, w: &mut Window, cx: &mut Context<Self>) {
        self.creating_session = true;
        self.error = None;
        self.permission = None;
        self.usage = None;
        self.entries.clear();
        self.config_options.clear();
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
        self.push_local(EntryRole::User, text.clone());
        self.running = true;
        self.error = None;
        self.scroll.scroll_to_bottom();
        invoke("prompt", json!({"sessionId": session_id, "text": text}));
        cx.notify();
    }

    fn send_starter(&mut self, text: &str, w: &mut Window, cx: &mut Context<Self>) {
        replace(&self.composer, text, w, cx);
        self.send_prompt(w, cx);
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
        match command {
            "start_service" => match result {
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
                    invoke("list_sessions", json!({"cwd": Value::Null}));
                }
                Err(error) => {
                    self.status = ServiceStatus::Error;
                    self.error = Some(error);
                }
            },
            "new_session" | "resume_session" => {
                self.creating_session = false;
                let requested_session_id = self.requested_session_id.take();
                match result {
                    Ok(reply) => {
                        self.active_session = reply
                            .get("sessionId")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .or(requested_session_id);
                        if command == "resume_session" {
                            self.push_local(
                                EntryRole::System,
                                "会话已恢复。ACP 不回放历史消息；新消息将从这里继续。".to_string(),
                            );
                        }
                        self.config_options = reply
                            .get("configOptions")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        self.rebuild_selectors(w, cx);
                        invoke("list_sessions", json!({"cwd": Value::Null}));
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
            "list_sessions" => {
                if let Ok(reply) = result {
                    self.sessions = reply
                        .get("sessions")
                        .and_then(Value::as_array)
                        .map(|sessions| {
                            sessions
                                .iter()
                                .filter_map(|session| {
                                    Some(SessionSummary {
                                        id: session.get("sessionId")?.as_str()?.to_string(),
                                        cwd: session.get("cwd")?.as_str()?.to_string(),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
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
                    Err(error) => self.error = Some(error),
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
                }
                Err(error) => self.error = Some(error),
            },
            "cancel_session" | "respond_permission" | "stop_service" => {
                if let Err(error) = result {
                    self.error = Some(error);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn handle_event(&mut self, event: Value, w: &mut Window, cx: &mut Context<Self>) {
        match event.get("kind").and_then(Value::as_str) {
            Some("service") => match event.get("status").and_then(Value::as_str) {
                Some("starting") => self.status = ServiceStatus::Starting,
                Some("running") => self.status = ServiceStatus::Connected,
                Some("exited") => {
                    self.status = ServiceStatus::Error;
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

    fn render_sidebar(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = Paint::get(cx);
        let mut list = column().gap(px(4.));
        for session in self.sessions.clone() {
            let selected = self.active_session.as_deref() == Some(session.id.as_str());
            let short = session.id.get(..8).unwrap_or(&session.id).to_string();
            let for_click = session.clone();
            list = list.child(
                column()
                    .id(gpui::ElementId::Name(session.id.clone().into()))
                    .w_full()
                    .gap(px(3.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .bg(if selected { p.hover } else { p.panel })
                    .hover(|style| style.bg(p.hover))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("会话 {short}")),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(p.faint)
                            .truncate()
                            .child(session.cwd.clone()),
                    )
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.resume(for_click.clone(), w, cx);
                    })),
            );
        }
        column()
            .w(px(260.))
            .h_full()
            .flex_none()
            .gap(px(14.))
            .p(px(16.))
            .bg(p.panel)
            .border_r_1()
            .border_color(p.line)
            .child(
                row().justify_between().child(title("会话")).child(
                    Button::new("refresh-sessions")
                        .icon_only(Glyph::Redo)
                        .label("刷新会话")
                        .ghost()
                        .disabled(!self.connected())
                        .on_click(cx.listener(|_, _, _, _| {
                            invoke("list_sessions", json!({"cwd": Value::Null}));
                        }))
                        .into_themed(cx),
                ),
            )
            .child(
                Button::new("new-session")
                    .label("新建会话")
                    .with_icon(Glyph::Plus, false)
                    .disabled(!self.connected() || self.running || self.creating_session)
                    .on_click(cx.listener(|this, _, w, cx| this.new_session(w, cx)))
                    .into_themed(cx),
            )
            .child(
                div()
                    .id("session-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(if self.sessions.is_empty() {
                        note("其他持久会话将显示在这里。", cx)
                    } else {
                        note(format!("{} 个可恢复会话", self.sessions.len()), cx)
                    })
                    .child(list),
            )
            .child(
                column()
                    .gap(px(4.))
                    .pt(px(12.))
                    .border_t_1()
                    .border_color(p.line)
                    .child(note("工作区", cx))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(p.secondary)
                            .truncate()
                            .child(if self.workspace.is_empty() {
                                "正在解析…".to_string()
                            } else {
                                self.workspace.clone()
                            }),
                    ),
            )
    }

    fn render_header(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let p = Paint::get(cx);
        let (status_text, status_color) = match self.status {
            ServiceStatus::Starting => ("正在连接", p.secondary),
            ServiceStatus::Connected if self.running => ("正在运行", p.success),
            ServiceStatus::Connected => ("已连接", p.success),
            ServiceStatus::Error => ("连接异常", p.danger),
        };
        row()
            .w_full()
            .h(px(68.))
            .flex_none()
            .px(px(22.))
            .gap(px(12.))
            .border_b_1()
            .border_color(p.line)
            .child(
                column()
                    .gap(px(1.))
                    .child(
                        div()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("DeepSeek Harness"),
                    )
                    .child(note(self.agent_name.clone(), cx)),
            )
            .child(div().size(px(7.)).rounded_full().bg(status_color))
            .child(note(status_text, cx))
            .child(div().flex_1())
            .when_some(self.model_select.clone(), |element, select| {
                element.child(div().w(px(220.)).child(select))
            })
            .when_some(self.reasoning_select.clone(), |element, select| {
                element.child(div().w(px(150.)).child(select))
            })
            .child(
                Button::new("toggle-settings")
                    .icon_only(Glyph::Settings)
                    .label("连接设置")
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_settings = !this.show_settings;
                        cx.notify();
                    }))
                    .into_themed(cx),
            )
            .child(
                Button::new("toggle-theme")
                    .icon_only(if Theme::global(cx).mode == ThemeMode::Dark {
                        Glyph::Sun
                    } else {
                        Glyph::Moon
                    })
                    .label("切换主题")
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_theme(cx)))
                    .into_themed(cx),
            )
    }

    fn render_settings(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = Paint::get(cx);
        row()
            .w_full()
            .p(px(18.))
            .gap(px(14.))
            .bg(p.tray)
            .border_b_1()
            .border_color(p.line)
            .child(
                column()
                    .flex_1()
                    .gap(px(6.))
                    .child(note("工作区", cx))
                    .child(input(&self.workspace_editor, cx)),
            )
            .child(
                column()
                    .flex_1()
                    .gap(px(6.))
                    .child(note("dsh-acp 服务目录", cx))
                    .child(input(&self.service_editor, cx)),
            )
            .child(
                Button::new("connect-service")
                    .label("重新连接")
                    .loading(self.status == ServiceStatus::Starting)
                    .disabled(self.running)
                    .on_click(cx.listener(|this, _, w, cx| this.start_service(w, cx)))
                    .into_themed(cx),
            )
    }

    fn render_conversation(
        &mut self,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        let mut content = column()
            .w_full()
            .max_w(px(860.))
            .mx_auto()
            .gap(px(16.))
            .py(px(24.));
        if self.entries.is_empty() {
            let mut starters = column().gap(px(10.)).w_full().max_w(px(520.)).mx_auto();
            for (index, prompt) in [
                "分析当前项目结构，并说明最重要的三个模块。",
                "检查这个工作区的代码，提出可执行的改进建议。",
                "帮我编写一个简短、可验证的实现计划。",
            ]
            .into_iter()
            .enumerate()
            {
                starters = starters.child(
                    div()
                        .id(("starter", index))
                        .w_full()
                        .px(px(16.))
                        .py(px(12.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(p.line)
                        .bg(p.panel)
                        .cursor_pointer()
                        .hover(|style| style.border_color(p.faint).bg(p.hover))
                        .child(prompt)
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.send_starter(prompt, w, cx);
                        })),
                );
            }
            content = content.child(
                column()
                    .items_center()
                    .gap(px(16.))
                    .pt(px(54.))
                    .pb(px(30.))
                    .child(icon(Glyph::Rocket, 34., p.secondary))
                    .child(
                        div()
                            .text_size(px(24.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("今天想完成什么？"),
                    )
                    .child(note(
                        "由 DeepSeek Harness 驱动，通过 ACP 执行真实任务。",
                        cx,
                    ))
                    .child(starters),
            );
        } else {
            for entry in &self.entries {
                content = content.child(render_entry(entry, cx));
            }
            if self.running {
                content =
                    content.child(note("Harness 正在执行；消息和工具结果将在提交后显示。", cx));
            }
        }
        div()
            .id("conversation")
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(24.))
            .child(content)
            .child(div().h(px(16.)))
    }

    fn render_permission(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let permission = self.permission.as_ref()?;
        let p = Paint::get(cx);
        let call = permission
            .tool_call
            .get("toolCallId")
            .and_then(Value::as_str)
            .unwrap_or("工具调用");
        let mut actions = row().gap(px(8.));
        for (id, name) in &permission.options {
            let option_id = id.clone();
            let button = if id == "allow-once" {
                Button::new(gpui::ElementId::Name(format!("permission-{id}").into())).primary()
            } else {
                Button::new(gpui::ElementId::Name(format!("permission-{id}").into())).secondary()
            };
            actions = actions.child(
                button
                    .label(name.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.respond_permission(option_id.clone(), cx);
                    }))
                    .into_themed(cx),
            );
        }
        Some(
            row()
                .w_full()
                .max_w(px(860.))
                .mx_auto()
                .p(px(14.))
                .rounded(px(8.))
                .border_1()
                .border_color(p.danger.opacity(0.5))
                .bg(p.panel)
                .child(icon(Glyph::Alert, 20., p.danger))
                .child(
                    column()
                        .flex_1()
                        .gap(px(2.))
                        .child(title("需要工具执行权限"))
                        .child(note(format!("工具调用 {call} 正在等待你的决定。"), cx)),
                )
                .child(actions),
        )
    }

    fn render_composer(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let p = Paint::get(cx);
        let usage = self.usage.map(|(used, size)| {
            if size > 0 {
                format!(
                    "上下文 {} / {}（{:.0}%）",
                    used,
                    size,
                    used as f64 / size as f64 * 100.
                )
            } else {
                format!("上下文 {used}")
            }
        });
        column()
            .w_full()
            .flex_none()
            .gap(px(10.))
            .px(px(24.))
            .py(px(16.))
            .border_t_1()
            .border_color(p.line)
            .bg(p.stage)
            .children(self.render_permission(cx))
            .when_some(self.error.clone(), |element, error| {
                element.child(
                    row()
                        .w_full()
                        .max_w(px(860.))
                        .mx_auto()
                        .child(icon(Glyph::Alert, 16., p.danger))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.))
                                .text_color(p.danger)
                                .child(error),
                        )
                        .child(
                            Button::new("dismiss-error")
                                .icon_only(Glyph::Close)
                                .label("关闭错误")
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.error = None;
                                    cx.notify();
                                }))
                                .into_themed(cx),
                        ),
                )
            })
            .child(
                row()
                    .w_full()
                    .max_w(px(860.))
                    .mx_auto()
                    .gap(px(10.))
                    .child(div().flex_1().child(input(&self.composer, cx)))
                    .child(if self.running {
                        Button::new("cancel-prompt")
                            .label("停止")
                            .danger()
                            .with_icon(Glyph::Pause, false)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
                            .into_themed(cx)
                    } else {
                        Button::new("send-prompt")
                            .label("发送")
                            .with_icon(Glyph::Send, false)
                            .disabled(!self.connected() || self.creating_session)
                            .on_click(cx.listener(|this, _, w, cx| this.send_prompt(w, cx)))
                            .into_themed(cx)
                    }),
            )
            .child(
                row()
                    .w_full()
                    .max_w(px(860.))
                    .mx_auto()
                    .justify_between()
                    .child(note(
                        usage.unwrap_or_else(|| "ACP · committed updates".to_string()),
                        cx,
                    ))
                    .child(note("⌘ / Ctrl + Enter 发送", cx)),
            )
    }
}

impl Render for DshApp {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::get(cx);
        let mut main = column()
            .flex_1()
            .h_full()
            .min_w_0()
            .gap_0()
            .bg(p.stage)
            .child(self.render_header(cx));
        if self.show_settings {
            main = main.child(self.render_settings(w, cx));
        }
        main = main
            .child(self.render_conversation(w, cx))
            .child(self.render_composer(cx));
        row()
            .size_full()
            .gap_0()
            .bg(p.stage)
            .text_color(p.ink)
            .child(self.render_sidebar(w, cx))
            .child(main)
    }
}

fn render_entry(entry: &ChatEntry, cx: &App) -> gpui::Div {
    let p = Paint::get(cx);
    match entry.role {
        EntryRole::User => row().w_full().justify_end().child(
            div()
                .max_w(gpui::relative(0.82))
                .px(px(16.))
                .py(px(11.))
                .rounded(px(8.))
                .bg(p.bright)
                .text_color(p.inverse)
                .child(entry.text.clone()),
        ),
        EntryRole::Assistant => column()
            .w_full()
            .gap(px(8.))
            .child(
                row()
                    .child(icon(Glyph::Bulb, 16., p.success))
                    .child(title("DeepSeek")),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .child(entry.text.clone()),
            ),
        EntryRole::Thought => column()
            .w_full()
            .gap(px(6.))
            .p(px(14.))
            .rounded(px(8.))
            .bg(p.tray)
            .border_l_2()
            .border_color(p.faint)
            .child(note("思考过程", cx))
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(19.))
                    .text_color(p.secondary)
                    .child(entry.text.clone()),
            ),
        EntryRole::Tool => {
            let status = match entry.status.as_str() {
                "in_progress" => "运行中",
                "completed" => "已完成",
                "failed" => "失败",
                "approval" => "等待授权",
                _ => &entry.status,
            };
            let mut detail = column().gap(px(6.));
            if !entry.input.is_empty() {
                detail = detail.child(note("输入", cx)).child(
                    div()
                        .text_size(px(11.))
                        .text_color(p.secondary)
                        .child(entry.input.clone()),
                );
            }
            if !entry.text.is_empty() {
                detail = detail.child(note("输出", cx)).child(
                    div()
                        .text_size(px(11.))
                        .text_color(p.secondary)
                        .child(entry.text.clone()),
                );
            }
            column()
                .w_full()
                .gap(px(8.))
                .p(px(14.))
                .rounded(px(8.))
                .border_1()
                .border_color(p.line)
                .bg(p.panel)
                .child(
                    row()
                        .child(icon(Glyph::Settings, 15., p.secondary))
                        .child(title(if entry.title.is_empty() {
                            "工具调用"
                        } else {
                            &entry.title
                        }))
                        .child(div().flex_1())
                        .child(badge(status, cx)),
                )
                .child(detail)
        }
        EntryRole::System => row()
            .w_full()
            .justify_center()
            .child(note(entry.text.clone(), cx)),
        EntryRole::Error => row()
            .w_full()
            .child(icon(Glyph::Alert, 16., p.danger))
            .child(div().text_color(p.danger).child(entry.text.clone())),
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
