//! GPUI application, WASM lifecycle, and Tauri event handling.

use crate::{
    anim,
    anim::{FISH_SWIM, menu_in, modal_in, pending_dot},
    bridge,
    commands::{
        CommandCatalog, CommandSpec, canonical_host_command, client_side_command,
        resolve_command_token, visible_commands,
    },
    files::{
        FileListing, FileToken, WorkspaceFile, active_file_token, file_mention, replace_token,
    },
    markdown::{Block, InlinePart, code_spans, inline_parts, parse_markdown},
    right_dock::{
        DockFrame, DockPaneId, DockSlide, DockSurface, RightPanelTab, TerminalCheckpoint,
    },
    sessions::{
        PendingInteraction, SessionActivity, SessionIndicator, SessionSummary, merge_session_page,
        short_session_title,
    },
    state::{ChatEntry, EntryRole, apply_update, prompt_failure_message},
    tokens::{dsh, elevation_prominent, elevation_soft},
    welcome::{Welcome, WelcomeEvent},
};
use gpui::{
    AnimationExt, App, AsyncApp, Context, FontWeight, ScrollHandle, Subscription, Window,
    WindowHandle, WindowOptions, div, prelude::*, px,
};
use kui::components::button::Button;
use kui::components::select::Select;
use kui::foundation::{Theme, ThemeMode};
use kui::primitives::control::UiLanguage;
use kui::primitives::editor::{EditEvent, Editor, EditorShortcutEvent, InputMode};
use kui::primitives::glyph::{Glyph, icon};
use kui::primitives::ops_surface::{column, editor, input, note, replace, row};
use kui::primitives::picker::{PickerOption, SelectionChanged};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

struct Host {
    app: AsyncApp,
    window: WindowHandle<DshApp>,
    _event_listener: Closure<dyn FnMut(String)>,
    _key_listener: Closure<dyn FnMut(String, String, u8, u8, bool, bool) -> bool>,
    _file_listener: Closure<dyn FnMut(String)>,
    _dock_cancel_listener: Closure<dyn FnMut()>,
    _browser_selection_listener: Closure<dyn FnMut(String)>,
    _html_focus_listener: Closure<dyn FnMut(String, u32)>,
    _transparency_listener: Closure<dyn FnMut()>,
    _motion_listener: Closure<dyn FnMut(bool)>,
    _runtime_error_listener: Closure<dyn FnMut(String)>,
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
        cx.text_system()
            .add_fonts(vec![
                std::borrow::Cow::Borrowed(include_bytes!("../assets/Montserrat-Light.ttf")),
                std::borrow::Cow::Borrowed(include_bytes!("../assets/Montserrat-Regular.ttf")),
                std::borrow::Cow::Borrowed(include_bytes!("../assets/Montserrat-Medium.ttf")),
            ])
            .expect("load the official Montserrat brand font");
        cx.set_global(UiLanguage::Chinese);
        kui::init(cx);
        cx.set_reduce_motion(bridge::prefers_reduced_motion());
        Theme::global_mut(cx).set_mode(ThemeMode::Dark);
        if bridge::has_mac_window_material() {
            cx.set_text_rendering_mode(gpui::TextRenderingMode::Grayscale);
        }
        let window = cx
            .open_window(
                WindowOptions {
                    inactive_frame_interval: None,
                    ..Default::default()
                },
                |w, cx| cx.new(|cx| DshApp::new(w, cx)),
            )
            .expect("open the KWork GPUI window");
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
        let key_listener = Closure::<dyn FnMut(String, String, u8, u8, bool, bool) -> bool>::new(
            |combo: String, code: String, modifiers, phase, repeat, composing| {
                with_host(|host| {
                    host.app.update(|cx| {
                        host.window
                            .update(cx, |app, w, cx| {
                                app.handle_shortcut_input(
                                    &combo, &code, modifiers, phase, repeat, composing, w, cx,
                                )
                            })
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
            },
        );
        let file_listener = Closure::<dyn FnMut(String)>::new(|payload: String| {
            with_host(|host| {
                host.app.update(|cx| {
                    let _ = host.window.update(cx, |app, _, cx| {
                        app.handle_file_event(&payload, cx);
                    });
                });
            });
        });
        let dock_cancel_listener = Closure::<dyn FnMut()>::new(|| {
            with_host(|host| {
                host.app.update(|cx| {
                    let _ = host.window.update(cx, |app, _, cx| {
                        app.cancel_dock_tab_drag(cx);
                        app.cancel_float_drag(cx);
                        for tab in app.sidebar.documents.values() {
                            tab.selection.cancel_drag();
                        }
                    });
                });
            });
        });
        let browser_selection_listener = Closure::<dyn FnMut(String)>::new(|prefix: String| {
            with_host(|host| {
                host.app.update(|cx| {
                    let _ = host.window.update(cx, |app, w, cx| {
                        app.advance_browser_selection(&prefix, w, cx);
                    });
                });
            });
        });
        let html_focus_listener =
            Closure::<dyn FnMut(String, u32)>::new(|key: String, revision| {
                with_host(|host| {
                    host.app.update(|cx| {
                        let _ = host.window.update(cx, |app, w, cx| {
                            app.html_preview_focus_changed(key.parse().ok(), revision, w, cx)
                        });
                    });
                });
            });
        let transparency_listener = Closure::<dyn FnMut()>::new(|| {
            with_host(|host| host.app.update(|cx| cx.refresh_windows()));
        });
        let motion_listener = Closure::<dyn FnMut(bool)>::new(|reduce| {
            with_host(|host| host.app.update(|cx| cx.set_reduce_motion(reduce)));
        });
        let runtime_error_listener = Closure::<dyn FnMut(String)>::new(|message: String| {
            with_host(|host| {
                host.app.update(|cx| {
                    let _ = host.window.update(cx, |app, _, cx| {
                        app.push_toast(message.clone(), cx);
                    });
                });
            });
        });
        HOST.set(Some(Host {
            app: cx.to_async(),
            window,
            _event_listener: listener,
            _key_listener: key_listener,
            _file_listener: file_listener,
            _dock_cancel_listener: dock_cancel_listener,
            _browser_selection_listener: browser_selection_listener,
            _html_focus_listener: html_focus_listener,
            _transparency_listener: transparency_listener,
            _motion_listener: motion_listener,
            _runtime_error_listener: runtime_error_listener,
        }));
        with_host(|host| {
            bridge::listen_keys(host._key_listener.as_ref().unchecked_ref());
            bridge::listen_files(host._file_listener.as_ref().unchecked_ref());
            bridge::listen_dock_cancel(host._dock_cancel_listener.as_ref().unchecked_ref());
            bridge::listen_browser_selection(
                host._browser_selection_listener.as_ref().unchecked_ref(),
            );
            document_html::listen_focus(host._html_focus_listener.as_ref().unchecked_ref());
            bridge::listen_runtime_error(host._runtime_error_listener.as_ref().unchecked_ref());
            bridge::watch_reduced_motion(host._motion_listener.as_ref().unchecked_ref());
            if bridge::has_mac_window_material() {
                bridge::watch_transparency(host._transparency_listener.as_ref().unchecked_ref());
            }
        });
        cx.activate(true);
        if bridge::is_mac() {
            wasm_bindgen_futures::spawn_local(async {
                if let Some(bytes) = bridge::terminal_font().await {
                    with_host(|host| {
                        host.app.update(|cx| {
                            if cx.text_system().add_fonts(vec![bytes.into()]).is_ok() {
                                cx.refresh_windows();
                            }
                        });
                    });
                }
            });
        }
        wasm_bindgen_futures::spawn_local(async {
            let registered = with_host(|host| {
                wasm_bindgen_futures::JsFuture::from(bridge::listen(
                    host._event_listener.as_ref().unchecked_ref(),
                ))
            });
            if let Some(registered) = registered {
                let _ = registered.await;
            }
            invoke("browser_shell_generation", json!({}));
            invoke("start_auth", json!({}));
            invoke("load_workspace_preferences", json!({}));
            invoke("load_shortcuts", json!({}));
            if bridge::is_mac() {
                invoke("workspace_applications", json!({}));
            }
            invoke("load_developer_tools", json!({}));
            invoke("start_service", json!({}));
        });
    });
}

fn with_host<R>(operation: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOST.with_borrow_mut(|host| host.as_mut().map(operation))
}

pub(crate) fn invoke(command: &'static str, args: Value) {
    invoke_with_reply(command, command, args);
}

fn invoke_with_reply(command: &'static str, reply: &'static str, args: Value) {
    let owner = args["sessionId"].as_str().map(str::to_owned);
    wasm_bindgen_futures::spawn_local(async move {
        let result = bridge::invoke(command, args).await;
        with_host(|host| {
            host.app.update(|cx| {
                let _ = host.window.update(cx, |app, w, cx| {
                    app.handle_reply(reply, owner.as_deref(), result, w, cx);
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
    Credentials,
    General,
    Appearance,
    Model,
    Providers,
    Plugins,
    AgentPresets,
    SessionLog,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkDetailsMode {
    Compact,
    Standard,
    Detailed,
    Verbose,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerMenu {
    Model,
    ModelChoices,
    ReasoningChoices,
    Permission,
    PermissionChoices,
    CommandModels,
    Commands,
    Files,
}

impl ComposerMenu {
    fn is_model(self) -> bool {
        matches!(
            self,
            Self::Model | Self::ModelChoices | Self::ReasoningChoices
        )
    }

    fn is_card_overlay(self) -> bool {
        self.is_command_picker() || matches!(self, Self::Commands | Self::Files)
    }

    fn is_command_picker(self) -> bool {
        matches!(self, Self::PermissionChoices | Self::CommandModels)
    }
}

struct SidebarDrawerFocus {
    scope: gpui::FocusHandle,
    opener: gpui::FocusHandle,
    close: gpui::FocusHandle,
    more: gpui::FocusHandle,
}

impl SidebarDrawerFocus {
    fn new(cx: &mut Context<DshApp>) -> Self {
        Self {
            scope: cx.focus_handle(),
            opener: cx.focus_handle(),
            close: cx.focus_handle(),
            more: cx.focus_handle(),
        }
    }
}

type ComposerAnchor = Rc<Cell<Option<gpui::Bounds<gpui::Pixels>>>>;

/// 官方 StatsPills DialogPill：Activity / Usage 点击展开详情。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatsDialog {
    Activity,
    Usage,
}

enum PendingAction {
    New {
        cwd: String,
        draft: Option<&'static str>,
    },
    Resume(SessionSummary),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ForkOrigin {
    Answer(u64),
    Sidebar,
    Shortcut,
}

struct SessionRename {
    id: String,
    restore_focus: Option<gpui::FocusHandle>,
    close: gpui::FocusHandle,
    cancel: gpui::FocusHandle,
    confirm: gpui::FocusHandle,
    pending: bool,
    error: Option<String>,
}

/// One embedded PTY tab in the right dock.
pub(crate) struct TerminalTab {
    id: String,
    title: Option<String>,
    requested_shell: Option<String>,
    connection: TerminalConnection,
    sequence: u64,
    pending: Vec<terminal::TerminalUpdate>,
    screen: gpui::Entity<terminal::TerminalScreen>,
}

enum TerminalConnection {
    Opening {
        create: bool,
        reading_shells: bool,
    },
    Connected,
    Missing,
    Exited {
        code: Option<u32>,
    },
    Failed {
        error: String,
        retry_create: bool,
        has_screen: bool,
    },
}

impl TerminalTab {
    fn apply_update(&mut self, update: terminal::TerminalUpdate, cx: &mut App) -> bool {
        if update.sequence <= self.sequence {
            return false;
        }
        self.sequence = update.sequence;
        match update.operation {
            terminal::TerminalOperation::Output { data } => {
                self.screen.update(cx, |screen, cx| screen.feed(&data, cx));
            }
            terminal::TerminalOperation::Resize { cols, rows } => {
                self.screen
                    .update(cx, |screen, cx| screen.observe_grid(cols, rows, cx));
            }
            terminal::TerminalOperation::Exit { code } => self.end(code, cx),
            terminal::TerminalOperation::Rename { title } => {
                self.title = Some(title);
                return true;
            }
        }
        false
    }

    fn end(&mut self, code: Option<u32>, cx: &mut App) {
        self.connection = TerminalConnection::Exited { code };
        self.screen.update(cx, |screen, cx| screen.unbind(cx));
    }
}

#[derive(Default)]
struct SessionSidebar {
    surface: DockSurface,
    persisted: DockSurface,
    workspace: String,
    file_trees: HashMap<DockPaneId, dock_files::DockFileTree>,
    documents: HashMap<u64, dock_documents::DocumentTab>,
    plans: HashMap<u64, plan::PlanPreview>,
    tab_strips: HashMap<DockPaneId, dock_strip::DockStrip>,
    terminals: HashMap<u64, TerminalTab>,
    browsers: HashMap<u64, browser::BrowserTab>,
}

impl SessionSidebar {
    fn persist(&mut self, session: &str) {
        if self.surface != self.persisted
            && let Ok(json) = serde_json::to_string(&self.surface)
        {
            bridge::write_sidebar(session, &json);
            self.persisted = self.surface.clone();
        }
    }
}

/// Composer 草稿附件（对齐官方 attachment rail：64px 高卡片）。
pub(crate) struct DraftAttachment {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) kind: AttachmentKind,
    pub(crate) mime: String,
    /// data URL 预览；图片发送时从此提取 ACP `image` block 的 base64。
    pub(crate) preview: Option<String>,
}

/// 排队发送：Host Inbox 里的待处理消息（官方 QueueDock 的一行）。
///
/// 消息一旦提交就归 Host 所有：`item_id` 是 Inbox 里的稳定身份，`mode` 说明
/// 它等待新回合（排队）还是下一次步骤（插话）。客户端不再自己保存正文，
/// 只按 Host 的待处理列表渲染，这样关闭应用也不会丢。
#[derive(Clone)]
pub(crate) struct QueuedPrompt {
    pub(crate) item_id: String,
    pub(crate) mode: crate::submit::SubmitMode,
    pub(crate) display: String,
    /// 加入队列时的本地序号：用于忽略比它更早请求的 Host 快照。
    pub(crate) added_at: u64,
}

/// 已经组好 ACP prompt blocks、可以直接交给 `session/prompt` 的一条消息。
#[derive(Clone)]
pub(crate) struct OutgoingPrompt {
    pub(crate) display: String,
    pub(crate) prompt: Vec<Value>,
}

/// 一次会话指令提交：成功后才清理对应草稿与附件。
pub(super) struct CommandSubmission {
    draft: String,
    attachment_ids: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachmentKind {
    Image,
    File,
}

pub(crate) struct PermissionRequest {
    pub(crate) request_id: Value,
    pub(crate) tool_call: Value,
    pub(crate) options: Vec<(String, String)>,
}

/// 官方轮次结束状态：分隔线文案由它决定。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnEnd {
    Completed,
    Stopped,
    Failed,
    MaxTokens,
}

impl TurnEnd {
    pub(crate) fn label(self) -> &'static str {
        match self {
            TurnEnd::Completed => "已完成",
            TurnEnd::Stopped => "已停止",
            TurnEnd::Failed => "处理失败",
            // Official shows the normal completed label; the warning row above
            // carries the output-token-cap message.
            TurnEnd::MaxTokens => "已完成",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TurnSummary {
    pub(crate) end: TurnEnd,
    /// 官方页脚不显示耗时；保留读数供统计与无障碍描述使用。
    #[allow(dead_code)]
    pub(crate) seconds: f32,
    /// 本轮新增的上下文消耗（token 数；usage 无数据时为 None）。
    pub(crate) used: Option<u64>,
}

/// One picture shown in the full-screen viewer (official 查看大图 dialog).
#[derive(Clone)]
pub(crate) struct ImageViewer {
    pub(crate) data_url: String,
    pub(crate) name: String,
}

#[derive(Clone)]
pub(crate) struct Toast {
    pub(crate) message: String,
    /// 创建时刻；3s 后开始 1s 淡出，4s 移除。
    pub(crate) at: f64,
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
    conversation: Conversation,
    parked_conversations: HashMap<String, Conversation>,
    session_activity: HashMap<String, SessionActivity>,
    running_subagents: HashMap<String, String>,
    /// Read-only child transcript currently open, and the parent to return to.
    subagent_view: Option<subagent_view::SubagentView>,
    /// Blank session opened by a previous launch; reused instead of creating a
    /// new unused session each time.
    scratch_session: Option<String>,
    /// The pending open is that scratch-session reuse, so a missing session
    /// falls back to a fresh one instead of a read-only view.
    scratch_opening: bool,
    opening_session_events: Vec<Value>,
    pub(crate) page: WorkspacePage,
    pending_action: Option<PendingAction>,
    pub(crate) creating_session: bool,
    fork_pending: Option<ForkOrigin>,
    pub(crate) copied_mark: Option<String>,
    message_feedback: message_feedback::MessageFeedback,
    pub(crate) model_select: Option<gpui::Entity<Select>>,
    pub(crate) reasoning_select: Option<gpui::Entity<Select>>,
    pub(crate) composer: gpui::Entity<Editor>,
    pub(crate) workspace_editor: gpui::Entity<Editor>,
    pub(crate) workspace_rename_editor: gpui::Entity<Editor>,
    session_rename_editor: gpui::Entity<Editor>,
    pub(crate) service_editor: gpui::Entity<Editor>,
    pub(crate) search_editor: gpui::Entity<Editor>,
    pub(crate) sessions_scroll: ScrollHandle,
    pub(crate) search_expanded: bool,
    pub(crate) search_slide: kui::primitives::family::Slide,
    pub(crate) sidebar_collapsed: bool,
    sidebar_narrow: bool,
    sidebar_drawer: bool,
    sidebar_drawer_focus: SidebarDrawerFocus,
    narrow_sidebar_expanded: bool,
    pub(crate) sidebar_width: f32,
    pub(crate) sidebar_dragging: bool,
    pub(crate) sidebar_slide: Option<DockSlide>,
    pub(crate) brand_hover: kui::primitives::family::Slide,
    pub(crate) show_more_menu: bool,
    pub(crate) show_settings: bool,
    pub(crate) settings_page: SettingsPage,
    transcript_preferences: transcript_preferences::TranscriptPreferences,
    model_providers: model_providers::ProviderSettings,
    runtime_settings: Option<runtime_settings::RuntimeSettings>,
    web_search_settings: Option<web_search_settings::WebSearchSettings>,
    plugin_center: plugin_center::PluginCenter,
    automations: automations::Automations,
    session_archival: session_archive::ArchiveUi,
    plugin_installer: Option<plugin_install::Installer>,
    plugin_install_trigger: Option<gpui::FocusHandle>,
    plugin_add_menu: Option<plugin_install::AddMenu>,
    shortcuts: shortcut_settings::ShortcutUi,
    header_menu: Option<header_chrome::OpenHeaderMenu>,
    exiting_header_menu: Option<header_chrome::ExitingHeaderMenu>,
    header_menu_buttons: Rc<RefCell<Vec<gpui::Bounds<gpui::Pixels>>>>,
    session_markdown: session_markdown::SessionMarkdownCopy,
    session_downloads: session_download::SessionDownloads,
    session_feedback: session_feedback::SessionFeedback,
    workspace_applications: Vec<header_chrome::WorkspaceApplication>,
    workspace_open_with: Option<String>,
    workspace_open_pending: bool,
    developer_tools: bool,
    developer_tools_loading: bool,
    developer_tools_saving: bool,
    developer_tools_error: bool,
    developer_tools_switched_at: Option<f64>,
    pub(crate) content_font_delta: i32,
    pub(crate) work_details: WorkDetailsMode,
    pub(crate) collapse_next_input: bool,
    pub(crate) keep_folded_replies: bool,
    pub(crate) show_workspace_menu: bool,
    pub(crate) show_workspace_view_options: bool,
    pub(crate) workspace_context_menu: Option<(String, f32)>,
    pub(crate) hovered_workspace: Option<String>,
    hovered_session: Option<String>,
    session_row_focus: HashMap<String, gpui::FocusHandle>,
    session_row_more_focus: HashMap<String, gpui::FocusHandle>,
    session_title_focus: gpui::FocusHandle,
    pub(crate) collapsed_workspaces: std::collections::HashSet<String>,
    /// 官方水位线：每个 Workspace 当前显示的 Session 行数（初始 5，不持久化）。
    pub(crate) workspace_watermarks: HashMap<String, usize>,
    pub(crate) hidden_workspaces: std::collections::HashSet<String>,
    added_workspaces: Vec<String>,
    workspace_picker_pending: bool,
    pub(crate) workspace_aliases: std::collections::HashMap<String, String>,
    pub(crate) workspace_rename: Option<String>,
    pub(crate) workspace_delete: Option<String>,
    session_rename: Option<SessionRename>,
    pub(crate) pinned_sessions: std::collections::HashSet<String>,
    legacy_session_titles: HashMap<String, String>,
    session_title_seqs: HashMap<String, u64>,
    pub(crate) archived_sessions: std::collections::HashSet<String>,
    pub(crate) toasts: Vec<Toast>,
    /// User-message images already requested from the host (by attachment id).
    pub(crate) requested_attachments: std::collections::HashSet<String>,
    /// Chat HTTP(S) links open in the built-in right-sidebar browser (official
    /// default) unless the 网页链接 preference selects the system browser.
    pub(crate) link_open_external: bool,
    /// Composer statistics compact mode (official 性能与用量 · 简洁).
    pub(crate) performance_compact: bool,
    /// Full-screen picture opened from a user bubble (official image dialog).
    pub(crate) image_viewer: Option<ImageViewer>,
    confirm_permission: Option<String>,
    pub(crate) full_access_acknowledged: bool,
    pub(crate) stats_dialog: Option<StatsDialog>,
    stats_dialog_bounds: ComposerAnchor,
    pub(crate) workspace_group_by: WorkspaceGroupBy,
    pub(crate) workspace_order_by: WorkspaceOrderBy,
    pub(crate) workspace_archived_filter: WorkspaceArchivedFilter,
    sidebar: SessionSidebar,
    parked_sidebars: HashMap<String, SessionSidebar>,
    right_dock_slide: DockSlide,
    right_dock_frame: DockFrame,
    right_dock_viewport: Option<f32>,
    right_dock_preferred_width: Option<f32>,
    right_dock_drag: Option<start_panel::DockDrag>,
    dock_tab_gesture: Option<dock_drag::DockTabGesture>,
    dock_tab_menus: dock_tab_menu::DockTabMenus,
    dock_controls: dock_controls::DockControls,
    terminal_shell_menu: terminal_shell_menu::ShellMenuState,
    terminal_title_edit: Option<terminal_title::TerminalTitleEdit>,
    float_gesture: Option<floating_panes::FloatGesture>,
    float_return: Option<floating_panes::FloatReturn>,
    float_grips: HashMap<DockPaneId, floating_panes::FloatGrip>,
    right_dock_focus: HashMap<DockPaneId, gpui::FocusHandle>,
    pub(crate) drop_active: bool,
    pub(crate) composer_menu: Option<ComposerMenu>,
    composer_card_bounds: ComposerAnchor,
    mention_files: FileListing,
    pub(crate) file_token: Option<FileToken>,
    pub(crate) file_filter: String,
    pub(crate) file_selected: usize,
    next_file_request: u64,
    pub(crate) file_show_crumbs: bool,
    /// 右侧内嵌终端：多页签 Host PTY（对齐官方 sidebar-terminal tabs）。
    next_terminal_tab: u64,
    next_browser_key: u64,
    browser_shell: Option<u64>,
    browser_revision: u64,
    browser_placements: Vec<browser::BrowserPlacement>,
    pub(crate) command_filter: String,
    pub(crate) command_from_slash: bool,
    pub(crate) command_selected: usize,
    pub(crate) command_catalog: CommandCatalog,
    pub(crate) model_filter_editor: gpui::Entity<Editor>,
    model_menu: model_menu::ModelMenu,
    permission_menu: permission_picker::PermissionMenu,
    command_picker: command_picker::CommandPicker,
    /// Official RunningStatus activity label ("深度求索中，用时 Xs···").
    ///
    /// Uses the local `LiveShimmer` for the same reason as `process_shimmer`:
    /// the elapsed-seconds clock rewrites this text once per second, and KUI's
    /// component would replay a fade-in each time.
    pub(crate) seeking_shimmer: gpui::Entity<live_shimmer::LiveShimmer>,
    /// Official ChatGroupSeat live process title shimmer (`TextShimmer active={!closed}`).
    ///
    /// This is the local `LiveShimmer` rather than KUI's `TextShimmer`: the live
    /// title embeds streaming reasoning text that changes almost every frame,
    /// and KUI's component replays a fade-in per text change, which never
    /// completes and leaves the row flickering.
    pub(crate) process_shimmer: gpui::Entity<live_shimmer::LiveShimmer>,
    pub(crate) last_escape: Option<f64>,
    suppress_file_trigger: bool,
    sessions_loading: bool,
    /// ACP `session/list` pagination finished (or failed); local merge may finalize the sidebar.
    sessions_acp_complete: bool,
    pub(crate) show_welcome: bool,
    shell_presentation: Option<(bool, Option<bool>)>,
    native_fullscreen: bool,
    pub(crate) welcome: gpui::Entity<Welcome>,
    turn_ticker_running: bool,
    /// Last tick from the live turn clock. A stale flag with an old heartbeat
    /// means the previous ticker exited without clearing the flag.
    turn_ticker_heartbeat: f64,
    _composer_subscriptions: Vec<Subscription>,
    _workspace_rename_subscription: Subscription,
    _session_rename_subscription: Subscription,
    _search_subscription: Subscription,
    _model_filter_subscription: Subscription,
    _config_subscriptions: Vec<Subscription>,
    _welcome_subscription: Subscription,
}

impl DshApp {
    fn shell_fullscreen(&self, w: &Window) -> bool {
        self.native_fullscreen || w.is_fullscreen()
    }

    /// 侧栏是否处于 rail 态：手动折叠，或视口 < 1024 触发官方自动折叠。
    pub(crate) fn rail_active(&self, w: &Window) -> bool {
        if f32::from(w.viewport_size().width) < 1024. {
            !self.narrow_sidebar_expanded
        } else {
            self.sidebar_collapsed
        }
    }

    fn sidebar_track_width(&self, w: &Window) -> f32 {
        if self.sidebar_drawer {
            0.
        } else if self.rail_active(w) {
            workspace::collapsed_sidebar_width()
        } else {
            self.sidebar_width
        }
    }

    fn sidebar_drawer_covers_window(&self) -> bool {
        self.sidebar_drawer
            && (self.narrow_sidebar_expanded
                || self
                    .sidebar_slide
                    .as_ref()
                    .is_some_and(|slide| slide.sample(bridge::monotonic_ms()).0 > 0.))
    }

    fn set_left_sidebar_collapsed(
        &mut self,
        collapsed: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if f32::from(w.viewport_size().width) < 1024. {
            self.narrow_sidebar_expanded = !collapsed;
            if self.sidebar_drawer {
                self.sidebar_dragging = false;
                self.composer_menu = None;
                self.header_menu = None;
                self.exiting_header_menu = None;
                if collapsed {
                    self.show_more_menu = false;
                    self.show_workspace_view_options = false;
                    self.sidebar_drawer_focus.opener.focus(w, cx);
                } else {
                    self.sidebar_drawer_focus.close.focus(w, cx);
                }
            }
        } else {
            self.sidebar_collapsed = collapsed;
            self.save_workspace_preferences(cx);
        }
        cx.notify();
    }

    fn new(w: &mut Window, cx: &mut Context<Self>) -> Self {
        document_fonts::prepare(cx);
        // 启动即空会话 → 官方 hero placeholder；有 transcript 后由 render_composer 切换。
        let composer = editor(
            "描述你想要构建的内容, / 调用指令, @ 文件或对话".into(),
            InputMode::InlineMulti,
            w,
            cx,
        );
        composer.update(cx, |editor, _| editor.set_command_submit(true));
        // `EditorShortcutEvent` is Cmd/Ctrl+Enter: the complementary delivery
        // gesture (official `resolveSubmitMode`), so a busy session steers
        // instead of queueing.
        let submit = cx.subscribe_in(&composer, w, |this, _, _: &EditorShortcutEvent, w, cx| {
            this.submit_prompt_accelerated(w, cx);
        });
        let submit_plain = cx.subscribe_in(&composer, w, |this, _, event: &EditEvent, w, cx| {
            if *event == EditEvent::Submitted {
                if this.composer_menu == Some(ComposerMenu::Commands) && this.command_from_slash {
                    this.activate_first_command(w, cx);
                } else if this.composer_menu != Some(ComposerMenu::Files) {
                    this.submit_prompt(w, cx);
                }
            } else if *event == EditEvent::Changed {
                this.sync_file_trigger(w, cx);
                this.sync_command_trigger(cx);
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
            |this, _, event: &EditEvent, w, cx| match event {
                EditEvent::Submitted => {
                    this.confirm_workspace_rename(cx);
                    cx.notify();
                }
                EditEvent::Escape => {
                    this.close_workspace_dialog(w, cx);
                    cx.notify();
                }
                EditEvent::Changed => cx.notify(),
                _ => {}
            },
        );
        let session_rename_editor = editor("会话名称".into(), InputMode::Single, w, cx);
        let session_rename_subscription = cx.subscribe_in(
            &session_rename_editor,
            w,
            |this, _, event: &EditEvent, w, cx| {
                match event {
                    EditEvent::Submitted => this.confirm_session_rename(cx),
                    EditEvent::Escape => this.close_workspace_dialog(w, cx),
                    EditEvent::Changed => {
                        if let Some(rename) = &mut this.session_rename {
                            rename.error = None;
                        }
                    }
                    _ => {}
                }
                cx.notify();
            },
        );
        let service_editor = editor(
            "dsh-acp 路径（留空自动寻找 ./dsh-acp）".into(),
            InputMode::Single,
            w,
            cx,
        );
        let search_editor = editor("搜索会话名称".into(), InputMode::Search, w, cx);
        search_editor.update(cx, |input, _| {
            input.font_size = 13.;
            input.disabled = true;
        });
        let search_subscription =
            cx.subscribe_in(&search_editor, w, |this, _, event: &EditEvent, w, cx| {
                if *event == EditEvent::Escape {
                    this.close_workspace_search(w, cx);
                }
                cx.notify();
            });
        let model_filter_editor = editor("搜索模型…".into(), InputMode::Single, w, cx);
        model_filter_editor.update(cx, |input, _| input.font_size = 12.);
        let model_filter_subscription = cx.subscribe_in(
            &model_filter_editor,
            w,
            |this, _, event: &EditEvent, _, cx| {
                if *event == EditEvent::Changed
                    && this.composer_menu == Some(ComposerMenu::ModelChoices)
                {
                    let query = this.model_filter_editor.read(cx).value();
                    if this.model_menu.query != query {
                        this.model_menu.query = query.to_owned();
                        this.model_menu.active = Some(0);
                        this.model_menu.viewport.reset();
                        cx.notify();
                    }
                }
            },
        );
        // 官方“深度求索中”标签：与官方 TextShimmer 同参数的移动高光，
        // 但用本地组件，使每秒跳动的用时不会重启动画。
        let seeking_shimmer = cx.new(|cx| {
            live_shimmer::LiveShimmer::new("深度求索中···", cx)
                .with_duration(std::time::Duration::from_millis(1500))
                .with_font_size(12.)
        });
        // Official process header title: same shimmer while the group is live.
        // Uses the local component so a continuously growing title keeps one
        // uninterrupted sweep instead of restarting a fade on every change.
        let process_shimmer = cx.new(|cx| {
            live_shimmer::LiveShimmer::new("正在分析请求", cx)
                .with_duration(std::time::Duration::from_millis(1500))
                .with_font_size(13.)
        });
        let welcome = cx.new(|cx| Welcome::new(w, cx));
        let welcome_subscription =
            cx.subscribe_in(&welcome, w, |this, _, event: &WelcomeEvent, w, cx| {
                match event {
                    WelcomeEvent::Changed => {
                        let welcome = this.welcome.read(cx);
                        this.show_welcome = welcome.should_show();
                        this.sync_shell_window(cx);
                    }
                    WelcomeEvent::OpenModelProviders => {
                        this.show_settings = true;
                        this.open_model_providers(w, cx);
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
            conversation: Conversation::fresh(),
            parked_conversations: HashMap::new(),
            session_activity: HashMap::new(),
            running_subagents: HashMap::new(),
            subagent_view: None,
            scratch_session: None,
            scratch_opening: false,
            opening_session_events: Vec::new(),
            page: WorkspacePage::Conversation,
            pending_action: None,
            creating_session: false,
            fork_pending: None,
            copied_mark: None,
            message_feedback: Default::default(),
            model_select: None,
            reasoning_select: None,
            composer,
            workspace_editor,
            workspace_rename_editor,
            session_rename_editor,
            service_editor,
            search_editor,
            sessions_scroll: ScrollHandle::new(),
            search_expanded: false,
            search_slide: kui::primitives::family::Slide::new(0.),
            sidebar_collapsed: false,
            sidebar_narrow: false,
            sidebar_drawer: false,
            sidebar_drawer_focus: SidebarDrawerFocus::new(cx),
            narrow_sidebar_expanded: false,
            sidebar_width: 280.,
            sidebar_dragging: false,
            sidebar_slide: None,
            brand_hover: kui::primitives::family::Slide::snappy(0.),
            show_more_menu: false,
            show_settings: false,
            settings_page: SettingsPage::Credentials,
            transcript_preferences: transcript_preferences::TranscriptPreferences::new(cx),
            model_providers: model_providers::ProviderSettings::new(w, cx),
            runtime_settings: None,
            web_search_settings: None,
            plugin_center: plugin_center::PluginCenter::default(),
            automations: automations::Automations::new(),
            session_archival: session_archive::ArchiveUi::default(),
            plugin_installer: None,
            plugin_install_trigger: None,
            plugin_add_menu: None,
            shortcuts: shortcut_settings::ShortcutUi::new(w, cx),
            header_menu: None,
            exiting_header_menu: None,
            header_menu_buttons: Rc::default(),
            session_markdown: session_markdown::SessionMarkdownCopy::default(),
            session_downloads: session_download::SessionDownloads::new(cx),
            session_feedback: session_feedback::SessionFeedback::new(w, cx),
            workspace_applications: Vec::new(),
            workspace_open_with: None,
            workspace_open_pending: false,
            developer_tools: false,
            developer_tools_loading: true,
            developer_tools_saving: false,
            developer_tools_error: false,
            developer_tools_switched_at: None,
            content_font_delta: 0,
            work_details: WorkDetailsMode::Standard,
            collapse_next_input: false,
            keep_folded_replies: true,
            show_workspace_menu: false,
            show_workspace_view_options: false,
            workspace_context_menu: None,
            hovered_workspace: None,
            hovered_session: None,
            session_row_focus: HashMap::new(),
            session_row_more_focus: HashMap::new(),
            session_title_focus: cx.focus_handle(),
            collapsed_workspaces: std::collections::HashSet::new(),
            workspace_watermarks: HashMap::new(),
            hidden_workspaces: std::collections::HashSet::new(),
            added_workspaces: Vec::new(),
            workspace_picker_pending: false,
            workspace_aliases: std::collections::HashMap::new(),
            workspace_rename: None,
            workspace_delete: None,
            session_rename: None,
            pinned_sessions: std::collections::HashSet::new(),
            legacy_session_titles: HashMap::new(),
            session_title_seqs: HashMap::new(),
            archived_sessions: std::collections::HashSet::new(),
            toasts: Vec::new(),
            requested_attachments: std::collections::HashSet::new(),
            link_open_external: false,
            performance_compact: false,
            image_viewer: None,
            confirm_permission: None,
            full_access_acknowledged: false,
            stats_dialog: None,
            stats_dialog_bounds: ComposerAnchor::default(),
            workspace_group_by: WorkspaceGroupBy::Workspace,
            workspace_order_by: WorkspaceOrderBy::Updated,
            workspace_archived_filter: WorkspaceArchivedFilter::Hide,
            sidebar: SessionSidebar::default(),
            parked_sidebars: HashMap::new(),
            right_dock_slide: DockSlide::default(),
            right_dock_frame: DockFrame::default(),
            right_dock_viewport: None,
            right_dock_preferred_width: None,
            right_dock_drag: None,
            dock_tab_gesture: None,
            dock_tab_menus: dock_tab_menu::DockTabMenus::default(),
            dock_controls: dock_controls::DockControls::default(),
            terminal_shell_menu: terminal_shell_menu::ShellMenuState::default(),
            terminal_title_edit: None,
            float_gesture: None,
            float_return: None,
            float_grips: HashMap::new(),
            right_dock_focus: HashMap::new(),
            drop_active: false,
            composer_menu: None,
            composer_card_bounds: ComposerAnchor::default(),
            mention_files: FileListing::default(),
            file_token: None,
            file_filter: String::new(),
            file_selected: 0,
            next_file_request: 0,
            file_show_crumbs: false,
            next_terminal_tab: 0,
            next_browser_key: 0,
            browser_shell: None,
            browser_revision: 0,
            browser_placements: Vec::new(),
            command_filter: String::new(),
            command_from_slash: false,
            command_selected: 0,
            command_catalog: CommandCatalog::default(),
            model_filter_editor,
            model_menu: model_menu::ModelMenu::new(w, cx),
            permission_menu: permission_picker::PermissionMenu::new(cx),
            command_picker: command_picker::CommandPicker::new(w, cx),
            seeking_shimmer,
            process_shimmer,
            last_escape: None,
            suppress_file_trigger: false,
            sessions_loading: false,
            sessions_acp_complete: false,
            show_welcome: true,
            shell_presentation: None,
            native_fullscreen: false,
            welcome,
            turn_ticker_running: false,
            turn_ticker_heartbeat: 0.,
            _composer_subscriptions: vec![submit, submit_plain],
            _workspace_rename_subscription: workspace_rename_subscription,
            _session_rename_subscription: session_rename_subscription,
            _search_subscription: search_subscription,
            _model_filter_subscription: model_filter_subscription,
            _config_subscriptions: Vec::new(),
            _welcome_subscription: welcome_subscription,
        }
    }

    pub(crate) fn connected(&self) -> bool {
        self.status == ServiceStatus::Connected
    }

    /// 本轮新增消耗（运行中实时刷新，供“已消耗 ◇ N”使用）。
    pub(crate) fn turn_used_delta(&self) -> Option<u64> {
        self.conversation
            .usage
            .map(|(used, _)| used.saturating_sub(self.conversation.turn_usage_base.unwrap_or(0)))
            .filter(|used| *used > 0)
    }

    /// 官方 toast：160ms 滑入，停留 3s，1s 淡出后移除（总 4s）。
    /// 生命周期由 `anim` 的常量推导，避免渲染曲线与卸载定时器各写一个数而漂移。
    pub(crate) fn push_toast(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        let total_ms = (anim::TOAST_TOTAL_SECONDS * 1000.) as f64;
        // Wake slightly after the fade finishes so the last frame is not clipped.
        let wake_ms = total_ms as u64 + 100;
        self.toasts
            .retain(|toast| bridge::monotonic_ms() - toast.at < total_ms);
        self.toasts.push(Toast {
            message: message.into(),
            at: bridge::monotonic_ms(),
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(wake_ms))
                .await;
            this.update(cx, |app, cx| {
                app.toasts
                    .retain(|toast| bridge::monotonic_ms() - toast.at < total_ms);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn refresh_sessions(&mut self) {
        if self.sessions_loading {
            return;
        }
        self.sessions_loading = true;
        self.sessions_acp_complete = false;
        // Local durable sessions (projcache) load in parallel with ACP pagination.
        invoke("list_local_sessions", json!({}));
        invoke(
            "list_sessions",
            json!({"cwd": Value::Null, "cursor": Value::Null}),
        );
    }

    fn activate_session(
        &mut self,
        session: SessionSummary,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_session.as_deref() != Some(session.id.as_str()) {
            self.command_catalog.clear();
            let next = self
                .parked_conversations
                .remove(&session.id)
                .unwrap_or_else(Conversation::fresh);
            let mut previous = std::mem::replace(&mut self.conversation, next);
            if let Some(id) = self.detach_sidebar(w, cx) {
                previous.draft = self.composer.read(cx).value().to_owned();
                previous.draft_workspace = self
                    .sessions
                    .iter()
                    .find(|session| session.id == id && session.blank && !previous.running)
                    .map(|session| session.cwd.clone());
                if previous.draft_workspace.is_some()
                    && previous.draft.is_empty()
                    && previous.draft_attachments.is_empty()
                    && previous.plan_active != Some(true)
                    && previous.plan_reviews.is_empty()
                    && previous.questions.is_empty()
                {
                    if previous.acp_open {
                        invoke("close_session", json!({"sessionId": id}));
                    }
                    self.sessions.retain(|session| session.id != id);
                    self.parked_sidebars.remove(&id);
                } else {
                    self.parked_conversations.insert(id, previous);
                }
            }
            let draft = std::mem::take(&mut self.conversation.draft);
            self.replace_composer_without_trigger(&draft, w, cx);
            self.composer_menu = None;
            self.drop_active = false;
            self.last_escape = None;
            self.stats_dialog = None;
            self.sidebar = self.parked_sidebars.remove(&session.id).unwrap_or_else(|| {
                let surface = DockSurface::restore(&bridge::read_sidebar(&session.id));
                SessionSidebar {
                    persisted: surface.clone(),
                    surface,
                    ..Default::default()
                }
            });
            self.mount_restored_browsers(w, cx);
        }
        if self.sidebar.workspace != session.cwd {
            self.sidebar.workspace.clone_from(&session.cwd);
            self.sidebar.file_trees.clear();
            self.sidebar.documents.clear();
        }
        self.sessions.retain(|known| known.id != session.id);
        self.sessions.insert(0, session.clone());
        self.workspace.clone_from(&session.cwd);
        replace(&self.workspace_editor, self.workspace.clone(), w, cx);
        if let Some(activity) = self.session_activity.get_mut(&session.id) {
            activity.acknowledge();
        }
        self.active_session = Some(session.id);
        self.mount_restored_terminals(w, cx);
        self.start_turn_ticker(cx);
        self.maybe_follow_tail();
    }

    fn detach_sidebar(&mut self, w: &mut Window, cx: &mut Context<Self>) -> Option<String> {
        self.header_menu = None;
        self.exiting_header_menu = None;
        self.confirm_permission = None;
        self.full_access_acknowledged = false;
        self.dismiss_session_feedback(w, cx);
        self.clear_session_feedback();
        self.message_feedback.reset();
        if self.focused_right_pane(w, cx).is_some() {
            self.focus_composer(w, cx);
        }
        self.finish_terminal_title(true, w, cx);
        self.persist_right_sidebar();
        let previous = self.active_session.take();
        let mut sidebar = std::mem::take(&mut self.sidebar);
        for tree in sidebar.file_trees.values_mut() {
            tree.suspend();
        }
        for document in sidebar.documents.values_mut() {
            document.suspend_watch();
        }
        for strip in sidebar.tab_strips.values() {
            strip.suspend();
        }
        if let Some(id) = &previous {
            self.parked_sidebars.insert(id.clone(), sidebar);
        }
        self.right_dock_drag = None;
        self.dock_tab_gesture = None;
        self.dock_tab_menus = dock_tab_menu::DockTabMenus::default();
        self.dock_controls = dock_controls::DockControls::default();
        self.terminal_shell_menu = terminal_shell_menu::ShellMenuState::default();
        self.float_gesture = None;
        self.float_return = None;
        self.float_grips.clear();
        self.right_dock_focus.clear();
        self.right_dock_viewport = None;
        self.mention_files = FileListing::default();
        self.file_token = None;
        previous
    }

    fn sidebar_for_session(&mut self, session: &str) -> Option<&mut SessionSidebar> {
        if self.active_session.as_deref() == Some(session) {
            Some(&mut self.sidebar)
        } else {
            self.parked_sidebars.get_mut(session)
        }
    }

    fn conversation_mut(&mut self, session: &str) -> Option<&mut Conversation> {
        if self.active_session.as_deref() == Some(session) {
            Some(&mut self.conversation)
        } else {
            self.parked_conversations.get_mut(session)
        }
    }

    fn session_conversation(&self, session: &str) -> Option<&Conversation> {
        if self.active_session.as_deref() == Some(session) {
            Some(&self.conversation)
        } else {
            self.parked_conversations.get(session)
        }
    }

    fn session_indicator(&self, id: &str) -> (SessionIndicator, usize) {
        let conversation = self.session_conversation(id);
        let running = conversation.is_some_and(|conversation| conversation.running);
        let pending = conversation.and_then(|conversation| {
            if conversation.permission.is_some() {
                Some(PendingInteraction::Approval)
            } else if !conversation.plan_reviews.is_empty() {
                Some(PendingInteraction::PlanReview)
            } else if conversation
                .questions
                .iter()
                .any(|panel| panel.is_pending())
            {
                Some(PendingInteraction::Question)
            } else {
                None
            }
        });
        let subagents = self
            .running_subagents
            .values()
            .filter(|parent| parent.as_str() == id)
            .count();
        let baseline = SessionActivity::default();
        let status = self.session_activity.get(id).unwrap_or(&baseline);
        (status.indicator(running, pending, subagents), subagents)
    }

    fn observe_local_session_activity(&mut self, id: &str, previous: bool, running: bool) {
        let selected = self.active_session.as_deref() == Some(id);
        self.session_activity
            .entry(id.to_owned())
            .or_default()
            .observe_local(previous, running, selected);
    }

    fn observe_host_session_activity(&mut self, id: &str, update: &Value) {
        let Some(running) = update["running"].as_bool() else {
            return;
        };
        if let Some(parent) = update["parentSessionId"].as_str() {
            if running {
                self.running_subagents
                    .insert(id.to_owned(), parent.to_owned());
            } else {
                self.running_subagents.remove(id);
            }
            return;
        }
        let local_running = self
            .session_conversation(id)
            .is_some_and(|conversation| conversation.running);
        let selected = self.active_session.as_deref() == Some(id);
        self.session_activity
            .entry(id.to_owned())
            .or_default()
            .observe_host(running, local_running, selected);
    }

    fn record_session_prompt_time(&mut self, id: &str, time: u64) {
        let activity = self.session_activity.entry(id.to_owned()).or_default();
        activity.last_prompt_at_ms = activity.last_prompt_at_ms.max(Some(time));
    }

    fn persist_right_sidebar(&mut self) {
        if let Some(session) = &self.active_session {
            self.sidebar.persist(session);
        }
    }

    fn apply_session_page(&mut self, reply: &Value) {
        let incoming = reply
            .get("sessions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|session| {
                let id = session.get("sessionId")?.as_str()?.to_string();
                let cwd = session.get("cwd")?.as_str()?.to_string();
                // ACP session/list often omits titles or sends placeholders; treat those as missing
                // so peek_session_titles can fill from projcache / session.v4.
                let title = session
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|title| {
                        !title.is_empty()
                            && *title != "未命名会话"
                            && *title != "未命名"
                            && *title != "Untitled"
                            && *title != "New session"
                    })
                    .map(str::to_string);
                Some(SessionSummary {
                    id,
                    cwd,
                    title,
                    blank: false,
                    created_at_ms: session
                        .get("createdAt")
                        .and_then(Value::as_u64)
                        .or_else(|| {
                            session
                                .get("createdAt")
                                .and_then(Value::as_f64)
                                .map(|value| value as u64)
                        }),
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
            self.sessions_acp_complete = true;
            // Official sidebar also surfaces durable local sessions ACP may omit.
            invoke("list_local_sessions", json!({}));
        }
    }

    fn apply_local_sessions(&mut self, reply: &Value) {
        let incoming = reply
            .get("sessions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|session| {
                let id = session.get("sessionId")?.as_str()?.to_string();
                let cwd = session.get("cwd")?.as_str()?.to_string();
                let title = session
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|title| {
                        !title.is_empty()
                            && *title != "未命名会话"
                            && *title != "未命名"
                            && *title != "Untitled"
                            && *title != "New session"
                    })
                    .map(str::to_string);
                Some(SessionSummary {
                    id,
                    cwd,
                    title,
                    blank: false,
                    created_at_ms: session
                        .get("createdAt")
                        .and_then(Value::as_u64)
                        .or_else(|| {
                            session
                                .get("createdAt")
                                .and_then(Value::as_f64)
                                .map(|value| value as u64)
                        }),
                })
            });
        merge_session_page(&mut self.sessions, incoming, self.active_session.as_deref());
        // ACP session/list omits titles; enrich from durable session.v4 logs / first prompt.
        self.request_session_titles();
        if self.sessions_acp_complete {
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

    /// Harness-style "添加工作区": delegate to the host's native directory
    /// picker. The chosen path is applied in `handle_reply`.
    pub(super) fn request_workspace_picker(&mut self) {
        if self.workspace_picker_pending {
            return;
        }
        self.workspace_picker_pending = true;
        invoke("pick_workspace_directory", json!({}));
    }

    fn finish_workspace_pick(
        &mut self,
        result: Result<Value, String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace_picker_pending = false;
        let reply = match result {
            Ok(reply) => reply,
            Err(error) => {
                self.push_toast(error, cx);
                return;
            }
        };
        let Some(path) = reply.as_str().filter(|path| !path.is_empty()) else {
            return;
        };
        if !self
            .added_workspaces
            .iter()
            .any(|workspace| workspace == path)
        {
            self.added_workspaces.push(path.to_owned());
        }
        self.hidden_workspaces.remove(path);
        self.collapsed_workspaces.remove(path);
        self.save_workspace_preferences(cx);
        if self.connected() {
            self.new_session_at(path.to_owned(), None, w, cx);
        } else if !self.service_starting {
            replace(&self.workspace_editor, path.to_owned(), w, cx);
            self.start_service(w, cx);
        }
        cx.notify();
    }

    fn start_service(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.conversation.running
            || self
                .parked_conversations
                .values()
                .any(|session| session.running)
            || self.service_starting
            || self.creating_session
            || self.fork_pending.is_some()
            || self.plugin_install_busy()
        {
            return;
        }
        let workspace = self.workspace_editor.read(cx).value().trim().to_string();
        let service_path = self.service_editor.read(cx).value().trim().to_string();
        self.service_starting = true;
        self.runtime_settings = None;
        self.web_search_settings = None;
        self.plugin_center = plugin_center::PluginCenter::default();
        self.automations = automations::Automations::new();
        self.plugin_installer = None;
        self.close_plugin_add_menu();
        self.creating_session = false;
        self.pending_action = None;
        self.status = ServiceStatus::Starting;
        self.error = None;
        self.detach_sidebar(w, cx);
        self.page = WorkspacePage::Conversation;
        self.conversation = Conversation::fresh();
        self.parked_conversations.clear();
        self.session_activity.clear();
        self.running_subagents.clear();
        self.hovered_session = None;
        self.opening_session_events.clear();
        self.sessions.clear();
        self.sessions_loading = false;
        self.composer_menu = None;
        self.rebuild_selectors(w, cx);
        invoke(
            "start_service",
            json!({"workspace": workspace, "servicePath": service_path}),
        );
        cx.notify();
    }

    pub(crate) fn open_page(&mut self, page: WorkspacePage, cx: &mut Context<Self>) {
        self.page = page;
        if page != WorkspacePage::Automations {
            self.close_automation_create_menu();
        }
        if self.sidebar_drawer {
            self.narrow_sidebar_expanded = false;
        }
        self.composer_menu = None;
        self.show_workspace_menu = false;
        self.show_workspace_view_options = false;
        self.show_more_menu = false;
        cx.notify();
    }

    fn new_session(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.new_session_at(self.workspace.clone(), None, w, cx);
    }

    /// Open the blank session a previous launch left behind, or create one.
    ///
    /// Creating a session writes its seed events to disk immediately, so doing
    /// that on every launch left one unused 未命名 session behind per start.
    /// The remembered id is only reused while it is still a blank session: the
    /// first message sent into it forgets the id, and the next launch creates a
    /// fresh blank session again.
    fn open_scratch_session(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.connected() || self.creating_session {
            return;
        }
        let remembered = self.scratch_session.clone();
        let Some(id) = remembered else {
            self.new_session(w, cx);
            return;
        };
        self.scratch_opening = true;
        self.begin_action(
            PendingAction::Resume(SessionSummary {
                id,
                cwd: self.workspace.clone(),
                title: None,
                blank: true,
                created_at_ms: None,
            }),
            cx,
        );
    }

    /// Remember the blank session a launch opened.
    fn remember_scratch_session(&mut self, id: &str, cx: &App) {
        if self.scratch_session.as_deref() == Some(id) {
            return;
        }
        self.scratch_session = Some(id.to_owned());
        self.save_workspace_preferences(cx);
    }

    /// The scratch session received its first content: stop reusing it.
    fn forget_scratch_session(&mut self, session: &str, cx: &App) {
        if self.scratch_session.as_deref() != Some(session) {
            return;
        }
        self.scratch_session = None;
        self.save_workspace_preferences(cx);
    }

    fn new_session_at(
        &mut self,
        cwd: String,
        draft: Option<&'static str>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.connected() || self.creating_session {
            return;
        }
        if self.sidebar_drawer && self.narrow_sidebar_expanded {
            self.set_left_sidebar_collapsed(true, w, cx);
        }
        if self.conversation.acp_open
            && self.sessions.iter().any(|session| {
                session.blank
                    && session.cwd == cwd
                    && self.active_session.as_ref() == Some(&session.id)
            })
        {
            self.open_page(WorkspacePage::Conversation, cx);
        } else if let Some((id, _)) = self.parked_conversations.iter().find(|(_, session)| {
            session.acp_open && session.draft_workspace.as_deref() == Some(cwd.as_str())
        }) {
            self.resume(
                SessionSummary {
                    id: id.clone(),
                    cwd,
                    title: None,
                    blank: true,
                    created_at_ms: None,
                },
                w,
                cx,
            );
        } else {
            self.begin_action(PendingAction::New { cwd, draft }, cx);
            return;
        }
        if let Some(draft) = draft {
            let existing = self.composer.read(cx).value();
            let draft = if existing.is_empty() {
                draft.to_owned()
            } else {
                format!("{existing}\n\n{draft}")
            };
            self.replace_composer_without_trigger(&draft, w, cx);
        }
    }

    fn resume(&mut self, session: SessionSummary, w: &mut Window, cx: &mut Context<Self>) {
        if !self.connected() || self.creating_session {
            return;
        }
        // Any ordinary navigation leaves the read-only child record.
        self.subagent_view = None;
        if self.archived_sessions.contains(&session.id) {
            // 官方：已归档结果不清空查询、不导航。
            self.push_toast("已归档对话暂时无法查看，请取消归档后查看", cx);
            return;
        }
        // 官方：选择未归档搜索结果会清空并收起搜索、打开会话，
        // 分组浏览还按需展开其所属 Workspace。
        if self.search_expanded {
            self.close_workspace_search(w, cx);
        }
        if self.workspace_group_by != WorkspaceGroupBy::Flat {
            self.collapsed_workspaces.remove(&session.cwd);
        }
        if self.sidebar_drawer && self.narrow_sidebar_expanded {
            self.set_left_sidebar_collapsed(true, w, cx);
        }
        if self
            .parked_conversations
            .get(&session.id)
            .is_some_and(|session| session.acp_open)
        {
            self.page = WorkspacePage::Conversation;
            self.error = None;
            self.activate_session(session, w, cx);
            self.rebuild_selectors(w, cx);
            cx.notify();
        } else {
            self.begin_action(PendingAction::Resume(session), cx);
        }
    }

    fn fork_session(&mut self, session_id: &str, origin: ForkOrigin, cx: &mut Context<Self>) {
        let at_seq = match origin {
            ForkOrigin::Answer(seq) => Some(seq),
            _ => None,
        };
        let open = at_seq.is_some();
        if !self.connected() || self.creating_session || self.fork_pending.is_some() {
            return;
        }
        self.fork_pending = Some(origin);
        self.creating_session = open;
        invoke(
            "fork_session",
            json!({
                "sessionId": session_id,
                "atSeq": at_seq,
                "open": open,
                "allowMigration": origin != ForkOrigin::Sidebar,
            }),
        );
        cx.notify();
    }

    fn finish_fork(
        &mut self,
        result: Result<Value, String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(origin) = self.fork_pending.take() else {
            return;
        };
        let open = matches!(origin, ForkOrigin::Answer(_));
        if open {
            self.creating_session = false;
        }
        let reply = match result {
            Ok(reply) => reply,
            Err(error) => {
                if open {
                    self.opening_session_events.clear();
                }
                if error.contains("session/migration-required") {
                    self.push_toast("请先打开原会话完成迁移，再创建分支", cx);
                } else if origin == ForkOrigin::Shortcut {
                    let message = if error.contains("session/fork-unavailable") {
                        "当前会话没有已结束的轮次"
                    } else {
                        "无法分叉会话，请重试"
                    };
                    self.push_toast(message, cx);
                }
                self.refresh_sessions();
                return;
            }
        };
        let Some(id) = reply["sessionId"].as_str() else {
            if open {
                self.opening_session_events.clear();
            }
            return;
        };
        let summary = SessionSummary {
            id: id.to_owned(),
            cwd: reply["cwd"].as_str().unwrap_or(&self.workspace).to_owned(),
            title: reply["title"].as_str().map(str::to_owned),
            blank: false,
            created_at_ms: reply["createdAt"].as_u64(),
        };
        if open {
            self.open_conversation(summary, Some(&reply), true, None, w, cx);
        } else {
            merge_session_page(
                &mut self.sessions,
                [summary],
                self.active_session.as_deref(),
            );
        }
        self.refresh_sessions();
    }

    fn begin_action(&mut self, action: PendingAction, cx: &mut Context<Self>) {
        self.creating_session = true;
        self.opening_session_events.clear();
        match &action {
            PendingAction::New { cwd, .. } => invoke("new_session", json!({"cwd": cwd})),
            PendingAction::Resume(session) => invoke(
                "resume_session",
                json!({"sessionId": session.id, "cwd": session.cwd}),
            ),
        }
        self.pending_action = Some(action);
        cx.notify();
    }

    fn open_conversation(
        &mut self,
        session: SessionSummary,
        reply: Option<&Value>,
        resumed: bool,
        draft: Option<&str>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = session.id.clone();
        self.page = WorkspacePage::Conversation;
        self.error = None;
        self.activate_session(session, w, cx);
        self.conversation.acp_open = reply.is_some();
        if reply.is_some() {
            self.request_permission_mode();
            invoke("plan_mode", json!({"sessionId": id}));
            invoke("question_state", json!({"sessionId": id}));
            invoke("session_commands", json!({"sessionId": id}));
            // Reopening a session must restore the messages still waiting in
            // the Host Inbox: they outlive the client that submitted them.
            self.request_inbox_state(&id);
            self.request_goal_state(&id);
        }
        self.conversation.resumed_session = resumed;
        self.conversation.config_options = reply
            .and_then(|reply| reply["configOptions"].as_array())
            .cloned()
            .unwrap_or_default();
        if reply.is_some_and(|reply| reply["hostRunning"] == true) && !self.conversation.running {
            self.conversation.begin_turn();
            self.start_turn_ticker(cx);
        }
        if let Some(draft) = draft {
            self.replace_composer_without_trigger(draft, w, cx);
        }
        self.rebuild_selectors(w, cx);
        if resumed {
            self.request_session_history();
        }
        for event in std::mem::take(&mut self.opening_session_events) {
            if event["sessionId"] == id {
                self.handle_event(event, w, cx);
            }
        }
    }

    fn finish_session_open(
        &mut self,
        result: Result<Value, String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(action) = self.pending_action.take() else {
            return;
        };
        self.creating_session = false;
        match (action, result) {
            (PendingAction::New { cwd, draft }, Ok(reply)) => {
                if let Some(id) = reply["sessionId"].as_str() {
                    self.open_conversation(
                        SessionSummary {
                            id: id.to_owned(),
                            cwd,
                            title: None,
                            blank: true,
                            created_at_ms: None,
                        },
                        Some(&reply),
                        false,
                        draft,
                        w,
                        cx,
                    );
                    self.remember_scratch_session(id, cx);
                    self.refresh_sessions();
                }
            }
            (PendingAction::Resume(session), Ok(reply)) => {
                let scratch = self.scratch_opening
                    && self.scratch_session.as_deref() == Some(session.id.as_str());
                self.scratch_opening = false;
                self.open_conversation(session, Some(&reply), true, None, w, cx);
                if scratch {
                    // Reusing the blank session is a fresh start, not a restore.
                    self.conversation.resumed_session = false;
                }
                self.refresh_sessions();
            }
            (PendingAction::Resume(_session), Err(_error)) if self.scratch_opening => {
                // The remembered blank session is gone (cleaned up or archived);
                // start a fresh one instead of showing an empty read-only view.
                self.scratch_opening = false;
                self.scratch_session = None;
                self.save_workspace_preferences(cx);
                self.new_session(w, cx);
            }
            (PendingAction::Resume(session), Err(error)) => {
                // A writer elsewhere can hold the lease while its transcript remains readable.
                self.opening_session_events.clear();
                self.open_conversation(session, None, true, None, w, cx);
                self.conversation.error = Some(if error.contains("already owned") {
                    "该会话正被另一个实例占用；已载入只读历史，关闭占用实例后可继续对话。".into()
                } else {
                    error
                });
            }
            (PendingAction::New { .. }, Err(error)) => {
                self.opening_session_events.clear();
                self.error = Some(error);
            }
        }
    }

    /// Ask the host to read every user-message image the transcript still lacks.
    ///
    /// The Harness logs attachment references, not pixels, so each bubble needs
    /// one `read_attachment` round trip before it can show the picture.
    fn request_user_images(&mut self) {
        for entry in self
            .conversation
            .entries
            .iter()
            .filter(|entry| entry.role == crate::state::EntryRole::User)
        {
            for image in crate::state::user_images(entry) {
                if image.data_url.is_some()
                    || !self
                        .requested_attachments
                        .insert(image.attachment_id.clone())
                {
                    continue;
                }
                invoke(
                    "read_attachment",
                    json!({
                        "attachmentId": image.attachment_id,
                        "mediaType": image.media_type,
                    }),
                );
            }
        }
    }

    /// Request every absolute workspace picture referenced by Markdown prose.
    ///
    /// Official `MarkdownPathImages` resolves absolute image destinations through
    /// the Host file API; here each new path is read once and cached, so the
    /// renderer can show the picture without side effects during paint.
    fn preload_markdown_images(&mut self) {
        for entry in &self.conversation.entries {
            if entry.text.is_empty() || !entry.text.contains("![") {
                continue;
            }
            for destination in crate::markdown::image_destinations(&entry.text) {
                let Some(path) = crate::media::absolute_image_path(&destination) else {
                    continue;
                };
                if crate::media::request_local_image(&path) {
                    invoke("read_image_file", json!({ "path": path }));
                }
            }
        }
    }

    fn request_session_history(&mut self) {
        let Some(session_id) = self.active_session.clone() else {
            return;
        };
        self.conversation.history_loading = true;
        self.conversation.history_prepend_pending = false;
        invoke(
            "load_session_history",
            json!({
                "sessionId": session_id,
                "cwd": self.workspace,
                "limit": 2500,
            }),
        );
    }

    pub(crate) fn request_older_history(&mut self, cx: &mut Context<Self>) {
        if self.conversation.history_loading
            || !self.conversation.history_has_more
            || self.creating_session
        {
            return;
        }
        let Some(session_id) = self.active_session.clone() else {
            return;
        };
        let Some(before_seq) = self.conversation.history_oldest_seq else {
            self.conversation.history_has_more = false;
            return;
        };
        self.conversation.history_loading = true;
        self.conversation.history_prepend_pending = true;
        invoke(
            "load_session_history",
            json!({
                "sessionId": session_id,
                "cwd": self.workspace,
                "beforeSeq": before_seq,
                "limit": 800,
            }),
        );
        cx.notify();
    }

    fn apply_session_history(&mut self, session: &str, reply: &Value, prepend: bool) {
        let events = reply
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let has_more = reply
            .get("hasMore")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut projected = crate::history::project_session_events_with_more(&events, has_more);
        if let Some(time) = projected.last_prompt_at_ms {
            self.record_session_prompt_time(session, time);
        }
        if let Some((seq, title)) = &projected.session_title {
            self.apply_durable_session_title(session, title, *seq);
        }
        let Some(conversation) = self.conversation_mut(session) else {
            return;
        };
        conversation.history_loading = false;
        for event in &events {
            conversation.turn_tails.observe_history(event);
            conversation.turn_process.observe_history(event);
        }
        conversation.history_has_more = projected.has_more;
        // The catalog is independent of transcript rows: a parent whose history
        // page holds only subagent facts must still list its children.
        for row in std::mem::take(&mut projected.subagents) {
            crate::subagents::upsert_subagent(&mut conversation.subagents, row);
        }
        // Pages can arrive newest-first; the catalog reads in discovery order.
        conversation
            .subagents
            .sort_by_key(|row| row.created_at_ms.unwrap_or(u64::MAX));
        if projected.entries.is_empty() {
            return;
        }
        for entry in &projected.entries {
            conversation.reconcile_assistant_stream(
                entry.external_id.as_deref(),
                entry.role,
                &entry.key,
            );
        }
        // A call can start on an earlier history page or settle while hydration is in flight.
        let tools: std::collections::HashMap<_, _> = projected
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.role == EntryRole::Tool)
            .map(|(index, entry)| (entry.key.clone(), index))
            .collect();
        let recent = std::mem::take(&mut conversation.entries);
        for entry in recent {
            if entry.role == EntryRole::Tool
                && let Some(&index) = tools.get(&entry.key)
            {
                crate::tool_calls::merge_tool_observation(&mut projected.entries[index], entry);
                continue;
            }
            let duplicate = !prepend
                && entry.external_id.as_ref().is_some_and(|id| {
                    projected.entries.iter().any(|stored| {
                        stored.role == entry.role && stored.external_id.as_ref() == Some(id)
                    })
                });
            if !duplicate {
                projected.entries.push(entry);
            }
        }
        conversation.entries = projected.entries;
        if !prepend {
            conversation.follow_tail = true;
            conversation.scroll.scroll_to_bottom();
        }
        if conversation.usage.is_none()
            && let Some(usage) = projected.usage
        {
            conversation.usage = Some(usage);
        }
        conversation.uncached_input_tokens = projected.uncached_input_tokens;
        conversation.cache_read_tokens = projected.cache_read_tokens;
        conversation.cache_write_tokens = projected.cache_write_tokens;
        conversation.output_tokens = projected.output_tokens;
        if projected.decode_tokens > 0 {
            conversation.decode_tokens = projected.decode_tokens;
        }
        if projected.decode_ms > 0. {
            conversation.decode_ms = projected.decode_ms;
        }
        if projected.turns > 0 {
            conversation.stats_turns = projected.turns;
        }
        if projected.steps > 0 {
            conversation.stats_steps = projected.steps;
        }
        if let Some(stats) = reply.get("sessionStats") {
            if let Some(turns) = stats.get("turns").and_then(Value::as_u64) {
                conversation.stats_turns = turns;
            }
            if let Some(steps) = stats.get("steps").and_then(Value::as_u64) {
                conversation.stats_steps = steps;
            }
            if let Some(ms) = stats.get("llmMs").and_then(Value::as_f64) {
                conversation.stats_llm_ms = ms;
            }
            if let Some(ms) = stats.get("toolMs").and_then(Value::as_f64) {
                conversation.stats_tool_ms = ms;
            }
            if let Some(ms) = stats.get("ttftMs").and_then(Value::as_f64) {
                conversation.stats_ttft_ms = ms;
            }
            if let Some(steps) = stats.get("ttftSteps").and_then(Value::as_u64) {
                conversation.stats_ttft_steps = steps;
            }
            if let Some(ms) = stats.get("decodeMs").and_then(Value::as_f64) {
                if ms > 0. {
                    conversation.decode_ms = ms;
                }
            }
            if let Some(tokens) = stats
                .get("decodeTokens")
                .and_then(Value::as_u64)
                .or_else(|| {
                    stats
                        .get("decodeTokens")
                        .and_then(Value::as_f64)
                        .map(|v| v as u64)
                })
            {
                if tokens > 0 {
                    conversation.decode_tokens = tokens;
                }
            }
        }
        // Official tokenUsage projection (whole-log totals) wins over window fold.
        if let Some(usage) = reply.get("tokenUsage") {
            let read_u64 = |key: &str| {
                usage
                    .get(key)
                    .and_then(Value::as_u64)
                    .or_else(|| usage.get(key).and_then(Value::as_f64).map(|v| v as u64))
                    .unwrap_or(0)
            };
            let uncached = read_u64("uncachedInputTokens");
            let cache_read = read_u64("cacheReadTokens");
            let cache_write = read_u64("cacheWriteTokens");
            let output = read_u64("outputTokens");
            if uncached + cache_read + cache_write + output > 0 {
                conversation.uncached_input_tokens = uncached;
                conversation.cache_read_tokens = cache_read;
                conversation.cache_write_tokens = cache_write;
                conversation.output_tokens = output;
                let billed = uncached + cache_read + cache_write + output;
                let window = conversation
                    .usage
                    .map(|(_, size)| size)
                    .filter(|size| *size > billed)
                    .unwrap_or(billed.max(1));
                conversation.usage = Some((billed, window));
            }
        }
        if let Some(oldest) = projected.oldest_seq {
            conversation.history_oldest_seq = Some(match conversation.history_oldest_seq {
                Some(current) if prepend => current.min(oldest),
                _ => oldest,
            });
        }
    }

    fn request_session_titles(&mut self) {
        let missing: Vec<Value> = self
            .sessions
            .iter()
            .filter(|session| session.title.is_none())
            .map(|session| {
                json!({
                    "sessionId": session.id,
                    "cwd": session.cwd,
                })
            })
            .collect();
        if missing.is_empty() {
            return;
        }
        invoke("peek_session_titles", json!({ "sessions": missing }));
    }

    fn apply_session_titles(&mut self, reply: &Value) {
        let Some(map) = reply.as_object() else {
            return;
        };
        for (id, value) in map {
            let Some(title) = value
                .as_str()
                .map(str::trim)
                .filter(|title| !title.is_empty())
            else {
                continue;
            };
            if let Some(session) = self
                .sessions
                .iter_mut()
                .find(|session| session.id == *id && session.title.is_none())
            {
                session.title = Some(title.to_string());
            }
        }
    }

    fn apply_durable_session_title(&mut self, id: &str, title: &str, seq: u64) {
        if let Some(session) = self.sessions.iter_mut().find(|session| session.id == id)
            && self
                .session_title_seqs
                .get(id)
                .is_none_or(|&latest| seq > latest)
        {
            self.session_title_seqs.insert(id.to_owned(), seq);
            session.title = Some(title.to_owned());
        }
    }

    /// Enter 提交入口：空闲即发送；生成中按官方行为投递到 Host Inbox。
    fn submit_prompt(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.submit_prompt_gesture(crate::submit::SubmitGesture::Enter, w, cx);
    }

    /// Cmd/Ctrl+Enter 提交入口：繁忙时使用与 Enter 互补的投递方式。
    fn submit_prompt_accelerated(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.submit_prompt_gesture(crate::submit::SubmitGesture::Accelerated, w, cx);
    }

    fn submit_prompt_gesture(
        &mut self,
        gesture: crate::submit::SubmitGesture,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.conversation.permission_mode.changing()
            || self.confirm_permission.is_some()
            || self.conversation.command_busy
        {
            return;
        }
        // Client-owned commands open their own surface. Resolving the built-in
        // token (rather than comparing the raw line) keeps the Chinese spellings
        // working: `/权限` previously fell through to the model as plain text.
        // An argument switches the preset directly, matching the official
        // handler (`/permission <preset>`), and still runs the confirmation gate
        // for the dangerous presets.
        // `/model` is a KWork extension rather than an official builtin, so it is
        // matched literally alongside them.
        let draft = self.composer.read(cx).value().trim().to_owned();
        let resolved = resolve_command_token(&draft);
        let argument = resolved
            .as_ref()
            .filter(|(name, _)| name == "permission")
            .map(|(_, rest)| rest.trim().to_owned());
        let menu = match argument.as_deref() {
            Some("") => Some(ComposerMenu::PermissionChoices),
            _ if matches!(draft.as_str(), "/model") => Some(ComposerMenu::CommandModels),
            _ => None,
        };
        if let Some(command) = menu {
            self.open_command_picker(command, true, w, cx);
            cx.notify();
            return;
        }
        if let Some(preset) = argument.filter(|preset| !preset.is_empty()) {
            self.replace_composer_without_trigger("", w, cx);
            self.choose_permission(preset.to_owned(), w, cx);
            return;
        }
        if self.submit_feedback_command(w, cx)
            || self.submit_export_command(w, cx)
            || self.submit_plan_command(w, cx)
            || self.submit_host_command(w, cx)
        {
            return;
        }
        // A prompt the Host has not settled yet still owns the single ACP
        // prompt slot even after its turn ended, so a direct `session/prompt`
        // would be rejected. Such a message goes through the Inbox as well.
        if self.conversation.running || self.conversation.prompt_inflight {
            self.queue_prompt(gesture, w, cx);
        } else {
            self.send_prompt(w, cx);
        }
    }

    /// 把 `/goal`、`/compact`、`/permission` 等及中文别名交给会话的 ACP 命令目录执行。
    /// `/plan` 由 `submit_plan_command` 处理，因为它还要提交可选的后续消息；
    /// 纯客户端的命令（如 `/export`）在这里返回 `false`，交给后面的提交路径。
    fn submit_host_command(&mut self, w: &mut Window, cx: &mut Context<Self>) -> bool {
        let draft = self.composer.read(cx).value().to_owned();
        let Some((name, line)) = canonical_host_command(&draft) else {
            return false;
        };
        if name == "plan" || client_side_command(&name) {
            return false;
        }
        if !self.command_catalog.supports(&name) {
            self.conversation.error = Some("当前会话暂不支持该指令。".to_owned());
            cx.notify();
            return true;
        }
        let attachment_ids = self
            .conversation
            .draft_attachments
            .iter()
            .map(|item| item.id.clone())
            .collect();
        self.run_host_command(
            line,
            Some(CommandSubmission {
                draft,
                attachment_ids,
            }),
            w,
            cx,
        );
        true
    }

    /// 执行无输入命令（菜单里的 `/compact`）或已经带齐参数的命令。
    /// 命令未成功前保留草稿和附件，与 Harness 的 admission 行为一致。
    pub(super) fn run_host_command(
        &mut self,
        line: String,
        submission: Option<CommandSubmission>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.active_session.clone() else {
            self.error = Some("请先打开会话再执行指令。".to_owned());
            cx.notify();
            return;
        };
        if !self.connected() || !self.conversation.acp_open {
            self.conversation.error = Some("会话尚未连接，指令未执行。".to_owned());
            cx.notify();
            return;
        }
        if self.conversation.command_busy {
            return;
        }
        self.forget_scratch_session(&session, cx);
        let name = line
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned();
        let accepts_attachments = self.command_catalog.accepts_attachments(&name);
        let attachments = if submission.is_some() {
            match command_attachment_payload(
                &self.conversation.draft_attachments,
                accepts_attachments,
            ) {
                Ok(attachments) => attachments,
                Err(error) => {
                    self.conversation.error = Some(error);
                    cx.notify();
                    return;
                }
            }
        } else {
            Vec::new()
        };
        self.conversation.command_busy = true;
        self.conversation.error = None;
        self.composer_menu = None;
        cx.notify();
        let args = json!({
            "sessionId": session,
            "line": line,
            "attachments": attachments,
        });
        cx.spawn_in(w, async move |this, cx| {
            let result = bridge::invoke("execute_session_command", args).await;
            let _ = this.update_in(cx, |this, w, cx| {
                let selected = this.active_session.as_deref() == Some(session.as_str());
                let Some(conversation) = this.conversation_mut(&session) else {
                    return;
                };
                conversation.command_busy = false;
                match result {
                    // The command card comes from the session event stream, so the
                    // RPC reply only has to settle the pending state.
                    Ok(_reply) => {
                        conversation.error = None;
                        if let Some(CommandSubmission {
                            ref draft,
                            ref attachment_ids,
                        }) = submission
                        {
                            conversation
                                .draft_attachments
                                .retain(|item| !attachment_ids.contains(&item.id));
                            if !selected && conversation.draft == *draft {
                                conversation.draft.clear();
                            }
                        }
                        if selected
                            && submission
                                .as_ref()
                                .is_some_and(|item| this.composer.read(cx).value() == item.draft)
                        {
                            this.replace_composer_without_trigger("", w, cx);
                        }
                        // The result is not toasted: the bridge forwards the
                        // session's `command/run` + `command/done` events, and the
                        // transcript renders them as a command card (official
                        // GenericCommandCard). A toast would float the same text
                        // over the composer and vanish after 4s.
                    }
                    Err(error) => conversation.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 繁忙时提交：投递到 Host Inbox（官方 `ComposerSubmissionPolicy`）。
    ///
    /// `queue` 等当前回合结束后再开一个新回合；`steer` 在当前回合的下一次
    /// 步骤边界插入。消息一旦被 Host 接受就归它管：客户端只在 dock 里保留
    /// 一行预览，关掉应用后由 `inbox_state` 补回。
    fn queue_prompt(
        &mut self,
        gesture: crate::submit::SubmitGesture,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.connected() || self.creating_session {
            return;
        }
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() && self.conversation.draft_attachments.is_empty() {
            return;
        }
        let Some(session) = self.active_session.clone() else {
            self.error = Some("会话尚未就绪，请稍后重试。".to_string());
            cx.notify();
            return;
        };
        // 兼容官方 `resolveSubmitMode`：会话无法插话（或已空闲）时一律排队；
        // 具体落点仍由 Host 决定，因为它才是回合状态的所有者。
        let mode = crate::submit::resolve_submit_mode(
            crate::submit::DEFAULT_BUSY_ENTER,
            self.conversation.running,
            gesture,
            self.conversation.acp_open,
        );
        let (display, prompt) = build_prompt_payload(&text, &self.conversation.draft_attachments);
        let draft = self.composer.read(cx).value().to_owned();
        let attachment_ids: Vec<String> = self
            .conversation
            .draft_attachments
            .iter()
            .map(|item| item.id.clone())
            .collect();
        self.dispatch_inbox_prompt(
            session,
            display,
            prompt,
            mode,
            Some(CommandSubmission {
                draft,
                attachment_ids,
            }),
            w,
            cx,
        );
    }

    /// 把一条消息投递到 Host Inbox，并在成功后按需清理草稿与附件。
    ///
    /// 官方 admission 语义：投递失败时草稿和附件都留在原处，用户可以重试。
    fn dispatch_inbox_prompt(
        &mut self,
        session: String,
        display: String,
        prompt: Vec<Value>,
        mode: crate::submit::SubmitMode,
        submission: Option<CommandSubmission>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.forget_scratch_session(&session, cx);
        self.error = None;
        self.conversation.error = None;
        self.page = WorkspacePage::Conversation;
        cx.notify();
        cx.spawn_in(w, async move |this, cx| {
            let result = bridge::invoke(
                "queue_prompt",
                json!({
                    "sessionId": session,
                    "prompt": prompt,
                    "mode": mode.as_str(),
                }),
            )
            .await;
            let _ = this.update_in(cx, |this, w, cx| {
                let selected = this.active_session.as_deref() == Some(session.as_str());
                let Some(conversation) = this.conversation_mut(&session) else {
                    return;
                };
                match result {
                    Ok(reply) => {
                        if let Some(item_id) = reply["itemId"].as_str() {
                            let accepted =
                                crate::submit::SubmitMode::from_wire(reply["mode"].as_str());
                            conversation.push_inbox_row(item_id.to_owned(), accepted, display);
                        }
                        // 只有真正投递成功才清空草稿与附件。
                        if let Some(CommandSubmission {
                            ref draft,
                            ref attachment_ids,
                        }) = submission
                        {
                            conversation
                                .draft_attachments
                                .retain(|item| !attachment_ids.contains(&item.id));
                            if !selected && conversation.draft == *draft {
                                conversation.draft.clear();
                            }
                            if selected && this.composer.read(cx).value() == *draft {
                                this.replace_composer_without_trigger("", w, cx);
                            }
                        }
                    }
                    Err(error) => conversation.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn send_prompt(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.connected()
            || !self.conversation.acp_open
            || self.conversation.running
            || self.conversation.prompt_inflight
            || self.creating_session
            || (self.conversation.history_loading && !self.conversation.history_prepend_pending)
        {
            return;
        }
        let Some(session_id) = self.active_session.clone() else {
            self.error = Some("会话尚未就绪，请稍后重试。".to_string());
            cx.notify();
            return;
        };
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() && self.conversation.draft_attachments.is_empty() {
            return;
        }
        let (display, prompt) = build_prompt_payload(&text, &self.conversation.draft_attachments);
        replace(&self.composer, "", w, cx);
        self.conversation.draft_attachments.clear();
        self.page = WorkspacePage::Conversation;
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            if session.title.is_none() {
                session.title = Some(short_session_title(&display));
            }
            session.blank = false;
        }
        self.error = None;
        self.start_prompt(&session_id, OutgoingPrompt { display, prompt }, cx);
        cx.notify();
    }

    fn start_prompt(&mut self, session: &str, prompt: OutgoingPrompt, cx: &mut Context<Self>) {
        // The blank startup session becomes a real conversation: stop reusing it.
        self.forget_scratch_session(session, cx);
        let defer_collapse = self.collapse_next_input;
        let Some(conversation) = self.conversation_mut(session) else {
            return;
        };
        conversation.error = None;
        if defer_collapse && !conversation.entries.is_empty() {
            conversation.flow_motion.new_input();
        }
        conversation.push_local(EntryRole::User, prompt.display);
        conversation.prompt_inflight = true;
        conversation.begin_turn();
        self.observe_local_session_activity(session, false, true);
        if self.active_session.as_deref() == Some(session) {
            self.start_turn_ticker(cx);
            self.pin_follow_tail();
        }
        invoke(
            "prompt",
            json!({"sessionId": session, "prompt": prompt.prompt}),
        );
    }

    fn start_turn_ticker(&mut self, cx: &mut Context<Self>) {
        if !self.conversation.running {
            return;
        }
        let now = bridge::monotonic_ms();
        // A previous ticker can exit when its `this.update` fails, leaving the
        // flag true forever and freezing the elapsed seconds. A stale
        // heartbeat means it is dead: restart instead of blocking every turn.
        if self.turn_ticker_running && now - self.turn_ticker_heartbeat < 3_000. {
            return;
        }
        self.turn_ticker_running = true;
        self.turn_ticker_heartbeat = now;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let live = this
                    .update(cx, |app, cx| {
                        let now = bridge::monotonic_ms();
                        app.turn_ticker_heartbeat = now;
                        // A response can stop mid-stream without any end or
                        // error event; end the turn so it cannot hang forever.
                        if app.conversation.running
                            && crate::validation::turn_stalled(
                                now,
                                app.conversation.last_event_ms,
                                crate::validation::TURN_STALL_MS,
                            )
                        {
                            app.conversation.finish_turn(TurnEnd::Failed);
                            app.push_toast("长时间未收到响应，已停止等待，请重试。", cx);
                        }
                        let live = app.conversation.running;
                        app.turn_ticker_running = live;
                        if live {
                            cx.notify();
                        }
                        live
                    })
                    .unwrap_or(false);
                if !live {
                    break;
                }
            }
        })
        .detach();
    }

    fn finish_session_turn(&mut self, session: &str, end: TurnEnd, _cx: &mut Context<Self>) {
        let Some(conversation) = self.conversation_mut(session) else {
            return;
        };
        let previous = conversation.running;
        conversation.finish_turn(end);
        if end == TurnEnd::MaxTokens {
            if let Some(turn) = conversation.last_turn_number() {
                conversation.push_turn_notice(crate::state::turn_max_tokens_entry(turn, None));
            }
        }
        // Pending messages belong to the Host Inbox now: it opens the next turn
        // on its own, so the client must not flush anything here. A failed
        // prompt keeps its items queued for the user to review or remove.
        self.observe_local_session_activity(session, previous, false);
        if self.active_session.as_deref() == Some(session) {
            self.maybe_follow_tail();
        }
    }

    fn handle_prompt_reply(
        &mut self,
        session: &str,
        result: Result<Value, String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.active_session.as_deref() == Some(session);
        let Some(conversation) = self.conversation_mut(session) else {
            return;
        };
        if !std::mem::take(&mut conversation.prompt_inflight) {
            return;
        }
        let end = match result {
            Ok(reply) => match reply["stopReason"].as_str() {
                Some("max_tokens") => TurnEnd::MaxTokens,
                Some("cancelled") => TurnEnd::Stopped,
                _ => TurnEnd::Completed,
            },
            Err(error) => {
                let draft = conversation
                    .entries
                    .last()
                    .filter(|entry| entry.role == EntryRole::User && entry.external_id.is_none())
                    .map(|entry| entry.text.clone());
                if let Some(draft) = draft {
                    conversation.entries.pop();
                    if selected {
                        replace(&self.composer, draft, w, cx);
                    } else if conversation.draft.is_empty() {
                        conversation.draft = draft;
                    } else {
                        conversation.draft = format!("{draft}\n\n{}", conversation.draft);
                    }
                }
                if let Some(conversation) = self.conversation_mut(session) {
                    conversation.error = Some(prompt_failure_message(&error));
                }
                TurnEnd::Failed
            }
        };
        // A queued message can start the next turn before this ACP request
        // settles, so the durable event stream may already have closed the
        // turn. Ending it again would add a phantom turn summary.
        if self
            .conversation_mut(session)
            .is_some_and(|conversation| !conversation.running)
        {
            if selected {
                self.maybe_follow_tail();
            }
            cx.notify();
            return;
        }
        self.finish_session_turn(session, end, cx);
    }

    pub(crate) fn pick_attachments(&mut self, accept: &'static str, cx: &mut Context<Self>) {
        let owner = self.active_session.clone();
        cx.spawn(
            async move |this, cx| match bridge::pick_files(accept).await {
                Ok(raw) => {
                    let Ok(items) = serde_json::from_str::<Vec<Value>>(&raw) else {
                        return;
                    };
                    let _ = this.update(cx, |app, cx| {
                        if let Some(conversation) =
                            owner.as_deref().and_then(|id| app.conversation_mut(id))
                        {
                            conversation.ingest_draft_items(&items);
                        }
                        cx.notify();
                    });
                }
                Err(error) => {
                    let _ = this.update(cx, |app, cx| {
                        if let Some(conversation) =
                            owner.as_deref().and_then(|id| app.conversation_mut(id))
                        {
                            conversation.error = Some(error);
                        }
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    pub(crate) fn handle_file_event(&mut self, payload: &str, cx: &mut Context<Self>) {
        let Ok(event) = serde_json::from_str::<Value>(payload) else {
            return;
        };
        match event.get("kind").and_then(Value::as_str) {
            Some("drag") => {
                let active = event
                    .get("active")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if self.drop_active != active {
                    self.drop_active = active;
                    cx.notify();
                }
            }
            Some("files") => {
                self.drop_active = false;
                if let Some(items) = event.get("items").and_then(Value::as_array) {
                    self.conversation.ingest_draft_items(items);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// 用户发送或点「回到底部」时恢复尾部跟随。
    pub(crate) fn pin_follow_tail(&mut self) {
        self.conversation.follow_tail = true;
        if self.conversation.flow_motion.owns_tail() {
            self.conversation.flow_motion.new_input();
        }
        self.maybe_follow_tail();
    }

    /// Settle a freshly loaded transcript at its latest row.
    ///
    /// History is applied before the scroll container has measured the rows, so
    /// the single `scroll_to_bottom()` in `apply_session_history` sees a
    /// zero-height content box and does nothing; the reopened session then sat at
    /// the oldest row. Pin the tail for the first few paints (the official client
    /// restores its tail anchor after the first paint for the same reason) and
    /// stop once two consecutive frames already sit at the bottom. Leaving the
    /// tail, closing the session, or opening another one ends the wait.
    fn settle_history_scroll(&mut self, session: &str, cx: &mut Context<Self>) {
        if self.active_session.as_deref() != Some(session) || !self.conversation.follow_tail {
            return;
        }
        let session = session.to_owned();
        cx.spawn(async move |this, cx| {
            let mut confirmations = 0;
            for _ in 0..10 {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;
                let at_tail = this.update(cx, |app, cx| {
                    if app.active_session.as_deref() != Some(session.as_str())
                        || !app.conversation.follow_tail
                    {
                        return None;
                    }
                    let max = app.conversation.scroll.max_offset().y;
                    if max <= px(0.) {
                        // Rows not measured yet: keep waiting.
                        return Some(false);
                    }
                    if app.conversation.scroll.offset().y + max <= px(1.) {
                        return Some(true);
                    }
                    app.conversation.scroll.scroll_to_bottom();
                    cx.notify();
                    Some(false)
                });
                match at_tail.ok().flatten() {
                    Some(true) => {
                        confirmations += 1;
                        if confirmations >= 2 {
                            break;
                        }
                    }
                    Some(false) => confirmations = 0,
                    None => break,
                }
            }
        })
        .detach();
    }

    /// 流式更新：仅在仍跟随尾部时滚到底。
    pub(crate) fn maybe_follow_tail(&mut self) {
        if self.conversation.follow_tail && !self.conversation.flow_motion.owns_tail() {
            self.conversation.scroll.scroll_to_bottom();
        }
    }

    /// 根据当前偏移更新跟随权：贴底则恢复，离开阈值则解除。
    pub(crate) fn sync_follow_from_scroll(&mut self, delta: f32) {
        if self.conversation.flow_motion.owns_tail() {
            if delta > 0. {
                self.conversation.follow_tail = false;
                self.conversation.flow_motion.cancel_tail_request();
            }
            return;
        }
        let remaining = (self.conversation.scroll.max_offset().y
            + self.conversation.scroll.offset().y)
            .as_f32();
        self.conversation.follow_tail = remaining <= 25.;
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
        if let Some(permission) = self.conversation.permission.take() {
            invoke(
                "respond_permission",
                json!({"sessionId": self.active_session, "requestId": permission.request_id, "optionId": option_id}),
            );
            cx.notify();
        }
    }

    fn handle_reply(
        &mut self,
        command: &'static str,
        owner: Option<&str>,
        result: Result<Value, String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            command,
            "start_auth" | "auth_status" | "save_api_key" | "remove_api_key"
        ) {
            let was_configured = self.welcome.read(cx).configured();
            self.welcome.update(cx, |welcome, cx| {
                welcome.handle_reply(command, result, w, cx);
            });
            if command == "save_api_key"
                && !was_configured
                && self.welcome.read(cx).configured()
                && self.connected()
                && !self.conversation.running
                && !self.creating_session
                && self.conversation.entries.is_empty()
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
                        self.apply_archive_snapshot(&reply["workspaceArchive"]);
                        self.load_startup_model_providers(&reply, w, cx);
                        self.open_scratch_session(w, cx);
                    }
                    Err(error) => {
                        self.status = ServiceStatus::Error;
                        self.error = Some(error);
                        if self.show_settings && self.settings_page == SettingsPage::Providers {
                            self.open_model_providers(w, cx);
                        }
                    }
                }
            }
            "inbox_state" => self.finish_inbox_state(owner, result, w, cx),
            "goal_state" | "goal_action" => self.finish_goal_state(owner, result, w, cx),
            "fork_session" => self.finish_fork(result, w, cx),
            "rename_session" => self.finish_session_rename(result, w, cx),
            "new_session" | "resume_session" => self.finish_session_open(result, w, cx),
            "load_session_history" => {
                if let Some(session) = owner
                    && let Some(conversation) = self.conversation_mut(session)
                {
                    let prepend = std::mem::take(&mut conversation.history_prepend_pending);
                    match result {
                        Ok(reply) => {
                            self.apply_session_history(session, &reply, prepend);
                            self.request_user_images();
                            self.preload_markdown_images();
                            if !prepend {
                                self.settle_history_scroll(session, cx);
                            }
                        }
                        Err(error) => {
                            conversation.history_loading = false;
                            conversation.error = Some(format!("历史载入失败：{error}"));
                        }
                    }
                }
            }
            // One user-message picture finished decoding into a data URL: patch it
            // into every visible row that references the attachment.
            "read_attachment" => match result {
                Ok(reply) => {
                    if let (Some(id), Some(data_url)) = (
                        reply.get("attachmentId").and_then(Value::as_str),
                        reply.get("dataUrl").and_then(Value::as_str),
                    ) {
                        crate::state::set_user_image_data(
                            &mut self.conversation.entries,
                            id,
                            data_url,
                        );
                        cx.notify();
                    }
                }
                Err(_) => {}
            },
            // One Markdown workspace picture finished reading: cache it for the
            // renderer (failure keeps 图片无法预览 without a retry loop).
            "read_image_file" => match result {
                Ok(reply) => {
                    if let (Some(path), Some(media_type), Some(data_url)) = (
                        reply.get("path").and_then(Value::as_str),
                        reply.get("mediaType").and_then(Value::as_str),
                        reply.get("dataUrl").and_then(Value::as_str),
                    ) {
                        crate::media::store_local_image(path, media_type, data_url);
                        cx.notify();
                    }
                }
                Err(_) => {
                    crate::media::fail_pending_local_images();
                    cx.notify();
                }
            },
            "list_sessions" => match result {
                Ok(reply) => self.apply_session_page(&reply),
                Err(error) => {
                    // Still surface durable local history when ACP list fails.
                    self.error = Some(format!("会话列表加载失败：{error}"));
                    self.sessions_acp_complete = true;
                    invoke("list_local_sessions", json!({}));
                }
            },
            "list_local_sessions" => match result {
                Ok(reply) => self.apply_local_sessions(&reply),
                Err(_) => {
                    self.request_session_titles();
                    if self.sessions_acp_complete {
                        self.sessions_loading = false;
                    }
                }
            },
            "peek_session_titles" => match result {
                Ok(reply) => self.apply_session_titles(&reply),
                Err(error) => self.error = Some(format!("会话标题加载失败：{error}")),
            },
            "load_workspace_preferences" => match result {
                Ok(reply) => self.apply_workspace_preferences(reply, cx),
                Err(error) => self.error = Some(error),
            },
            "save_workspace_preferences" => {
                if let Err(error) = result {
                    self.error = Some(error);
                }
            }
            "pick_workspace_directory" => self.finish_workspace_pick(result, w, cx),
            "workspace_applications" => self.apply_workspace_applications(result, cx),
            "open_workspace_application" => self.finish_workspace_open(result, cx),
            "load_shortcuts" | "save_shortcuts" => {
                self.shortcut_reply(command == "save_shortcuts", result, w, cx)
            }
            "load_developer_tools" => {
                self.developer_tools_loading = false;
                if let Some(enabled) = result.ok().and_then(|reply| reply.as_bool()) {
                    self.update_html_preview_mode(enabled);
                }
            }
            "save_developer_tools" => {
                self.developer_tools_saving = false;
                if let Some(enabled) = result.ok().and_then(|reply| reply.as_bool()) {
                    self.developer_tools_switched_at = Some(bridge::monotonic_ms());
                    self.update_html_preview_mode(enabled);
                } else {
                    self.developer_tools_error = true;
                }
            }
            "prompt" => {
                if let Some(session) = owner {
                    self.handle_prompt_reply(session, result, w, cx);
                }
            }
            "set_config_option" => {
                if let Some(session) = owner
                    && let Some(conversation) = self.conversation_mut(session)
                {
                    match result {
                        Ok(reply) => {
                            conversation.config_options = reply["configOptions"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default();
                            if self.active_session.as_deref() == Some(session) {
                                self.rebuild_selectors(w, cx);
                            }
                        }
                        Err(error) => conversation.error = Some(error),
                    }
                }
            }
            "command_model" => self.finish_command_model(owner, result, w, cx),
            "button_model" => self.finish_model_menu(owner, result, w, cx),
            "browser_shell_generation" => match result {
                Ok(value) => {
                    self.browser_shell = value.as_u64();
                    document_html::republish_focus();
                }
                Err(error) => self.error = Some(error),
            },
            "plan_mode" => {
                if let Some(conversation) = owner.and_then(|session| self.conversation_mut(session))
                    && conversation.plan_active.is_none()
                {
                    conversation.plan_active =
                        result.ok().and_then(|value| value["active"].as_bool());
                }
            }
            "session_commands" => {
                if owner == self.active_session.as_deref() {
                    match result {
                        Ok(reply) => self.command_catalog.replace(&reply),
                        Err(_) => self.command_catalog.clear(),
                    }
                }
            }
            "permission_mode" => self.finish_permission_mode(owner, result, w, cx),
            // A cold resume has no projection stream yet: the bridge returns the
            // same `rows` shape as its `continued` event, so rebuild the pending
            // question panels from the reply instead of dropping it.
            "question_state" => match result {
                Ok(reply) => {
                    if let (Some(session), Some(rows)) = (owner, reply.as_array()) {
                        self.apply_question_state(session, rows, w, cx);
                    }
                }
                Err(error) => {
                    if let Some(conversation) =
                        owner.and_then(|session| self.conversation_mut(session))
                    {
                        conversation.error = Some(format!("待回答问题恢复失败：{error}"));
                    }
                }
            },
            "cancel_session" | "respond_permission" => {
                if let Err(error) = result
                    && let Some(conversation) =
                        owner.and_then(|session| self.conversation_mut(session))
                {
                    conversation.error = Some(error);
                }
            }
            "stop_service"
            | "open_start_tool"
            | "write_terminal"
            | "resize_terminal"
            | "stop_terminal"
            | "close_embedded_browser"
            | "embedded_browser_action"
            | "open_browser_url" => {
                if let Err(error) = result {
                    self.error = Some(error);
                }
            }
            // Fire-and-forget commands still owe the user an error when the host
            // fails. Previously these fell through to `_ => {}` and silently
            // discarded both success and failure, leaving the UI out of sync
            // (notably `close_session`, which removes its row optimistically).
            "close_session"
            | "configure_shell_window"
            | "focus_browser_shell"
            | "show_document_copy_menu"
            | "close_shortcut_window"
            | "start_window_dragging" => {
                if let Err(error) = result {
                    self.push_toast(error, cx);
                }
            }
            // Keep future commands honest: never swallow an error silently.
            _ => {
                if let Err(error) = result {
                    self.push_toast(error, cx);
                }
            }
        }
        cx.notify();
    }

    fn handle_conversation_event(&mut self, event: Value, w: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = event["sessionId"].as_str() else {
            return;
        };
        let update = &event["update"];
        if update["sessionUpdate"] == "gpui_agent_status" {
            self.observe_host_session_activity(session, update);
            return;
        }
        if update["sessionUpdate"] == "gpui_session_activity" {
            if let Some(time) = update["time"].as_u64() {
                self.record_session_prompt_time(session, time);
            }
            return;
        }
        if update["sessionUpdate"] == "gpui_session_title" {
            if let (Some(title), Some(seq)) = (update["title"].as_str(), update["seq"].as_u64()) {
                self.apply_durable_session_title(session, title, seq);
            }
            return;
        }
        // Any ACP content event proves the turn is still alive. State-only
        // updates (agent status / title / activity) return before this point.
        if let Some(conversation) = self.conversation_mut(session) {
            conversation.last_event_ms = Some(bridge::monotonic_ms());
        }
        let defer_collapse = self.collapse_next_input;
        let selected = self.active_session.as_deref() == Some(session);
        // Inbox bookkeeping owns its own conversation lookup: a pending row can
        // be claimed for a session that is currently parked, and the row must
        // leave the dock either way.
        if update["sessionUpdate"] == "gpui_user_message" {
            self.apply_logged_user_message(session, update, cx);
            return;
        }
        if update["sessionUpdate"] == "gpui_inbox_changed" {
            self.request_inbox_state(session);
            return;
        }
        if update["sessionUpdate"] == "gpui_goal_changed" {
            self.handle_goal_changed(session, update, cx);
            return;
        }
        if update["sessionUpdate"] == "gpui_deliverables" {
            if let Some(conversation) = self.conversation_mut(session) {
                let files = update
                    .get("files")
                    .and_then(|files| files.as_array())
                    .cloned()
                    .unwrap_or_default();
                if !files.is_empty() {
                    conversation.pending_deliverables.extend(files);
                }
            }
            return;
        }
        if update["sessionUpdate"] == "gpui_subagent_catalog" {
            if let (Some(conversation), Some(row)) = (
                self.conversation_mut(session),
                crate::subagents::SubagentRow::from_event_data(update),
            ) {
                crate::subagents::upsert_subagent(&mut conversation.subagents, row);
                cx.notify();
            }
            return;
        }
        let Some(conversation) = self.conversation_mut(session) else {
            if self.creating_session {
                self.opening_session_events.push(event);
            }
            return;
        };
        if event["kind"] == "permission" {
            conversation.apply_permission(&event);
            if selected {
                self.maybe_follow_tail();
            }
            return;
        }
        if update["sessionUpdate"] == "gpui_plan_review" {
            self.handle_plan_review(session, update, w, cx);
            return;
        }
        if update["sessionUpdate"] == "gpui_questions" {
            self.handle_questions(session, update, w, cx);
            return;
        }
        if update["sessionUpdate"] == "gpui_plan_mode" {
            conversation.plan_active = update["active"].as_bool();
            return;
        }
        if update["sessionUpdate"] == "gpui_assistant_stream" {
            if conversation.running {
                conversation.assistant_stream.update(update);
                if selected {
                    self.maybe_follow_tail();
                }
            }
            return;
        }
        if update["sessionUpdate"] == "gpui_turn_tail" {
            if let (Some(id), Some(time)) = (
                update["toolCall"]["toolCallId"].as_str(),
                update["toolCall"]["time"].as_f64(),
            ) && let Some(entry) = conversation.entries.iter_mut().find(|entry| {
                entry.role == EntryRole::Tool
                    && !entry.is_subcall()
                    && entry.external_id.as_deref() == Some(id)
            }) {
                entry.at_ms = Some(time);
            }
            if let (Some(turn), Some(plans)) =
                (update["turn"].as_u64(), update["planEvents"].as_array())
            {
                let mut seen = std::collections::HashSet::new();
                let plans: Vec<_> = plans
                    .iter()
                    .filter_map(crate::submitted_plan::SubmittedPlan::from_event)
                    .filter(|plan| seen.insert(plan.call_id.clone()))
                    .collect();
                if let Some(last) = plans.last() {
                    conversation.turn_tails.note_plan_tail(turn, &last.call_id);
                    conversation.submitted_plans.insert(turn, plans);
                }
            }
            conversation.turn_tails.observe_live(update);
            conversation.turn_process.observe_live(update);
            return;
        }
        if update["sessionUpdate"] == "turn_end"
            && let Some(plans) = update["turn"]
                .as_u64()
                .and_then(|turn| conversation.submitted_plans.remove(&turn))
        {
            for plan in plans {
                if !conversation.entries.iter().any(|entry| {
                    entry.role == EntryRole::Plan
                        && entry.external_id.as_deref() == Some(&plan.call_id)
                }) {
                    conversation.entries.push(ChatEntry::submitted_plan(plan));
                }
            }
        }
        let effect = apply_update(&mut conversation.entries, update);
        if update["content"]["text"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
        {
            let role = match update["sessionUpdate"].as_str() {
                Some("agent_thought_chunk") => Some(EntryRole::Thought),
                Some("agent_message_chunk") => Some(EntryRole::Assistant),
                _ => None,
            };
            if let (Some(role), Some(id)) = (role, update["messageId"].as_str())
                && let Some(entry) =
                    conversation.entries.iter().rev().find(|entry| {
                        entry.role == role && entry.external_id.as_deref() == Some(id)
                    })
            {
                let key = entry.key.clone();
                conversation.reconcile_assistant_stream(Some(id), role, &key);
            }
        }
        if update["sessionUpdate"] == "tool_call"
            && let Some(id) = update["toolCallId"].as_str()
            && let Some(time) = conversation.turn_process.tool_time(id)
            && let Some(entry) = conversation.entries.iter_mut().find(|entry| {
                entry.role == EntryRole::Tool
                    && !entry.is_subcall()
                    && entry.external_id.as_deref() == Some(id)
            })
        {
            entry.at_ms = Some(time as f64);
        }
        let now = bridge::now_ms();
        for entry in &mut conversation.entries {
            if entry.at_ms.is_none() {
                entry.at_ms = Some(now);
            }
        }
        if let Some(usage) = effect.usage {
            conversation.usage = Some(usage);
        }
        let options_changed = effect.config_options.is_some();
        if let Some(options) = effect.config_options {
            conversation.config_options = options;
        }
        let started = effect.turn_triggered && !conversation.running;
        if started {
            if defer_collapse && conversation.turn_summary.is_some() {
                conversation.flow_motion.new_input();
            }
            conversation.begin_turn();
        }
        if let Some(reason) = effect.turn_ended.as_deref()
            && conversation.running
        {
            use crate::history::turn_process::EndKind;
            let durable_end = update["turn"]
                .as_u64()
                .and_then(|turn| conversation.turn_process.end_kind(turn));
            let end = match (durable_end, reason) {
                (Some(EndKind::Stopped), _)
                | (_, "cancelled" | "cancel" | "aborted" | "interrupted") => TurnEnd::Stopped,
                (Some(EndKind::Failed), _) | (_, "error" | "failed") => TurnEnd::Failed,
                (_, "max_tokens") => TurnEnd::MaxTokens,
                _ => TurnEnd::Completed,
            };
            // Official TurnErrorItem: a terminal failure records why the turn
            // stopped, so a quota or auth problem is not just "处理失败".
            if let Some(turn) = update["turn"].as_u64()
                && let Some(entry) = crate::state::turn_failure_entry(
                    turn,
                    &update["reasonObject"],
                    update["time"].as_f64(),
                )
                && !conversation
                    .entries
                    .iter()
                    .any(|existing| existing.key == entry.key)
            {
                conversation.entries.push(entry);
            }
            self.finish_session_turn(session, end, cx);
        }
        if started {
            self.observe_local_session_activity(session, false, true);
        }
        if effect.turn_triggered
            && let Some(summary) = self
                .sessions
                .iter_mut()
                .find(|summary| summary.id == session)
        {
            summary.blank = false;
        }
        // Markdown pictures inside streamed or finalized prose are Host files:
        // read each new absolute destination once so the next paint can show it.
        self.preload_markdown_images();
        if selected {
            if options_changed {
                self.rebuild_selectors(w, cx);
            }
            self.start_turn_ticker(cx);
            self.maybe_follow_tail();
        }
    }

    fn handle_event(&mut self, event: Value, w: &mut Window, cx: &mut Context<Self>) {
        if event["kind"] == "auth" {
            self.welcome
                .update(cx, |welcome, cx| welcome.handle_event(&event, cx));
            cx.notify();
            return;
        }
        match event.get("kind").and_then(Value::as_str) {
            Some("shell_fullscreen") => {
                self.native_fullscreen = event["value"].as_bool().unwrap_or(false);
            }
            Some("session_log_download_started") => self.session_downloads.started(&event),
            Some("plugin_install") => self.plugin_install_event(&event, w, cx),
            Some("schedule_changed") => self.invalidate_automations(cx),
            Some("schedule_session") => {
                self.apply_local_sessions(&json!({"sessions": [event["session"]]}));
            }
            Some("service") => match event.get("status").and_then(Value::as_str) {
                Some("starting") => self.status = ServiceStatus::Starting,
                Some("running") => self.status = ServiceStatus::Connected,
                Some("exited") => {
                    self.status = ServiceStatus::Error;
                    self.service_starting = false;
                    self.session_activity.clear();
                    self.running_subagents.clear();
                    for conversation in std::iter::once(&mut self.conversation)
                        .chain(self.parked_conversations.values_mut())
                    {
                        conversation.acp_open = false;
                        conversation.permission = None;
                        conversation.plan_reviews.clear();
                        conversation.questions.clear();
                        conversation.submitted_plans.clear();
                        conversation.plan_active = None;
                        conversation.plan_mode_busy = false;
                        if conversation.running {
                            conversation.finish_turn(TurnEnd::Failed);
                        }
                    }
                    self.error = event
                        .get("detail")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| Some("dsh-acp 已退出".to_string()));
                }
                _ => {}
            },
            Some("session_update" | "permission") => self.handle_conversation_event(event, w, cx),
            Some("workspace_directory_changed") => {
                self.workspace_directory_changed(&event, cx);
                return;
            }
            Some("workspace_file_changed") => {
                self.workspace_file_changed(event, cx);
                return;
            }
            Some("main_shortcut_input" | "shortcut_record_input") => {
                self.shortcut_input_event(&event, w, cx);
                return;
            }
            Some("html_preview_input") => {
                self.handle_html_preview_input(&event, w, cx);
                return;
            }
            Some(
                "browser_page" | "browser_popup" | "browser_focus" | "browser_input"
                | "browser_terminated",
            ) => {
                self.handle_browser_event(&event, w, cx);
                return;
            }
            Some("terminal") => {
                let Some(id) = event
                    .get("terminalId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                else {
                    return;
                };
                let Ok(update) = serde_json::from_value::<terminal::TerminalUpdate>(event) else {
                    return;
                };
                for (owner, sidebar) in self
                    .active_session
                    .iter()
                    .zip(std::iter::once(&mut self.sidebar))
                    .chain(self.parked_sidebars.iter_mut())
                {
                    let Some((&key, tab)) =
                        sidebar.terminals.iter_mut().find(|(_, tab)| tab.id == id)
                    else {
                        continue;
                    };
                    match tab.connection {
                        TerminalConnection::Opening { .. } => tab.pending.push(update),
                        TerminalConnection::Connected | TerminalConnection::Exited { .. } => {
                            if tab.apply_update(update, cx)
                                && let Some(saved) = sidebar.surface.terminals.get_mut(&key)
                            {
                                saved.title = tab.title.clone();
                                sidebar.persist(owner);
                            }
                        }
                        _ => {}
                    }
                    break;
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn connect_terminal(
        &mut self,
        key: u64,
        create: bool,
        requested_shell: Option<String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(owner) = self.active_session.clone() else {
            return;
        };
        let Some(checkpoint) = self.sidebar.surface.terminals.get(&key) else {
            return;
        };
        if self.sidebar.terminals.contains_key(&key) {
            return;
        }
        let terminal_id = checkpoint.id.clone();
        let reading_shells = create && requested_shell.is_none();
        let screen = cx.new(|cx| terminal::TerminalScreen::new(w, cx));
        self.sidebar.terminals.insert(
            key,
            TerminalTab {
                id: terminal_id.clone(),
                title: checkpoint.title.clone(),
                screen,
                requested_shell: requested_shell.clone(),
                connection: TerminalConnection::Opening {
                    create,
                    reading_shells,
                },
                sequence: 0,
                pending: Vec::new(),
            },
        );

        let (command, mut args) = if create {
            (
                "start_terminal",
                json!({
                    "terminalId": terminal_id, "session": owner, "cwd": self.sidebar.workspace,
                    "cols": 80, "rows": 24,
                }),
            )
        } else {
            (
                "attach_terminal",
                json!({"terminalId": terminal_id, "session": owner}),
            )
        };
        cx.spawn_in(w, async move |this, cx| {
            let choice = if reading_shells {
                terminal_shell_menu::load_shells()
                    .await
                    .map(|shells| terminal_shell_menu::preferred_path(&shells).map(str::to_owned))
            } else {
                Ok(requested_shell)
            };
            let mut result = match choice {
                Ok(choice) => {
                    if create {
                        let mounted = this.update_in(cx, |app, _, cx| {
                            let Some(tab) = app
                                .sidebar_for_session(&owner)
                                .and_then(|sidebar| sidebar.terminals.get_mut(&key))
                                .filter(|tab| tab.id == terminal_id)
                            else {
                                return false;
                            };
                            if let Some(path) = &choice {
                                bridge::remember_terminal_shell(path);
                            }
                            tab.connection = TerminalConnection::Opening {
                                create,
                                reading_shells: false,
                            };
                            cx.notify();
                            true
                        });
                        if !mounted.unwrap_or(false) {
                            return;
                        }
                        args["shellPath"] = json!(choice);
                    }
                    bridge::invoke(command, args).await.and_then(|value| {
                        serde_json::from_value::<Option<terminal::TerminalSnapshot>>(value)
                            .map_err(|error| error.to_string())
                    })
                }
                Err(error) => Err(error),
            };
            let has_screen = matches!(result, Ok(Some(_)));
            if let Ok(Some(snapshot)) = &result {
                let restore = this.update_in(cx, |app, _, cx| {
                    let tab = app.sidebar_for_session(&owner)?.terminals.get_mut(&key)?;
                    if tab.id != terminal_id {
                        return None;
                    }
                    Some(tab.screen.update(cx, |screen, _| screen.restore(snapshot)))
                });
                match restore {
                    Ok(Some(promise)) => {
                        if wasm_bindgen_futures::JsFuture::from(promise).await.is_err() {
                            result = Err("终端画面传输异常，请重新连接。".into());
                        }
                    }
                    _ => return,
                }
            }
            let _ = this.update_in(cx, |app, w, cx| {
                let Some(sidebar) = app.sidebar_for_session(&owner) else {
                    return;
                };
                let Some(tab) = sidebar
                    .terminals
                    .get_mut(&key)
                    .filter(|tab| tab.id == terminal_id)
                else {
                    return;
                };
                match result {
                    Ok(Some(snapshot)) => {
                        tab.sequence = snapshot.sequence;
                        tab.title = Some(snapshot.title);
                        tab.connection = TerminalConnection::Connected;
                        tab.screen
                            .update(cx, |screen, cx| screen.restore_finished(cx));
                        if let Some(exit) = snapshot.exit {
                            tab.end(exit.code, cx);
                        }
                        let mut pending = std::mem::take(&mut tab.pending);
                        pending.sort_by_key(|update| update.sequence);
                        for update in pending {
                            tab.apply_update(update, cx);
                        }
                        if matches!(tab.connection, TerminalConnection::Connected) {
                            tab.screen
                                .update(cx, |screen, cx| screen.bind(&terminal_id, cx));
                        }
                        if let Some(saved) = sidebar.surface.terminals.get_mut(&key) {
                            saved.title = tab.title.clone();
                        }
                        sidebar.persist(&owner);
                        if app.active_session.as_deref() == Some(owner.as_str()) {
                            app.focus_terminal(key, w, cx);
                        }
                    }
                    Ok(None) => {
                        tab.connection = TerminalConnection::Missing;
                        tab.pending.clear();
                    }
                    Err(error) => {
                        tab.connection = TerminalConnection::Failed {
                            error,
                            retry_create: create && !has_screen,
                            has_screen,
                        };
                        tab.pending.clear();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn mount_restored_terminals(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let keys: Vec<_> = self.sidebar.surface.layout.terminal_keys().collect();
        for key in keys {
            self.connect_terminal(key, false, None, w, cx);
        }
    }

    fn new_terminal(
        &mut self,
        pane: DockPaneId,
        shell_path: Option<String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_session.is_none() {
            return;
        }
        self.next_terminal_tab = self
            .next_terminal_tab
            .max(self.sidebar.surface.layout.last_terminal_key())
            + 1;
        let key = self.next_terminal_tab;
        self.open_right_tab(pane, RightPanelTab::Terminal(key), w, cx);
        self.launch_terminal(key, shell_path, w, cx);
    }

    fn launch_terminal(
        &mut self,
        key: u64,
        shell_path: Option<String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(owner) = self.active_session.as_deref() else {
            return;
        };
        self.sidebar.surface.terminals.insert(
            key,
            TerminalCheckpoint {
                id: bridge::random_id(),
                title: None,
            },
        );
        // Persist the binding before asking the host to spawn.
        self.sidebar.persist(owner);
        self.connect_terminal(key, true, shell_path, w, cx);
    }

    fn replace_terminal(&mut self, key: u64, w: &mut Window, cx: &mut Context<Self>) {
        self.close_terminal(key, cx);
        self.launch_terminal(key, None, w, cx);
    }

    fn retry_terminal(&mut self, key: u64, w: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.sidebar.terminals.get(&key) else {
            return;
        };
        let TerminalConnection::Failed {
            retry_create: create,
            ..
        } = tab.connection
        else {
            return;
        };
        let shell_path = tab.requested_shell.clone();
        self.sidebar.terminals.remove(&key);
        self.connect_terminal(key, create, shell_path, w, cx);
    }

    fn close_terminal(&mut self, key: u64, cx: &mut Context<Self>) {
        self.cancel_closed_terminal_title(key);
        if let Some(tab) = self.sidebar.terminals.remove(&key) {
            tab.screen.update(cx, |screen, cx| screen.unbind(cx));
        }
        if let Some(saved) = self.sidebar.surface.terminals.remove(&key) {
            invoke("stop_terminal", json!({"terminalId": saved.id}));
        }
    }

    fn focus_terminal(&self, key: u64, w: &mut Window, cx: &mut Context<Self>) {
        let active = self
            .focused_right_pane(w, cx)
            .and_then(|pane| self.sidebar.surface.layout.pane(pane))
            .map(|pane| pane.active());
        if !self.terminal_title_has_focus(w, cx)
            && active == Some(RightPanelTab::Terminal(key))
            && let Some(tab) = self.sidebar.terminals.get(&key)
            && matches!(tab.connection, TerminalConnection::Connected)
        {
            tab.screen.update(cx, |screen, cx| screen.focus(w, cx));
        }
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
            .conversation
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
        self.sync_shell_window(cx);
        self.save_workspace_preferences(cx);
        cx.refresh_windows();
    }

    fn sync_shell_window(&mut self, cx: &App) {
        let dark = (!self.show_welcome).then(|| Theme::global(cx).mode == ThemeMode::Dark);
        let presentation = (self.show_welcome, dark);
        if self.shell_presentation != Some(presentation) {
            self.shell_presentation = Some(presentation);
            invoke(
                "configure_shell_window",
                json!({"welcome": self.show_welcome, "dark": dark}),
            );
        }
    }

    pub(crate) fn content_font_size(&self, base: f32) -> f32 {
        (base + self.content_font_delta as f32).clamp(12., 20.)
    }

    pub(crate) fn set_content_font_delta(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.content_font_delta = delta.clamp(-2, 4);
        self.save_workspace_preferences(cx);
        cx.notify();
    }
}

mod browser;
mod conversation;
use conversation::Conversation;

mod dock_controls;
pub(crate) use dock_controls::dock_action_button;
mod automations;
mod command_picker;
mod composer_stats;
mod dock_documents;
mod dock_drag;
mod dock_files;
mod dock_strip;
mod dock_tab_menu;
mod document_code;
pub(crate) mod document_fonts;
mod document_frontmatter;
mod document_html;
mod document_selection;
mod files;
mod floating_panes;
mod flow_motion;
mod flow_scroll;
mod goal_bar;
mod header_chrome;
mod live_shimmer;
mod menu_groups;
mod menu_surface;
mod message_feedback;
mod model_menu;
mod model_providers;
mod panels;
mod permission_dialog;
mod permission_picker;
mod plan;
mod plugin_center;
mod plugin_install;
mod plugin_versions;
mod process_scroll;
mod question_panel;
mod runtime_settings;
mod session_archive;
mod session_context_menu;
mod session_download;
mod session_feedback;
mod session_inbox;
mod session_markdown;
mod shortcut_settings;
mod start_panel;
mod subagent_view;
mod terminal;
mod terminal_shell_menu;
mod terminal_title;
mod transcript_preferences;
mod web_search_settings;
mod workspace;
mod workspace_rows;
mod workspace_search;
mod workspace_watch;

impl Render for DshApp {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = f32::from(w.viewport_size().width);
        let narrow = width < 1024.;
        let drawer = width < 768. && !bridge::is_windows();
        if self.sidebar_narrow != narrow || self.sidebar_drawer != drawer {
            self.sidebar_narrow = narrow;
            self.sidebar_drawer = drawer;
            self.narrow_sidebar_expanded = false;
            self.sidebar_dragging = false;
            self.sidebar_slide = None;
        }
        document_html::begin_frames();
        self.stats_dialog_bounds.set(None);
        self.auto_open_plan_preview(w, cx);
        self.update_right_panel_frame(w, cx);
        self.update_float_panes(w, cx);
        self.sync_dock_files(cx);
        self.sync_dock_documents(cx);
        self.sync_plan_previews(cx);
        self.update_dock_strips(cx);
        self.update_dock_tab_menu(w, cx);
        self.update_terminal_shell_menu(cx);
        self.header_menu_buttons.borrow_mut().clear();
        self.update_header_menu(w, cx);
        self.update_terminal_title(w, cx);
        self.persist_right_sidebar();
        self.dock_controls.begin_frame();
        let workspace = self.render_workspace(w, cx);
        self.dock_controls.end_frame();
        if self.browser_shell.is_some() {
            // The native surface needs the menu bounds measured during prepaint.
            let owner = cx.weak_entity();
            w.defer(cx, move |w, cx| {
                if let Some(owner) = owner.upgrade() {
                    owner.update(cx, |app, cx| app.sync_embedded_browser(w, cx));
                }
            });
        }
        workspace
            .when_some(document_fonts::interface_family(cx), |root, family| {
                root.font_family(family)
            })
            .when_some(
                document_fonts::interface_fallbacks(cx),
                |mut root, fallbacks| {
                    root.text_style().font_fallbacks = Some(fallbacks);
                    root
                },
            )
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
    let id = value.get("value")?.as_str()?;
    let name = value.get("name")?.as_str()?;
    let label = if id.is_empty() && name == "Provider default" {
        "Default".to_string()
    } else {
        model_choice_name(id, Some(name))
    };
    Some(PickerOption::new(id.to_string(), label))
}

fn model_choice_name(value: &str, supplied: Option<&str>) -> String {
    // The ACP controller can insert a legacy session's current model with
    // only its route ID. Preserve explicit catalog names and custom labels.
    if let Some(name) = supplied
        && !matches!(
            name,
            "deepseek-flash" | "deepseek-v4-flash" | "deepseek-v4-pro"
        )
    {
        return name.to_owned();
    }
    let route = serde_json::from_str::<[String; 2]>(value).ok();
    let name = supplied.unwrap_or_else(|| route.as_ref().map_or(value, |[_, id]| id));
    if route
        .as_ref()
        .is_some_and(|[provider, id]| provider == "deepseek-official" && id == name)
    {
        return match name {
            "deepseek-flash" => "DeepSeek-V4.1-Flash",
            "deepseek-v4-flash" => "DeepSeek-V4-Flash",
            "deepseek-v4-pro" => "DeepSeek-V4-Pro",
            _ => name,
        }
        .to_owned();
    }
    name.to_owned()
}

fn string_at(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// 组装 ACP `session/prompt` blocks：文本 + 内联图片（canonical base64）。
/// 待投递行在 dock 里的展示文案：Host 只回文本，附件用占位符表示。
fn pending_inbox_display(text: &str, has_image: bool) -> String {
    let text = text.trim();
    match (text.is_empty(), has_image) {
        (true, true) => "[图片]".to_string(),
        (true, false) => "（空消息）".to_string(),
        (false, true) => format!("{text}\n[图片]"),
        (false, false) => text.to_string(),
    }
}

fn build_prompt_payload(text: &str, attachments: &[DraftAttachment]) -> (String, Vec<Value>) {
    let mut blocks = Vec::new();
    let mut display_lines = Vec::new();
    if !text.is_empty() {
        blocks.push(json!({"type": "text", "text": text}));
        display_lines.push(text.to_string());
    }
    let mut file_notes = Vec::new();
    for item in attachments {
        match item.kind {
            AttachmentKind::Image => {
                if let Some((mime, data)) = image_block_from_attachment(item) {
                    blocks.push(json!({
                        "type": "image",
                        "mimeType": mime,
                        "data": data,
                    }));
                    display_lines.push(format!("[图片] {}", item.name));
                } else {
                    let note = format!("[图片未就绪] {}", item.name);
                    file_notes.push(note.clone());
                    display_lines.push(note);
                }
            }
            AttachmentKind::File => {
                let note = format!("📎 {} ({})", item.name, format_bytes(item.size));
                file_notes.push(note.clone());
                display_lines.push(note);
            }
        }
    }
    if !file_notes.is_empty() {
        let note = file_notes.join("\n");
        if text.is_empty()
            && blocks
                .iter()
                .all(|block| block.get("type") != Some(&json!("text")))
        {
            blocks.insert(0, json!({"type": "text", "text": note}));
        } else if let Some(first) = blocks.first_mut() {
            if first.get("type") == Some(&json!("text")) {
                let existing = first
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let merged = if existing.is_empty() {
                    note
                } else {
                    format!("{existing}\n\n{note}")
                };
                *first = json!({"type": "text", "text": merged});
            } else {
                blocks.insert(0, json!({"type": "text", "text": note}));
            }
        }
    }
    if blocks.is_empty() {
        blocks.push(json!({"type": "text", "text": ""}));
    }
    (display_lines.join("\n"), blocks)
}

/// 把草稿图片转换成 `commands.execute` 的 `CommandSubmitAttachment`。
/// KWork 的通用文件附件仍是草稿轨里的本地提示，未上传成 receipt，不能伪造。
fn command_attachment_payload(
    items: &[DraftAttachment],
    accepts_attachments: bool,
) -> Result<Vec<Value>, String> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    if !accepts_attachments {
        return Err("该指令不支持附件；草稿已保留。".to_owned());
    }
    let mut attachments = Vec::with_capacity(items.len());
    for item in items {
        match item.kind {
            AttachmentKind::File => {
                return Err("文件附件暂不支持该指令；草稿已保留。".to_owned());
            }
            AttachmentKind::Image => {
                let Some((mime, data)) = image_block_from_attachment(item) else {
                    return Err("图片尚未就绪；草稿已保留。".to_owned());
                };
                attachments.push(json!({
                    "type": "image",
                    "mediaType": mime,
                    "data": data,
                    "name": item.name,
                }));
            }
        }
    }
    Ok(attachments)
}

fn image_block_from_attachment(item: &DraftAttachment) -> Option<(String, String)> {
    let preview = item.preview.as_deref()?;
    let (header, data) = preview.split_once(",")?;
    if !header.starts_with("data:") || !header.contains(";base64") {
        return None;
    }
    let mime = header
        .trim_start_matches("data:")
        .split(';')
        .next()
        .unwrap_or("")
        .to_string();
    let mime = if mime.starts_with("image/") {
        mime
    } else if item.mime.starts_with("image/") {
        item.mime.clone()
    } else {
        return None;
    };
    match mime.as_str() {
        "image/png" | "image/jpeg" | "image/webp" | "image/gif" => {}
        _ => return None,
    }
    let data = data.trim();
    if data.is_empty() {
        return None;
    }
    Some((mime, data.to_string()))
}

pub(crate) fn format_bytes(size: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = size as f64;
    let mut unit = 0usize;
    while value >= 1024. && unit + 1 < UNITS.len() {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
