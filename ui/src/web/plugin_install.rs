//! The Host's plugin installer, including inspection, cancellation, and settlement.

use super::permission_dialog::{dialog_action, dialog_header};
use super::*;
use gpui::{Animation, AnimationExt, Div, Entity, FocusHandle, ScrollHandle, Stateful};
use serde::Deserialize;
use std::cell::Cell;

const GUIDE: [(&str, &str, &str); 3] = [
    (
        "填入插件 npm 包名",
        "dsh-plugin-whale-pet",
        "填写插件文档中 dsh plugin add 命令使用的包名。",
    ),
    (
        "填入 Git 仓库地址",
        "https://github.com/author/dsh-plugin",
        "把示例替换成实际仓库地址。",
    ),
    (
        "填入本地目录",
        "/Users/name/my-plugin",
        "把示例替换成插件目录的实际路径。",
    ),
];
const LOG_LIMIT: usize = 128 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Draft,
    Confirm,
    Checking,
    Installing,
    Cancelling,
    Applying,
    Reconciling,
    Done,
    Failed,
    Unknown,
}

impl Phase {
    fn busy(self) -> bool {
        matches!(
            self,
            Self::Checking
                | Self::Installing
                | Self::Cancelling
                | Self::Applying
                | Self::Reconciling
        )
    }

    fn can_cancel(self) -> bool {
        matches!(self, Self::Checking | Self::Installing | Self::Unknown)
    }

    fn title(self) -> &'static str {
        match self {
            Self::Draft | Self::Checking => "添加插件",
            Self::Confirm => "更新插件",
            Self::Installing => "正在安装插件…",
            Self::Cancelling => "正在停止安装…",
            Self::Applying => "正在应用配置，请稍候…",
            Self::Reconciling => "正在核对安装状态…",
            Self::Done => "插件安装完成",
            Self::Failed => "插件安装失败",
            Self::Unknown => "安装状态尚未确认",
        }
    }
}

#[derive(Clone)]
pub(super) struct PluginTarget {
    pub(super) name: String,
    pub(super) title: String,
    pub(super) from: Option<String>,
    pub(super) version: String,
    pub(super) registry: Option<String>,
}

enum Purpose {
    Manual,
    Catalog(PluginTarget),
    Update(PluginTarget),
}

impl Purpose {
    fn target(&self) -> Option<&PluginTarget> {
        match self {
            Self::Manual => None,
            Self::Catalog(target) | Self::Update(target) => Some(target),
        }
    }

    fn updating(&self) -> bool {
        self.target().is_some_and(|target| target.from.is_some())
    }

    fn accepts_build_approval(&self) -> bool {
        !matches!(self, Self::Update(_))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registries {
    registry: Option<String>,
    fallback_registries: Vec<String>,
    resolved: Option<String>,
}

#[derive(Deserialize)]
struct Subject {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    source: String,
}

#[derive(Deserialize)]
struct Attempt {
    registry: Option<String>,
    index: usize,
    total: usize,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum InstallReply {
    Refused { problem: String },
    Cancelled,
    Settled(InstallOutcome),
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallOutcome {
    application: String,
    stage: Option<String>,
    bundle: Option<String>,
    version: Option<String>,
    enabled: Option<bool>,
    problem: Option<String>,
    failure: Option<String>,
    #[serde(default)]
    pending_builds: Vec<String>,
    update: Option<UpdateOutcome>,
}

#[derive(Deserialize)]
struct UpdateOutcome {
    from: String,
    to: String,
    recovery: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UpdateStep {
    Checking,
    Removing,
    Installing,
    Restoring,
}

impl UpdateStep {
    fn from_event(step: &str) -> Option<Self> {
        match step {
            "checking" => Some(Self::Checking),
            "removing" => Some(Self::Removing),
            "installing" => Some(Self::Installing),
            "restoring" => Some(Self::Restoring),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
struct CancelReply {
    status: String,
}

pub(super) struct Installer {
    identity: Rc<()>,
    spec: Entity<Editor>,
    registry_editor: Entity<Editor>,
    _edits: Vec<Subscription>,
    close: FocusHandle,
    open: bool,
    phase: Phase,
    purpose: Purpose,
    steps: Vec<UpdateStep>,
    request: Option<String>,
    subject: Option<Subject>,
    outcome: Option<InstallOutcome>,
    attempt: Option<Attempt>,
    error: Option<String>,
    registries: Option<Registries>,
    registries_loading: bool,
    registry_error: bool,
    registry: Option<String>,
    registry_touched: bool,
    guide_open: bool,
    registry_open: bool,
    details_open: bool,
    approve_open: bool,
    close_on_cancel: bool,
    log: String,
    log_scroll: ScrollHandle,
}

impl Installer {
    fn new(w: &mut Window, cx: &mut Context<DshApp>) -> Self {
        let spec = editor("包名或地址".into(), InputMode::Single, w, cx);
        let registry_editor = editor("自定义安装源".into(), InputMode::Single, w, cx);
        spec.update(cx, |input, _| {
            input.set_placeholder("例如 dsh-plugin-whale-pet");
            input.font_size = 13.;
        });
        registry_editor.update(cx, |input, _| {
            input.set_placeholder("https://");
            input.font_size = 13.;
        });
        let edits = vec![
            cx.subscribe_in(&spec, w, |app, input, event: &EditEvent, w, cx| {
                if app
                    .plugin_installer
                    .as_ref()
                    .is_none_or(|install| install.spec != *input)
                {
                    return;
                }
                match event {
                    EditEvent::Submitted => app.begin_plugin_install(false, w, cx),
                    EditEvent::Changed => {
                        if let Some(install) = &mut app.plugin_installer
                            && install.phase == Phase::Draft
                        {
                            install.error = None;
                        }
                        cx.notify();
                    }
                    _ => {}
                }
            }),
            cx.subscribe_in(
                &registry_editor,
                w,
                |app, input, event: &EditEvent, w, cx| {
                    if app
                        .plugin_installer
                        .as_ref()
                        .is_none_or(|install| install.registry_editor != *input)
                    {
                        return;
                    }
                    match event {
                        EditEvent::Submitted => app.choose_custom_registry(w, cx),
                        EditEvent::Changed => cx.notify(),
                        _ => {}
                    }
                },
            ),
        ];
        spec.update(cx, |input, cx| input.focus(w, cx));
        Self {
            identity: Rc::new(()),
            spec,
            registry_editor,
            _edits: edits,
            close: cx.focus_handle(),
            open: true,
            phase: Phase::Draft,
            purpose: Purpose::Manual,
            steps: Vec::new(),
            request: None,
            subject: None,
            outcome: None,
            attempt: None,
            error: None,
            registries: None,
            registries_loading: false,
            registry_error: false,
            registry: None,
            registry_touched: false,
            guide_open: false,
            registry_open: false,
            details_open: false,
            approve_open: false,
            close_on_cancel: false,
            log: String::new(),
            log_scroll: ScrollHandle::new(),
        }
    }

    fn label(&self) -> String {
        self.registry
            .as_deref()
            .map_or_else(|| "默认安装源".into(), registry_label)
    }

    fn title(&self) -> &'static str {
        if !self.purpose.updating() {
            return self.phase.title();
        }
        match self.phase {
            Phase::Confirm => "更新插件",
            Phase::Checking => "正在检查目标版本…",
            Phase::Installing => match self.steps.last() {
                Some(UpdateStep::Checking) => "正在检查目标版本…",
                Some(UpdateStep::Removing) => "正在移除旧版本…",
                Some(UpdateStep::Restoring) => "正在恢复原版本…",
                _ => "正在安装新版本…",
            },
            Phase::Done => "插件更新完成",
            Phase::Failed => "插件更新失败",
            Phase::Unknown => "无法确认更新结果",
            _ => self.phase.title(),
        }
    }

    fn draft(&mut self, w: &mut Window, cx: &mut Context<DshApp>) {
        self.phase = Phase::Draft;
        self.request = None;
        self.error = None;
        self.approve_open = false;
        self.details_open = false;
        self.spec.update(cx, |input, cx| {
            input.disabled = false;
            input.focus(w, cx);
            cx.notify();
        });
    }

    fn append_log(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let at_bottom = -self.log_scroll.offset().y >= self.log_scroll.max_offset().y - px(18.);
        self.log
            .push_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
        if self.log.len() > LOG_LIMIT {
            let mut start = self.log.len() - LOG_LIMIT;
            while !self.log.is_char_boundary(start) {
                start += 1;
            }
            self.log.drain(..start);
        }
        if at_bottom {
            self.log_scroll.scroll_to_bottom();
        }
    }
}

pub(super) struct AddMenu {
    trigger: FocusHandle,
    item: FocusHandle,
    open: bool,
}

impl DshApp {
    pub(super) fn plugin_install_busy(&self) -> bool {
        self.plugin_installer
            .as_ref()
            .is_some_and(|install| install.phase.busy())
    }

    pub(super) fn close_plugin_add_menu(&mut self) {
        if let Some(menu) = &mut self.plugin_add_menu {
            menu.open = false;
        }
    }

    fn toggle_plugin_add_menu(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = &mut self.plugin_add_menu else {
            return;
        };
        menu.open = !menu.open;
        if menu.open {
            menu.item.focus(w, cx);
            // The portaled row joins the dispatch tree on the next frame.
            cx.on_next_frame(w, |app, w, cx| {
                if let Some(menu) = app.plugin_add_menu.as_ref().filter(|menu| menu.open) {
                    menu.item.focus(w, cx);
                }
            });
        } else {
            menu.trigger.focus(w, cx);
        }
        cx.notify();
    }

    pub(super) fn render_plugin_install_trigger(
        &mut self,
        w: &Window,
        show_label: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let task = self
            .plugin_installer
            .as_ref()
            .is_some_and(|install| install.request.is_some());
        let enabled = self.plugin_installer.is_some() || self.can_install_plugins();
        let p = dsh(cx);
        let focus = self
            .plugin_install_trigger
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let label = if task
            && self
                .plugin_installer
                .as_ref()
                .is_some_and(|install| install.purpose.updating())
        {
            "查看更新任务"
        } else if task {
            "查看安装任务"
        } else {
            "添加插件"
        };
        let button = row()
            .id("add-plugin")
            .role(gpui::Role::Button)
            .aria_label(label)
            .track_focus(&focus)
            .tab_stop(enabled)
            .h(px(32.))
            .px(px(12.))
            .gap(px(6.))
            .when(task, |button| button.rounded(px(12.)))
            .when(!task, |button| button.rounded_l(px(12.)))
            .text_size(px(13.))
            .bg(p.button_primary_fill)
            .text_color(p.label_primary_foreground)
            .child(icon(Glyph::Plus, 13., p.label_primary_foreground))
            .when(show_label, |button| button.child(label));
        let primary = if enabled {
            super::dock_action_button(
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(p.button_primary_hover)),
                cx.listener(|app, _, w, cx| app.open_plugin_installer(w, cx)),
            )
        } else {
            button.opacity(0.45)
        };
        if task {
            return row().child(primary);
        }
        let menu = self.plugin_add_menu.get_or_insert_with(|| AddMenu {
            trigger: cx.focus_handle(),
            item: cx.focus_handle(),
            open: false,
        });
        let open = menu.open && enabled;
        let arrow = row()
            .id("add-plugin-methods")
            .role(gpui::Role::Button)
            .aria_label("选择添加插件方式")
            .aria_expanded(open)
            .track_focus(&menu.trigger)
            .tab_stop(enabled)
            .relative()
            .size(px(32.))
            .justify_center()
            .rounded_r(px(12.))
            .border_l(px(0.5))
            .border_color(p.label_primary_foreground.alpha(0.3))
            .bg(p.button_primary_fill)
            .child(icon(Glyph::Down, 12., p.label_primary_foreground));
        let arrow = if enabled {
            super::dock_action_button(
                arrow
                    .cursor_pointer()
                    .hover(|style| style.bg(p.button_primary_hover))
                    .capture_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, w, cx| {
                        let stroke = &event.keystroke;
                        if stroke.modifiers.modified() {
                            return;
                        }
                        let open = app.plugin_add_menu.as_ref().is_some_and(|menu| menu.open);
                        match stroke.key.as_str() {
                            "down" if !open => app.toggle_plugin_add_menu(w, cx),
                            "enter" | "space" if open => app.open_plugin_installer(w, cx),
                            "escape" if open => app.toggle_plugin_add_menu(w, cx),
                            _ => return,
                        }
                        w.prevent_default();
                        cx.stop_propagation();
                    })),
                cx.listener(|app, _, w, cx| app.toggle_plugin_add_menu(w, cx)),
            )
        } else {
            arrow.opacity(0.45)
        };
        let arrow = arrow.children(open.then(|| self.plugin_add_options(w, cx)));
        row().gap_0().child(primary).child(arrow)
    }

    fn plugin_add_options(&self, w: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        use gpui::{AvailableSpace, Bounds, MouseDownEvent, Pixels, canvas, point, size};
        let p = dsh(cx);
        let width = px(320.).min(w.viewport_size().width - px(24.));
        let bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let anchor = bounds.clone();
        let focus = &self.plugin_add_menu.as_ref().expect("open menu").item;
        let card = column()
            .id("plugin-add-menu")
            .role(gpui::Role::Menu)
            .w(width)
            .p(px(4.))
            .rounded(px(16.))
            .occlude()
            .child(super::menu_surface::menu_material(p, 16.))
            .shadow(super::menu_surface::menu_elevation(p))
            .on_mouse_down_out(cx.listener(move |app, event: &MouseDownEvent, _, cx| {
                if !bounds
                    .get()
                    .is_some_and(|bounds| bounds.contains(&event.position))
                {
                    app.close_plugin_add_menu();
                    cx.notify();
                }
            }))
            .capture_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, w, cx| {
                if matches!(event.keystroke.key.as_str(), "escape" | "tab") {
                    if let Some(menu) = &app.plugin_add_menu {
                        menu.trigger.focus(w, cx);
                    }
                    app.close_plugin_add_menu();
                    if event.keystroke.key == "escape" {
                        w.prevent_default();
                        cx.stop_propagation();
                    }
                    cx.notify();
                }
            }))
            .child(super::dock_action_button(
                row()
                    .id("plugin-add-existing")
                    .role(gpui::Role::MenuItem)
                    .aria_label("安装第三方插件")
                    .track_focus(focus)
                    .tab_stop(true)
                    .w_full()
                    .h(px(72.))
                    .px(px(12.))
                    .gap(px(8.))
                    .rounded(px(12.))
                    .cursor_pointer()
                    .hover(|style| style.bg(p.hover))
                    .focus_visible(|style| style.bg(p.hover))
                    .child(icon(Glyph::Download, 14., p.label_secondary))
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .line_height(px(20.))
                                    .child("安装第三方插件"),
                            )
                            .child(note("支持 npm 包、Git 仓库和本地目录", cx)),
                    ),
                cx.listener(|app, _, w, cx| app.open_plugin_installer(w, cx)),
            ))
            .into_any_element();
        canvas(
            move |trigger, w, cx| {
                anchor.set(Some(trigger));
                let mut card = card;
                let measured = card.layout_as_root(
                    size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                    w,
                    cx,
                );
                let view = w.viewport_size();
                let x = (trigger.right() - measured.width)
                    .max(px(12.))
                    .min((view.width - px(12.) - measured.width).max(px(12.)));
                let y = (trigger.bottom() + px(4.))
                    .min((view.height - px(12.) - measured.height).max(px(12.)));
                w.defer_draw(card, point(x.round(), y.round()), 4, None);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    fn open_plugin_installer(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.close_plugin_add_menu();
        if let Some(install) = &mut self.plugin_installer {
            install.open = true;
            if install.phase == Phase::Draft {
                install.spec.update(cx, |input, cx| input.focus(w, cx));
            } else {
                install.close.focus(w, cx);
            }
        } else if self.can_install_plugins() {
            self.plugin_installer = Some(Installer::new(w, cx));
            self.request_plugin_registries(cx);
        }
        cx.notify();
    }

    pub(super) fn open_catalog_plugin(
        &mut self,
        target: PluginTarget,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_target_installer(Purpose::Catalog(target), false, w, cx);
    }

    pub(super) fn confirm_catalog_update(
        &mut self,
        target: PluginTarget,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_target_installer(Purpose::Catalog(target), true, w, cx);
    }

    pub(super) fn open_plugin_version_update(
        &mut self,
        target: PluginTarget,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_target_installer(Purpose::Update(target), true, w, cx);
    }

    fn open_target_installer(
        &mut self,
        purpose: Purpose,
        confirm: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.plugin_installer.is_some() {
            self.open_plugin_installer(w, cx);
            return;
        }
        if !self.can_install_plugins() {
            return;
        }
        let mut install = Installer::new(w, cx);
        install.purpose = purpose;
        install.phase = Phase::Confirm;
        install.close.focus(w, cx);
        self.plugin_installer = Some(install);
        if confirm {
            cx.notify();
        } else {
            self.begin_plugin_install(false, w, cx);
        }
    }

    fn request_plugin_registries(&mut self, cx: &mut Context<Self>) {
        let Some(install) = &mut self.plugin_installer else {
            return;
        };
        if install.registries_loading {
            return;
        }
        install.registries_loading = true;
        install.registry_error = false;
        let identity = install.identity.clone();
        cx.spawn(async move |this, cx| {
            let result = bridge::invoke(
                "plugin_install",
                json!({"operation":{"action":"registries"}}),
            )
            .await
            .and_then(|value| {
                serde_json::from_value::<Registries>(value).map_err(|_| "无法读取安装源。".into())
            });
            let _ = this.update(cx, |app, cx| {
                let Some(install) = app
                    .plugin_installer
                    .as_mut()
                    .filter(|install| Rc::ptr_eq(&install.identity, &identity))
                else {
                    return;
                };
                install.registries_loading = false;
                match result {
                    Ok(registries) => {
                        if install.registries.is_none()
                            && !install.registry_touched
                            && install.phase == Phase::Draft
                        {
                            install.registry = registries.registry.clone();
                        }
                        install.registries = Some(registries);
                    }
                    Err(_) => install.registry_error = true,
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn begin_plugin_install(&mut self, approve: bool, w: &mut Window, cx: &mut Context<Self>) {
        if !self.connected() {
            return;
        }
        let Some(install) = &mut self.plugin_installer else {
            return;
        };
        let request = bridge::random_id();
        let operation = if approve {
            if install.phase != Phase::Failed
                || !install.approve_open
                || !install.purpose.accepts_build_approval()
                || install
                    .outcome
                    .as_ref()
                    .is_none_or(|outcome| outcome.pending_builds.is_empty())
            {
                return;
            }
            let Some(previous) = install.request.as_ref() else {
                return;
            };
            json!({"action":"approve", "requestId":request, "previousRequestId":previous})
        } else if install.phase == Phase::Confirm {
            match &install.purpose {
                Purpose::Catalog(target) => json!({
                    "action":"catalog", "requestId":request, "name":target.name, "from":target.from,
                    "version":target.version, "enabled":true,
                }),
                Purpose::Update(target) => json!({
                    "action":"update", "requestId":request, "name":target.name, "from":target.from,
                    "version":target.version, "registry":target.registry,
                }),
                Purpose::Manual => return,
            }
        } else {
            if install.phase != Phase::Draft || install.registries_loading {
                return;
            }
            let text = install.spec.read(cx).value().trim();
            if text.is_empty() {
                return;
            }
            json!({"action":"run", "requestId":request, "spec":text, "registry":install.registry})
        };
        install.request = Some(request.clone());
        install.phase = if approve {
            Phase::Installing
        } else {
            Phase::Checking
        };
        install.error = None;
        install.subject = None;
        install.steps.clear();
        if install.purpose.updating() {
            install.steps.push(UpdateStep::Checking);
        }
        install.attempt = None;
        install.outcome = None;
        install.approve_open = false;
        install.close_on_cancel = false;
        install.registry_open = false;
        install.log.clear();
        install.details_open = false;
        install.spec.update(cx, |input, cx| {
            input.disabled = true;
            cx.notify();
        });
        if install.purpose.target().is_some() {
            install.close.focus(w, cx);
        }
        self.request_plugin_install_result(request, operation, w, cx);
    }

    fn request_plugin_install_result(
        &self,
        request: String,
        operation: Value,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(w, async move |this, cx| {
            let result = bridge::invoke("plugin_install", json!({"operation":operation}))
                .await
                .and_then(|value| {
                    serde_json::from_value::<Option<InstallReply>>(value)
                        .map_err(|_| "无法读取插件安装结果。".into())
                });
            let _ = this.update_in(cx, |app, w, cx| {
                app.finish_plugin_install(&request, result, w, cx)
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_plugin_install(
        &mut self,
        request: &str,
        result: Result<Option<InstallReply>, String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(install) = self
            .plugin_installer
            .as_mut()
            .filter(|install| install.request.as_deref() == Some(request) && install.phase.busy())
        else {
            return;
        };
        let mut refresh = false;
        let mut close = false;
        match result {
            Ok(Some(InstallReply::Cancelled)) => {
                close = install.close_on_cancel || install.purpose.target().is_some();
                install.draft(w, cx);
                self.push_toast("已取消安装。", cx);
            }
            Ok(Some(InstallReply::Refused { problem })) => {
                if install.purpose.target().is_none() {
                    install.draft(w, cx);
                } else {
                    install.phase = Phase::Failed;
                }
                install.error = Some(inspect_problem(&problem).into());
                if problem == "invalid-registry" {
                    install.registry_open = true;
                    install
                        .registry_editor
                        .update(cx, |input, cx| input.focus(w, cx));
                }
            }
            Ok(Some(InstallReply::Settled(outcome)))
                if outcome.application == "cancelled" && !install.purpose.updating() =>
            {
                close = install.close_on_cancel || install.purpose.target().is_some();
                install.draft(w, cx);
                self.push_toast("已取消安装。", cx);
                refresh = true;
            }
            Ok(Some(InstallReply::Settled(outcome))) => {
                install.phase = if matches!(outcome.application.as_str(), "failed" | "cancelled") {
                    Phase::Failed
                } else if outcome.bundle.is_some()
                    && outcome
                        .update
                        .as_ref()
                        .is_none_or(|update| outcome.version.as_deref() == Some(&update.to))
                {
                    Phase::Done
                } else {
                    Phase::Unknown
                };
                install.error = match install.phase {
                    Phase::Failed => {
                        Some(install_problem(&outcome, install.purpose.updating()).into())
                    }
                    Phase::Unknown if install.purpose.updating() => {
                        Some("Host 未确认目标版本。请刷新插件清单。".into())
                    }
                    Phase::Unknown => Some("Host 未确认新增插件。请刷新插件清单。".into()),
                    _ => None,
                };
                install.outcome = Some(outcome);
                if install.open {
                    install.close.focus(w, cx);
                }
                refresh = true;
            }
            other => {
                install.phase = Phase::Unknown;
                install.error = Some(match other {
                    Err(error) => error,
                    Ok(None) => "Host 当前没有此安装任务。请检查插件列表。".into(),
                    _ => "未收到可确认的安装结果。".into(),
                });
                if install.open {
                    install.close.focus(w, cx);
                }
                refresh = true;
            }
        }
        if close {
            self.dismiss_plugin_installer(w, cx);
        }
        if refresh {
            if let Some(install) = self.plugin_installer.as_ref()
                && !install.open
                && matches!(install.phase, Phase::Done | Phase::Failed | Phase::Unknown)
            {
                self.push_toast(install.title(), cx);
            }
            self.refresh_plugin_center(cx);
        }
        cx.notify();
    }

    pub(super) fn plugin_install_event(
        &mut self,
        event: &Value,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = event["requestId"].as_str();
        let Some(install) = self
            .plugin_installer
            .as_mut()
            .filter(|install| id.is_some() && install.request.as_deref() == id)
        else {
            return;
        };
        match event["type"].as_str() {
            Some("subject") if install.phase.busy() => {
                install.subject = serde_json::from_value(event["subject"].clone()).ok();
                if install.phase != Phase::Cancelling {
                    install.phase = Phase::Installing;
                    if install.open {
                        install.close.focus(w, cx);
                    }
                }
            }
            Some("phase") if install.phase.busy() => match event["phase"].as_str() {
                Some("installing") => {
                    if install.phase != Phase::Cancelling {
                        install.phase = Phase::Installing;
                    }
                    install.attempt = serde_json::from_value(event["attempt"].clone()).ok();
                }
                Some("applying") => install.phase = Phase::Applying,
                Some("cancelling") => install.phase = Phase::Cancelling,
                _ => {}
            },
            Some("update") if install.phase.busy() && install.purpose.updating() => {
                if let Some(step) = event["step"].as_str().and_then(UpdateStep::from_event)
                    && !install.steps.contains(&step)
                {
                    install.steps.push(step);
                }
                install.phase = Phase::Installing;
            }
            Some("log") => {
                if let Some(text) = event["text"].as_str() {
                    install.append_log(text);
                }
            }
            _ => return,
        }
        cx.notify();
    }

    fn cancel_plugin_install(&mut self, close: bool, w: &mut Window, cx: &mut Context<Self>) {
        let Some(install) = &mut self.plugin_installer else {
            return;
        };
        let Some(request) = install.request.clone() else {
            return;
        };
        if install.phase == Phase::Cancelling {
            install.close_on_cancel |= close;
            return;
        }
        if !install.phase.can_cancel() {
            return;
        }
        let previous = install.phase;
        install.phase = Phase::Cancelling;
        install.close_on_cancel = close;
        install.error = None;
        install.close.focus(w, cx);
        cx.spawn_in(w, async move |this, cx| {
            let reply = bridge::invoke(
                "plugin_install",
                json!({"operation":{"action":"cancel","requestId":request}}),
            )
            .await
            .and_then(|value| {
                serde_json::from_value::<CancelReply>(value)
                    .map_err(|_| "取消请求未得到确认。".into())
            });
            let _ = this.update_in(cx, |app, w, cx| {
                let Some(install) = app.plugin_installer.as_mut().filter(|install| {
                    install.request.as_deref() == Some(&request)
                        && matches!(install.phase, Phase::Cancelling | Phase::Applying)
                }) else {
                    return;
                };
                match reply.as_ref().map(|reply| reply.status.as_str()) {
                    Ok("cancelled") => {
                        app.finish_plugin_install(
                            &request,
                            Ok(Some(InstallReply::Cancelled)),
                            w,
                            cx,
                        );
                    }
                    Ok("too-late") => {
                        install.phase = Phase::Applying;
                        install.error = Some("安装已进入收尾阶段，请等待结果。".into());
                        if install.close_on_cancel {
                            install.open = false;
                            if let Some(trigger) = &app.plugin_install_trigger {
                                trigger.focus(w, cx);
                            }
                        }
                    }
                    Ok("not-running") => app.observe_plugin_install(w, cx),
                    _ => {
                        if install.phase != Phase::Applying {
                            install.phase = previous;
                        }
                        install.error = Some("尚未确认安装已停止，可以重试取消或等待结果。".into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn observe_plugin_install(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(install) = &mut self.plugin_installer else {
            return;
        };
        let Some(request) = install.request.clone() else {
            return;
        };
        if install.phase == Phase::Reconciling {
            return;
        }
        install.phase = Phase::Reconciling;
        let operation = json!({"action":"observe","requestId":request});
        self.request_plugin_install_result(request, operation, w, cx);
    }

    fn dismiss_plugin_installer(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.plugin_installer = None;
        if let Some(trigger) = &self.plugin_install_trigger {
            trigger.focus(w, cx);
        }
        cx.notify();
    }

    fn close_plugin_installer(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(install) = &mut self.plugin_installer else {
            return;
        };
        if install.purpose.updating() && install.phase.busy() {
            return;
        }
        match install.phase {
            Phase::Checking | Phase::Installing | Phase::Cancelling => {
                self.cancel_plugin_install(true, w, cx)
            }
            Phase::Applying | Phase::Reconciling => {
                install.open = false;
                if let Some(trigger) = &self.plugin_install_trigger {
                    trigger.focus(w, cx);
                }
                cx.notify();
            }
            _ => self.dismiss_plugin_installer(w, cx),
        }
    }

    fn choose_custom_registry(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(install) = &mut self.plugin_installer else {
            return;
        };
        if install.phase != Phase::Draft {
            return;
        }
        let value = install.registry_editor.read(cx).value().trim().to_owned();
        if !value.starts_with("https://") && !value.starts_with("http://") {
            install.error = Some("请输入 HTTP 或 HTTPS 安装源地址。".into());
        } else {
            install.registry = Some(value);
            install.registry_touched = true;
            install.registry_open = false;
            install.error = None;
            install.spec.update(cx, |input, cx| input.focus(w, cx));
        }
        cx.notify();
    }

    pub(super) fn render_plugin_installer(
        &self,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let install = self
            .plugin_installer
            .as_ref()
            .filter(|install| install.open)?;
        let p = dsh(cx);
        let draft = matches!(install.phase, Phase::Draft | Phase::Checking)
            && install.purpose.target().is_none();
        let update = install
            .purpose
            .target()
            .filter(|target| target.from.is_some());
        let content = if let Some(target) = update {
            self.plugin_update_body(install, target, cx)
        } else if draft {
            self.plugin_install_form(install, w, cx)
        } else {
            self.plugin_install_progress(install, cx)
        };
        let footer = if draft {
            let can_run = install.phase == Phase::Draft
                && !install.registries_loading
                && !install.spec.read(cx).value().trim().is_empty();
            column().w_full().gap(px(20.)).px(px(24.)).pb(px(24.))
                .child(row().w_full().gap(px(8.)).items_start().p(px(10.)).rounded(px(12.))
                    .bg(p.state_warn_tertiary)
                    .child(icon(Glyph::Alert, 14., p.state_warn_primary))
                    .child(note("请确认插件来源可信。插件在本机以你的权限运行，来源不明的插件可能读取或修改你的数据。", cx)))
                .child(dialog_action("install-plugin-run",
                    if install.phase == Phase::Checking { "正在检查…" } else { "安装" },
                    true, !can_run, 0., cx, |app,w,cx| app.begin_plugin_install(false,w,cx)).w_full())
        } else {
            self.plugin_install_footer(install, cx)
        };
        let title = install.title();
        let header = dialog_header(
            update.map_or_else(
                || if draft { title } else { "" }.to_owned(),
                |target| format!("更新 {}", target.title),
            ),
            "close-plugin-install",
            Some(&install.close),
            cx,
            |app, w, cx| app.close_plugin_installer(w, cx),
        );
        Some(
            row()
                .id("plugin-install-overlay")
                .absolute()
                .inset_0()
                .p(px(24.))
                .justify_center()
                .items_center()
                .bg(p.bg_mask_1)
                .occlude()
                .child(
                    column()
                        .id("plugin-install-dialog")
                        .role(gpui::Role::Dialog)
                        .aria_label(title)
                        .tab_group()
                        .w(px(if update.is_some() { 460. } else { 560. }))
                        .max_w_full()
                        .max_h_full()
                        .min_h_0()
                        .overflow_hidden()
                        .rounded(px(24.))
                        .bg(p.bg_layer_2)
                        .shadow(crate::tokens::elevation_prominent())
                        .capture_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, w, cx| {
                            if event.keystroke.key == "escape" {
                                if let Some(install) = &mut app.plugin_installer
                                    && install.registry_open
                                {
                                    install.registry_open = false;
                                    install.spec.update(cx, |input, cx| input.focus(w, cx));
                                    cx.notify();
                                } else {
                                    app.close_plugin_installer(w, cx);
                                }
                                w.prevent_default();
                                cx.stop_propagation();
                            } else {
                                super::dock_controls::traverse_dock_focus(event, w, cx);
                            }
                        }))
                        .child(header)
                        .child(
                            column()
                                .id("plugin-install-body")
                                .w_full()
                                .min_h_0()
                                .overflow_y_scroll()
                                .px(px(24.))
                                .pb(px(20.))
                                .child(content),
                        )
                        .child(footer),
                ),
        )
    }

    fn plugin_install_form(&self, install: &Installer, w: &Window, cx: &mut Context<Self>) -> Div {
        let p = dsh(cx);
        let editing = install.phase == Phase::Draft;
        let mut body = column()
            .w_full()
            .gap(px(12.))
            .child(note("输入插件的包名、GitHub 仓库地址或本地目录路径。", cx))
            .child(installer_input(
                &install.spec,
                install.error.is_some(),
                w,
                cx,
            ))
            .when_some(install.error.as_ref(), |body, error| {
                body.child(install_error(error, cx))
            })
            .child(
                row()
                    .w_full()
                    .justify_between()
                    .gap(px(8.))
                    .child(install_link(
                        "install-guide",
                        if install.guide_open {
                            "收起引导"
                        } else {
                            "插件安装引导和示例"
                        },
                        true,
                        cx,
                        |app, _, cx| {
                            if let Some(install) = &mut app.plugin_installer {
                                install.guide_open = !install.guide_open;
                                cx.notify();
                            }
                        },
                    ))
                    .child(install_link(
                        "install-registry",
                        format!("安装源  {}", install.label()),
                        editing,
                        cx,
                        |app, _, cx| {
                            if let Some(install) = &mut app.plugin_installer {
                                install.registry_open = !install.registry_open;
                                cx.notify();
                            }
                        },
                    )),
            );
        if install.guide_open {
            let mut guide = column()
                .w_full()
                .gap(px(8.))
                .p(px(12.))
                .rounded(px(12.))
                .bg(p.bg_module_platform);
            for (index, (title, sample, hint)) in GUIDE.into_iter().enumerate() {
                guide = guide.child(
                    row()
                        .w_full()
                        .gap(px(8.))
                        .child(
                            column()
                                .flex_1()
                                .min_w_0()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_family("monospace")
                                        .child(sample),
                                )
                                .child(note(hint, cx)),
                        )
                        .child(install_link(
                            ("install-example", index),
                            "填入示例",
                            editing,
                            cx,
                            move |app, w, cx| {
                                if let Some(install) = &app.plugin_installer {
                                    replace(&install.spec, sample, w, cx);
                                    install.spec.update(cx, |input, cx| input.focus(w, cx));
                                }
                            },
                        )),
                );
            }
            body = body.child(guide);
        }
        if install.registry_open {
            body = body.child(self.plugin_registry_options(install, w, cx));
        }
        body
    }

    fn plugin_registry_options(
        &self,
        install: &Installer,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = dsh(cx);
        let mut choices = vec![None];
        if let Some(list) = &install.registries {
            for registry in list.registry.iter().chain(&list.fallback_registries) {
                let choice = Some(registry.as_str());
                if !choices.contains(&choice) {
                    choices.push(choice);
                }
            }
        }
        let mut body = column()
            .w_full()
            .gap(px(6.))
            .p(px(12.))
            .rounded(px(12.))
            .bg(p.bg_layer_3)
            .border(px(0.5))
            .border_color(p.border_l3)
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("选择安装源"),
            );
        for (index, registry) in choices.into_iter().enumerate() {
            let checked = install.registry.as_deref() == registry;
            let title = match registry {
                Some(url) => registry_label(url),
                None => install
                    .registries
                    .as_ref()
                    .and_then(|list| list.resolved.as_deref())
                    .map_or_else(
                        || "默认安装源".into(),
                        |url| format!("默认安装源（{}）", registry_label(url)),
                    ),
            };
            let value = registry.map(str::to_owned);
            body = body.child(install_link(
                ("install-registry-option", index),
                format!("{}{}", if checked { "✓  " } else { "" }, title),
                true,
                cx,
                move |app, w, cx| {
                    if let Some(install) = &mut app.plugin_installer {
                        install.registry = value.clone();
                        install.registry_touched = true;
                        install.registry_open = false;
                        install.error = None;
                        install.spec.update(cx, |input, cx| input.focus(w, cx));
                        cx.notify();
                    }
                },
            ));
        }
        if install.registry_error {
            body = body.child(
                row()
                    .gap(px(8.))
                    .child(install_error("无法读取安装源列表。", cx))
                    .child(install_link(
                        "reload-plugin-registries",
                        "重试",
                        true,
                        cx,
                        |app, _, cx| app.request_plugin_registries(cx),
                    )),
            );
        }
        body.child(
            row()
                .w_full()
                .gap(px(8.))
                .child(div().flex_1().min_w_0().child(installer_input(
                    &install.registry_editor,
                    false,
                    w,
                    cx,
                )))
                .child(install_link(
                    "use-custom-plugin-registry",
                    "使用地址",
                    true,
                    cx,
                    |app, w, cx| app.choose_custom_registry(w, cx),
                )),
        )
    }

    fn plugin_update_body(
        &self,
        install: &Installer,
        target: &PluginTarget,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = dsh(cx);
        let from = target
            .from
            .as_deref()
            .filter(|from| !from.is_empty())
            .unwrap_or("当前版本");
        let to = &target.version;
        let source = match &install.purpose {
            Purpose::Catalog(_) => "官方目录".into(),
            _ => target
                .registry
                .as_deref()
                .map_or_else(|| "默认安装源".into(), registry_label),
        };
        let mut body = column().w_full().gap(px(16.));
        if install.phase == Phase::Confirm {
            return body.child(
                row().w_full().h(px(64.)).justify_center().gap(px(16.))
                    .rounded(px(12.)).bg(p.bg_module_platform)
                    .font_family("monospace").text_size(px(16.))
                    .child(div().text_color(p.label_tertiary).child(from.to_owned()))
                    .child(gpui::svg().data(include_bytes!("../../assets/header-chevron-right.svg"))
                        .size(px(14.)).text_color(p.label_tertiary))
                    .child(to.clone()),
            ).child(note(format!("安装来源 {source}"), cx))
                .child(div().text_size(px(13.)).line_height(px(20.)).text_color(p.label_secondary).child(format!(
                    "将从 {from} 更新到 {to}，期间插件可能短暂停止。安装失败时会保留或重新安装 {from}。",
                )));
        }
        let outcome = install.outcome.as_ref();
        let failed = install.phase == Phase::Failed;
        if !install.phase.busy() {
            let (title, glyph, tone) = match install.phase {
                Phase::Done
                    if outcome.is_some_and(|outcome| outcome.application == "restart-required") =>
                {
                    (
                        format!("已安装 {to}，重连后生效"),
                        Glyph::Alert,
                        p.state_warn_primary,
                    )
                }
                Phase::Done => (
                    format!("已更新到 {to}"),
                    Glyph::Check,
                    p.state_success_primary,
                ),
                Phase::Failed => (
                    format!("无法更新到 {to}"),
                    Glyph::Close,
                    p.state_error_primary,
                ),
                _ => (
                    "无法确认更新结果".into(),
                    Glyph::Alert,
                    p.state_warn_primary,
                ),
            };
            body = body.child(
                row()
                    .w_full()
                    .items_start()
                    .gap(px(12.))
                    .child(
                        row()
                            .size(px(28.))
                            .flex_none()
                            .justify_center()
                            .rounded(px(8.))
                            .bg(tone.alpha(0.12))
                            .child(icon(glyph, 14., tone)),
                    )
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(title),
                            )
                            .when_some(install.error.as_ref(), |col, error| {
                                col.child(note(error.clone(), cx))
                            })
                            .when_some(
                                outcome
                                    .and_then(|outcome| outcome.update.as_ref())
                                    .filter(|_| failed),
                                |col, update| {
                                    let recovery = match update.recovery.as_str() {
                                        "kept" => format!("已保留原版本 {}。", update.from),
                                        "restored" => format!("已恢复原版本 {}。", update.from),
                                        "missing" => "插件未恢复，请检查插件清单。".into(),
                                        "changed" => "插件状态已变化，请检查当前版本。".into(),
                                        _ => "暂时无法核对已安装版本。".into(),
                                    };
                                    col.child(note(recovery, cx))
                                },
                            ),
                    ),
            );
        }
        let mut steps = column().w_full().gap(px(8.));
        for (index, step) in install.steps.iter().enumerate() {
            let last = index + 1 == install.steps.len();
            let failed_step = failed
                && match outcome.and_then(|outcome| outcome.stage.as_deref()) {
                    Some("remove") => *step == UpdateStep::Removing,
                    Some("enable" | "install")
                        if install.steps.contains(&UpdateStep::Installing) =>
                    {
                        *step == UpdateStep::Installing
                    }
                    _ => last,
                };
            let restored = *step == UpdateStep::Restoring
                && outcome
                    .and_then(|outcome| outcome.update.as_ref())
                    .is_some_and(|update| update.recovery == "restored");
            let (glyph, tone) = if last && install.phase.busy() {
                (Glyph::Spinner, p.state_business_primary)
            } else if failed_step || (failed && *step == UpdateStep::Restoring && !restored) {
                (Glyph::Close, p.state_error_primary)
            } else if last && install.phase == Phase::Unknown {
                (Glyph::Alert, p.state_warn_primary)
            } else {
                (Glyph::Check, p.state_success_primary)
            };
            let label = match step {
                UpdateStep::Checking => format!("检查 {to}"),
                UpdateStep::Removing => format!("停止并移除 {from}"),
                UpdateStep::Installing => format!("从 {source} 安装 {to}"),
                UpdateStep::Restoring => format!("重新安装 {from}"),
            };
            steps = steps.child(
                row()
                    .w_full()
                    .gap(px(8.))
                    .child(icon(glyph, 12., tone))
                    .child(note(label, cx)),
            );
        }
        body.child(steps).when(
            !install.log.is_empty()
                || outcome.is_some_and(|outcome| !outcome.pending_builds.is_empty()),
            |body| body.child(self.plugin_install_output(install, cx)),
        )
    }

    fn plugin_install_progress(&self, install: &Installer, cx: &mut Context<Self>) -> Div {
        let p = dsh(cx);
        let (art, color) = match install.phase {
            Phase::Confirm => (
                include_bytes!("../../assets/refresh-outline.svg") as &[u8],
                p.state_business_primary,
            ),
            Phase::Done => (
                include_bytes!("../../assets/plugin-install-check-circle-fill.svg") as &[u8],
                p.state_success_secondary,
            ),
            Phase::Failed | Phase::Unknown => (
                include_bytes!("../../assets/plugin-install-warning-outline.svg") as &[u8],
                p.state_warn_label,
            ),
            _ => (super::workspace::SPINNER_ARC_SVG, p.state_business_primary),
        };
        let glyph = gpui::svg().data(art).size(px(28.)).text_color(color);
        let status = if install.phase.busy() && !cx.reduce_motion() {
            glyph
                .with_animation(
                    "plugin-install-spinner",
                    Animation::new(std::time::Duration::from_millis(800)).repeat(),
                    |element, progress| {
                        element.with_transformation(
                            gpui::Transformation::default()
                                .with_rotation(gpui::radians(progress * 2. * std::f32::consts::PI)),
                        )
                    },
                )
                .into_any_element()
        } else {
            glyph.into_any_element()
        };
        let mut body = column().w_full().gap(px(14.)).child(
            column()
                .w_full()
                .items_center()
                .gap(px(8.))
                .child(status)
                .child(
                    div()
                        .text_size(px(18.))
                        .line_height(px(26.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(install.title()),
                )
                .when_some(install.error.as_ref(), |body, error| {
                    body.child(install_error(error, cx))
                }),
        );
        if let Some(attempt) = install.attempt.as_ref().filter(|attempt| attempt.total > 1) {
            body = body.child(note(
                format!(
                    "安装源 {} / {} · {}",
                    attempt.index,
                    attempt.total,
                    attempt
                        .registry
                        .as_deref()
                        .map_or_else(|| "默认安装源".into(), registry_label)
                ),
                cx,
            ));
        }
        if let Some(target) = install.purpose.target() {
            let version = target
                .from
                .as_ref()
                .filter(|from| !from.is_empty())
                .map_or_else(
                    || target.version.clone(),
                    |from| format!("{from} → {}", target.version),
                );
            body = body.child(
                column()
                    .w_full()
                    .gap(px(4.))
                    .p(px(12.))
                    .rounded(px(12.))
                    .bg(p.bg_module_platform)
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(target.title.clone()),
                    )
                    .child(note(version, cx)),
            );
        } else if let Some(subject) = &install.subject {
            let name = install
                .outcome
                .as_ref()
                .and_then(|outcome| outcome.bundle.as_deref())
                .or(subject.name.as_deref())
                .unwrap_or(match subject.source.as_str() {
                    "git" => "Git 仓库",
                    "path" => "本地目录",
                    "tarball" => "压缩包",
                    _ => "插件",
                });
            let version = install
                .outcome
                .as_ref()
                .and_then(|outcome| outcome.version.as_deref())
                .or(subject.version.as_deref());
            body = body.child(
                column()
                    .w_full()
                    .gap(px(4.))
                    .p(px(12.))
                    .rounded(px(12.))
                    .bg(p.bg_module_platform)
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(name.to_owned()),
                    )
                    .when_some(version, |body, version| {
                        body.child(note(format!("版本 {version}"), cx))
                    })
                    .when_some(subject.description.as_ref(), |body, desc| {
                        body.child(note(desc.clone(), cx))
                    }),
            );
        }
        if install.phase == Phase::Done
            && install
                .outcome
                .as_ref()
                .is_some_and(|outcome| outcome.application == "restart-required")
        {
            body = body.child(note(
                if install
                    .outcome
                    .as_ref()
                    .is_some_and(|outcome| outcome.enabled == Some(true))
                {
                    "插件已启用。请重新连接 ACP 服务，使变更生效。"
                } else {
                    "插件已安装。启用后请重新连接 ACP 服务，使变更生效。"
                },
                cx,
            ));
        }
        body.child(self.plugin_install_output(install, cx))
    }

    fn plugin_install_output(&self, install: &Installer, cx: &mut Context<Self>) -> Div {
        let p = dsh(cx);
        let mut body = column().w_full().gap(px(14.));
        if let Some(outcome) = install
            .outcome
            .as_ref()
            .filter(|outcome| !outcome.pending_builds.is_empty())
        {
            body = body.child(column().w_full().gap(px(8.)).p(px(12.)).rounded(px(12.)).bg(p.state_warn_tertiary)
                .child(note("安装需要允许以下依赖运行安装脚本。",cx))
                .child(div().font_family("monospace").text_size(px(12.)).child(outcome.pending_builds.join("\n")))
                .when(install.approve_open, |body| body.child(note(
                    "继续将保存当前 profile 的脚本许可。这些脚本在本机运行，请确认所列依赖来源可信。",cx))));
        }
        body = body.child(install_link(
            "plugin-install-details",
            if install.details_open {
                "收起安装详情"
            } else {
                "查看安装详情"
            },
            true,
            cx,
            |app, _, cx| {
                if let Some(install) = &mut app.plugin_installer {
                    install.details_open = !install.details_open;
                    if install.details_open {
                        install.log_scroll.scroll_to_bottom();
                    }
                    cx.notify();
                }
            },
        ));
        if install.details_open {
            body = body.child(
                column()
                    .id("plugin-install-log")
                    .w_full()
                    .max_h(px(190.))
                    .overflow_y_scroll()
                    .track_scroll(&install.log_scroll)
                    .p(px(10.))
                    .rounded(px(12.))
                    .bg(p.bg_layer_3)
                    .font_family("monospace")
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .text_color(p.label_secondary)
                    .child(if install.log.is_empty() {
                        "本次安装详情没有收到终端输出".into()
                    } else {
                        install.log.clone()
                    }),
            );
        }
        body
    }

    fn plugin_install_footer(&self, install: &Installer, cx: &mut Context<Self>) -> Div {
        let mut footer = row()
            .w_full()
            .justify_end()
            .gap(px(8.))
            .px(px(24.))
            .pb(px(24.));
        match install.phase {
            Phase::Confirm => {
                footer = footer
                    .child(dialog_action(
                        "cancel-plugin-update",
                        "取消",
                        false,
                        false,
                        0.,
                        cx,
                        |app, w, cx| app.dismiss_plugin_installer(w, cx),
                    ))
                    .child(dialog_action(
                        "confirm-plugin-update",
                        "更新",
                        true,
                        !self.can_install_plugins(),
                        0.,
                        cx,
                        |app, w, cx| app.begin_plugin_install(false, w, cx),
                    ));
            }
            Phase::Installing if !install.purpose.updating() => {
                footer = footer.child(super::permission_dialog::dialog_action(
                    "cancel-plugin-install",
                    "取消安装",
                    false,
                    false,
                    0.,
                    cx,
                    |app, w, cx| app.cancel_plugin_install(false, w, cx),
                ))
            }
            phase if phase.busy() && install.purpose.updating() => {
                footer = footer.child(dialog_action(
                    "plugin-update-running",
                    "正在更新",
                    false,
                    true,
                    0.,
                    cx,
                    |_, _, _| {},
                ));
            }
            Phase::Done => {
                footer = footer.child(super::permission_dialog::dialog_action(
                    "finish-plugin-install",
                    "完成",
                    false,
                    false,
                    0.,
                    cx,
                    |app, w, cx| app.dismiss_plugin_installer(w, cx),
                ));
                if !install.purpose.updating()
                    && let Some(name) = install
                        .outcome
                        .as_ref()
                        .filter(|outcome| outcome.enabled != Some(true))
                        .and_then(|outcome| outcome.bundle.clone())
                {
                    footer = footer.child(super::permission_dialog::dialog_action(
                        "enable-installed-plugin",
                        "立即启用",
                        true,
                        !self.can_install_plugins(),
                        0.,
                        cx,
                        move |app, w, cx| {
                            if app.can_install_plugins() {
                                app.dismiss_plugin_installer(w, cx);
                                app.activate_installed_plugin(name.clone(), cx);
                            }
                        },
                    ));
                }
            }
            Phase::Failed => {
                footer = footer.child(super::permission_dialog::dialog_action(
                    "edit-plugin-spec",
                    if install.purpose.target().is_some() {
                        "完成"
                    } else {
                        "返回编辑"
                    },
                    false,
                    false,
                    0.,
                    cx,
                    |app, w, cx| {
                        if app
                            .plugin_installer
                            .as_ref()
                            .is_some_and(|install| install.purpose.target().is_some())
                        {
                            app.dismiss_plugin_installer(w, cx);
                        } else if let Some(install) = &mut app.plugin_installer {
                            install.draft(w, cx);
                            cx.notify();
                        }
                    },
                ));
                if install.purpose.accepts_build_approval()
                    && install
                        .outcome
                        .as_ref()
                        .is_some_and(|outcome| !outcome.pending_builds.is_empty())
                {
                    footer = footer.child(super::permission_dialog::dialog_action(
                        "approve-plugin-scripts",
                        if install.approve_open {
                            "允许并重试"
                        } else {
                            "查看脚本授权"
                        },
                        true,
                        !self.connected(),
                        0.,
                        cx,
                        |app, w, cx| {
                            if app
                                .plugin_installer
                                .as_ref()
                                .is_some_and(|install| install.approve_open)
                            {
                                app.begin_plugin_install(true, w, cx);
                            } else if let Some(install) = &mut app.plugin_installer {
                                install.approve_open = true;
                                cx.notify();
                            }
                        },
                    ));
                }
            }
            Phase::Unknown => {
                footer = footer.child(super::permission_dialog::dialog_action(
                    "check-plugin-install",
                    "核对安装状态",
                    false,
                    !self.connected(),
                    0.,
                    cx,
                    |app, w, cx| app.observe_plugin_install(w, cx),
                ))
            }
            _ => {}
        }
        footer
    }
}

fn installer_input(editor: &Entity<Editor>, invalid: bool, w: &Window, cx: &App) -> Div {
    let p = dsh(cx);
    div()
        .w_full()
        .h(px(40.))
        .px(px(14.))
        .rounded(px(12.))
        .border(px(0.5))
        .border_color(if invalid {
            p.state_error_primary
        } else if editor.read(cx).focus.is_focused(w) {
            p.state_business_primary
        } else {
            p.border_l4
        })
        .bg(p.bg_layer_3)
        .child(editor.clone())
}

fn install_link(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    enabled: bool,
    cx: &mut Context<DshApp>,
    action: impl Fn(&mut DshApp, &mut Window, &mut Context<DshApp>) + 'static,
) -> Stateful<Div> {
    let p = dsh(cx);
    let button = row()
        .id(id)
        .role(gpui::Role::Button)
        .tab_stop(enabled)
        .min_h(px(28.))
        .px(px(8.))
        .rounded(px(8.))
        .text_size(px(12.5))
        .line_height(px(18.))
        .text_color(p.label_secondary)
        .child(label.into());
    if enabled {
        super::dock_action_button(
            button.cursor_pointer().hover(|style| style.bg(p.hover)),
            cx.listener(move |app, _, w, cx| action(app, w, cx)),
        )
    } else {
        button.opacity(0.45)
    }
}

fn install_error(message: &str, cx: &App) -> Div {
    div()
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(dsh(cx).state_error_primary)
        .child(message.to_owned())
}

pub(super) fn registry_label(url: &str) -> String {
    match url.trim_end_matches('/') {
        "https://registry.npmjs.org" => "npm 官方源".into(),
        "https://registry.npmmirror.com" => "国内镜像".into(),
        _ => url.to_owned(),
    }
}

fn inspect_problem(problem: &str) -> &'static str {
    match problem {
        "stale-version" => "插件版本已变化，请刷新清单后重试。",
        "not-updatable" => "当前安装无法更新此插件。",
        "invalid-registry" => "请输入有效的 HTTP 或 HTTPS 安装源地址。",
        "invalid-spec" => "请输入有效的包名、Git 仓库地址或本地目录。",
        "already-installed" => "此插件已安装。如需升级，请先卸载再安装。",
        "not-found" => "安装源中未找到此插件，请核对包名或更换安装源。",
        "not-a-package" => "所选路径没有可读取的插件包。",
        "not-a-bundle" => "此包没有声明 DeepSeek Harness 插件配置。",
        "network" => "无法访问安装源，请检查连接或更换安装源。",
        _ => "无法检查此插件，请核对输入并重试。",
    }
}

fn install_problem(outcome: &InstallOutcome, updating: bool) -> &'static str {
    match outcome.problem.as_deref() {
        Some("incompatible-version") => return "插件与当前 Harness 版本不兼容。",
        Some("unsupported-platform") => return "此插件不支持当前系统。",
        Some("already-installed") => return inspect_problem("already-installed"),
        Some("not-bundle") => return inspect_problem("not-a-bundle"),
        _ => {}
    }
    match outcome.failure.as_deref() {
        Some("pnpm-missing") => "当前 ACP 服务找不到 pnpm。",
        Some("network" | "timeout") if updating => "安装源连接失败或超时。",
        Some("network" | "timeout") => "安装源连接失败或超时，可在返回编辑后更换安装源。",
        Some("build-blocked") => "安装依赖的脚本等待授权。",
        Some("integrity") => "安装文件的完整性验证失败。",
        Some("disk-full") => "磁盘空间不足，无法安装插件。",
        Some("permission") => "ACP 服务没有写入插件目录的权限。",
        _ if updating => "Host 未完成插件更新。",
        _ => "Host 未完成插件安装，请查看安装详情。",
    }
}
