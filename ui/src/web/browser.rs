//! GPUI browser chrome and the placement of each tab's native child surface.

use super::menu_surface::MenuCover;
use super::{dock_controls::DockControl, start_panel::RIGHT_DOCK_HEADER_HEIGHT, *};
use crate::right_dock::{BrowserCheckpoint, BrowserObservations, FloatRect};
use gpui::{EntityInputHandler, FocusHandle, Focusable, TextAlign, svg};
use serde::{Deserialize, Serialize};

const BAR_HEIGHT: f32 = 38.;
const TOOL_SIZE: f32 = 28.;
const BAR_GAP: f32 = 4.;
const BAR_PADDING: f32 = 6.;
const ERROR_FONT: f32 = 11.;
const ERROR_LINE: f32 = 14.;
const ERROR_PAD_X: f32 = 12.;
const ERROR_PAD_Y: f32 = 6.;
const LOAD_FAILED: &str = "页面加载失败；请刷新重试或在系统浏览器中打开。";

#[derive(Clone, PartialEq, Eq, Serialize)]
struct BrowserRequest {
    navigation: u64,
    address: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BrowserAttachment {
    Pending,
    Attached,
    Failed,
}

struct BrowserLoad {
    request: BrowserRequest,
    requested_address: String,
    attachment: BrowserAttachment,
    observed: bool,
}

#[derive(Clone, PartialEq, Serialize)]
pub(super) struct BrowserPlacement {
    key: u64,
    #[serde(flatten)]
    request: BrowserRequest,
    bounds: BrowserBounds,
    frame_clip: Option<BrowserClip>,
    covers: Vec<BrowserClip>,
}

#[derive(Clone, Copy, PartialEq, Serialize)]
struct BrowserBounds {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl From<FloatRect> for BrowserBounds {
    fn from(rect: FloatRect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Serialize)]
struct BrowserClip {
    #[serde(flatten)]
    bounds: BrowserBounds,
    radius: f32,
    visual_only: bool,
}

impl From<MenuCover> for BrowserClip {
    fn from(cover: MenuCover) -> Self {
        Self {
            bounds: cover.rect.into(),
            radius: cover.radius,
            visual_only: cover.visual_only,
        }
    }
}

impl BrowserClip {
    fn new(rect: FloatRect, radius: f32) -> Self {
        Self {
            bounds: rect.into(),
            radius,
            visual_only: false,
        }
    }

    fn covers(self, bounds: FloatRect) -> bool {
        let BrowserBounds {
            x,
            y,
            width,
            height,
        } = self.bounds;
        width > 0.
            && height > 0.
            && bounds.intersects(FloatRect {
                x,
                y,
                width,
                height,
            })
    }
}

#[derive(Deserialize)]
struct BrowserPresentationResult {
    key: u64,
    navigation: u64,
    target: Option<BrowserCheckpoint>,
    error: Option<String>,
}

pub(super) struct BrowserTab {
    key: u64,
    input: gpui::Entity<Editor>,
    back_focus: FocusHandle,
    forward_focus: FocusHandle,
    address_focus: FocusHandle,
    go_focus: FocusHandle,
    external_focus: FocusHandle,
    restore_focus: FocusHandle,
    _subscriptions: [Subscription; 4],
    latest_revision: u64,
    observations: BrowserObservations,
    load: Option<BrowserLoad>,
    action_revision: u64,
    has_page: bool,
    loading: bool,
    can_back: bool,
    can_forward: bool,
    load_error: Option<String>,
    address_error: Option<String>,
}

impl BrowserTab {
    fn fail_browser_attachment(&mut self) {
        if let Some(load) = self.load.as_mut() {
            load.attachment = BrowserAttachment::Failed;
        }
        self.loading = false;
        self.has_page = false;
        self.can_back = false;
        self.can_forward = false;
        self.load_error = Some(LOAD_FAILED.into());
    }
}

impl DshApp {
    pub(super) fn advance_browser_selection(
        &self,
        prefix: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.sidebar.browsers.values().find(|tab| {
            let editor = tab.input.read(cx);
            editor.focus.is_focused(w) && !editor.is_composing()
        }) else {
            return;
        };
        tab.input.update(cx, |editor, cx| {
            if let Some(selection) = editor.selected_text_range(false, w, cx) {
                let start = selection.range.start;
                let end = start + prefix.encode_utf16().count();
                if end <= selection.range.end
                    && editor
                        .text_for_range(start..end, &mut None, w, cx)
                        .as_deref()
                        == Some(prefix)
                {
                    editor.set_selected_text_range(end..selection.range.end, w, cx);
                }
            }
        });
    }

    pub(super) fn mount_restored_browsers(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<_> = self.sidebar.surface.layout.browser_keys().collect();
        for id in ids {
            if !self.sidebar.browsers.contains_key(&id) {
                self.mount_browser(id, w, cx);
            }
        }
    }

    fn mount_browser(&mut self, tab_id: u64, w: &mut Window, cx: &mut Context<Self>) {
        self.next_browser_key += 1;
        let key = self.next_browser_key;
        let input = editor("输入 HTTP(S) 地址".into(), InputMode::Single, w, cx);
        input.update(cx, |input, _| input.font_size = 12.);
        if let Some(saved) = self.sidebar.surface.browsers.get(&tab_id) {
            replace(&input, saved.address.clone(), w, cx);
        }
        let subscription = cx.subscribe_in(&input, w, move |this, _, event: &EditEvent, w, cx| {
            if *event == EditEvent::Submitted
                && this
                    .sidebar
                    .browsers
                    .get(&tab_id)
                    .is_some_and(|tab| tab.key == key)
            {
                this.submit_browser_address(tab_id, w, cx);
            }
        });
        let focus = input.read(cx).focus_handle(cx);
        let focus_in = cx.on_focus(&focus, w, |_, _, cx| cx.notify());
        let focus_out = cx.on_blur(&focus, w, |_, _, cx| cx.notify());
        let address_focus = cx.focus_handle().tab_stop(false);
        let go_focus = cx.focus_handle().tab_stop(true);
        let address_blur = cx.on_focus_out(&address_focus, w, |_, _, _, cx| cx.notify());
        self.sidebar.browsers.insert(
            tab_id,
            BrowserTab {
                key,
                input,
                back_focus: cx.focus_handle(),
                forward_focus: cx.focus_handle(),
                address_focus,
                go_focus,
                external_focus: cx.focus_handle(),
                restore_focus: cx.focus_handle(),
                _subscriptions: [subscription, focus_in, focus_out, address_blur],
                latest_revision: 0,
                observations: BrowserObservations::default(),
                load: None,
                action_revision: 0,
                has_page: false,
                loading: false,
                can_back: false,
                can_forward: false,
                load_error: None,
                address_error: None,
            },
        );
    }

    pub(super) fn new_browser_tab(
        &mut self,
        pane: DockPaneId,
        initial: Option<String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_session.is_none() || self.sidebar.surface.layout.pane(pane).is_none() {
            return;
        }
        let id = self
            .sidebar
            .surface
            .layout
            .browser_keys()
            .max()
            .unwrap_or(0)
            + 1;
        self.mount_browser(id, w, cx);
        self.open_right_tab(pane, RightPanelTab::Browser(id), w, cx);
        if let Some(address) = initial {
            replace(&self.sidebar.browsers[&id].input, address.clone(), w, cx);
            self.request_browser_address(id, address, w, cx);
        }
    }

    fn submit_browser_address(&mut self, id: u64, w: &Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.sidebar.browsers.get(&id) {
            let address = tab.input.read(cx).value().to_owned();
            self.request_browser_address(id, address, w, cx);
        }
    }

    fn request_browser_address(
        &mut self,
        id: u64,
        address: String,
        w: &Window,
        cx: &mut Context<Self>,
    ) {
        let target = bridge::browser_address(&address)
            .map_err(|error| {
                error
                    .as_string()
                    .unwrap_or_else(|| "这个地址无效或过长。".into())
            })
            .and_then(|target| {
                serde_wasm_bindgen::from_value::<BrowserCheckpoint>(target)
                    .map_err(|_| "这个地址无效或过长。".into())
            });
        let target = match target {
            Ok(target) => target,
            Err(error) => {
                if let Some(tab) = self.sidebar.browsers.get_mut(&id) {
                    tab.address_error = Some(error);
                    cx.notify();
                }
                return;
            }
        };
        let same_document = self.sidebar.browsers.get(&id).is_some_and(|tab| {
            tab.load
                .as_ref()
                .is_some_and(|load| load.observed && load.attachment == BrowserAttachment::Attached)
        }) && self
            .sidebar
            .surface
            .browsers
            .get(&id)
            .is_some_and(|current| current.address == target.address);
        if same_document {
            self.browser_reload(id, w, cx);
        } else {
            self.begin_browser_load(id, address, target, w, cx);
        }
    }

    fn begin_browser_load(
        &mut self,
        id: u64,
        address: String,
        target: BrowserCheckpoint,
        w: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.sidebar.browsers.get_mut(&id) else {
            return;
        };
        let navigation = tab
            .load
            .as_ref()
            .map_or(1, |load| load.request.navigation + 1);
        tab.load = Some(BrowserLoad {
            request: BrowserRequest {
                navigation,
                address,
            },
            requested_address: target.address.clone(),
            attachment: BrowserAttachment::Pending,
            observed: false,
        });
        tab.action_revision += 1;
        tab.loading = true;
        tab.load_error = None;
        tab.address_error = None;
        if tab.input.read(cx).value() != target.address {
            replace(&tab.input, target.address.clone(), w, cx);
        }
        self.sidebar.surface.browsers.insert(id, target);
        self.persist_right_sidebar();
        cx.notify();
    }

    pub(super) fn close_browser_tab(&mut self, id: u64) {
        self.sidebar.surface.browsers.remove(&id);
        if let Some(tab) = self.sidebar.browsers.remove(&id)
            && let Some(shell) = self.browser_shell
        {
            invoke(
                "close_embedded_browser",
                json!({"shell": shell, "key": tab.key}),
            );
        }
    }

    pub(super) fn browser_reload(&mut self, id: u64, w: &Window, cx: &mut Context<Self>) {
        let Some(saved) = self
            .sidebar
            .surface
            .browsers
            .get(&id)
            .filter(|saved| !saved.address.is_empty())
            .cloned()
        else {
            return;
        };
        let Some(tab) = self.sidebar.browsers.get(&id) else {
            return;
        };
        let Some(load) = &tab.load else {
            self.request_browser_address(id, saved.address, w, cx);
            return;
        };
        if load.attachment == BrowserAttachment::Attached && tab.load_error.is_none() {
            self.run_browser_action(id, "reload", w, cx);
        } else {
            let address = if load.requested_address == saved.address {
                load.request.address.clone()
            } else {
                saved.address.clone()
            };
            self.begin_browser_load(id, address, saved, w, cx);
        }
    }

    fn browser_history(&mut self, id: u64, forward: bool, w: &Window, cx: &mut Context<Self>) {
        if self.sidebar.browsers.get(&id).is_some_and(|tab| {
            if forward {
                tab.can_forward
            } else {
                tab.can_back
            }
        }) {
            self.run_browser_action(id, if forward { "forward" } else { "back" }, w, cx);
        }
    }

    fn run_browser_action(
        &mut self,
        id: u64,
        action: &'static str,
        w: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(shell) = self.browser_shell else {
            return;
        };
        let Some(tab) = self.sidebar.browsers.get_mut(&id) else {
            return;
        };
        let Some(load) = tab
            .load
            .as_ref()
            .filter(|load| load.attachment == BrowserAttachment::Attached)
        else {
            return;
        };
        let navigation = load.request.navigation;
        if let Some(saved) = self.sidebar.surface.browsers.get(&id)
            && tab.input.read(cx).value() != saved.address
        {
            replace(&tab.input, saved.address.clone(), w, cx);
        }
        tab.loading = true;
        tab.address_error = None;
        tab.load_error = None;
        tab.action_revision += 1;
        let key = tab.key;
        let revision = tab.action_revision;
        cx.spawn(async move |this, cx| {
            let result = bridge::invoke(
                "embedded_browser_action",
                json!({"shell": shell, "key": key, "navigation": navigation, "action": action}),
            )
            .await;
            if result.is_err() {
                let _ = this.update(cx, |this, cx| {
                    if this.browser_shell == Some(shell)
                        && let Some((_, id, sidebar)) = this.browser_owner(key)
                        && let Some(tab) = sidebar.browsers.get_mut(&id)
                        && tab.action_revision == revision
                    {
                        tab.loading = false;
                        tab.load_error = Some(LOAD_FAILED.into());
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn browser_owner(&mut self, key: u64) -> Option<(String, u64, &mut SessionSidebar)> {
        if let Some(owner) = &self.active_session
            && let Some(id) = browser_tab_id(&self.sidebar, key)
        {
            return Some((owner.clone(), id, &mut self.sidebar));
        }
        self.parked_sidebars
            .iter_mut()
            .find_map(|(owner, sidebar)| {
                let id = browser_tab_id(sidebar, key)?;
                Some((owner.clone(), id, sidebar))
            })
    }

    fn visible_browser(&self, key: u64, cx: &App) -> Option<(u64, DockPaneId)> {
        if !self
            .browser_placements
            .iter()
            .any(|placement| placement.key == key)
        {
            return None;
        }
        let id = browser_tab_id(&self.sidebar, key)?;
        let pane = self
            .sidebar
            .surface
            .layout
            .all_panes()
            .find(|pane| pane.active() == RightPanelTab::Browser(id))?
            .id;
        self.dock_pane_on_screen(pane, cx).then_some((id, pane))
    }

    fn handle_browser_input(
        &mut self,
        key: u64,
        event: &Value,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, pane)) = self.visible_browser(key, cx) else {
            return;
        };
        self.finish_terminal_title(true, w, cx);
        // The native child still owns AppKit focus. Update dock order without
        // focusing a GPUI element over the website's active control.
        self.sidebar.surface.layout.focus(pane);
        self.dock_controls.dismiss_pinned(None);
        if event["kind"] == "browser_focus" {
            self.dismiss_header_menu_for_guest(cx);
            self.dismiss_stats_dialog(cx);
        }
        self.dismiss_dock_tab_menu(false, w, cx);
        self.dismiss_terminal_shell_menu(false, w, cx);
        if self.shortcuts.accepted_event(event)
            && let Ok(command) = serde_json::from_value(event["command"].clone())
        {
            self.run_application_shortcut(command, Some((pane, RightPanelTab::Browser(id))), w, cx);
        }
        let layout = &self.sidebar.surface.layout;
        if (layout.focused() != pane
            || !self.dock_pane_on_screen(pane, cx)
            || !layout
                .pane(pane)
                .is_some_and(|pane| pane.active() == RightPanelTab::Browser(id)))
            && let Some(shell) = self.browser_shell
        {
            invoke("focus_browser_shell", json!({"shell": shell}));
        }
        cx.notify();
    }

    pub(super) fn handle_browser_event(
        &mut self,
        event: &Value,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.browser_shell.is_none()
            || event.get("shell").and_then(Value::as_u64) != self.browser_shell
        {
            return;
        }
        let Some(key) = event.get("key").and_then(Value::as_u64) else {
            return;
        };
        let kind = event.get("kind").and_then(Value::as_str);
        if matches!(kind, Some("browser_focus" | "browser_input")) {
            self.handle_browser_input(key, event, w, cx);
            return;
        }
        if kind == Some("browser_popup") {
            if let Some((_, pane)) = self.visible_browser(key, cx)
                && let Some(address) = event.get("address").and_then(Value::as_str)
            {
                self.new_browser_tab(pane, Some(address.to_owned()), w, cx);
            }
            return;
        }
        let Some((session, id, sidebar)) = self.browser_owner(key) else {
            return;
        };
        let Some(tab) = sidebar.browsers.get_mut(&id) else {
            return;
        };
        let Some(load) = tab.load.as_mut() else {
            return;
        };
        if load.attachment == BrowserAttachment::Failed {
            return;
        }
        let navigation = load.request.navigation;
        let event_navigation = event.get("navigation").and_then(Value::as_u64);
        if kind == Some("browser_terminated") {
            if event_navigation == Some(navigation) && tab.observations.terminate(navigation) {
                tab.has_page = false;
                tab.loading = false;
                tab.can_back = false;
                tab.can_forward = false;
                tab.load_error = Some(LOAD_FAILED.into());
                cx.notify();
            }
            return;
        }
        let sequence = if let Some(event_navigation) = event_navigation {
            let Some(sequence) = event.get("sequence").and_then(Value::as_u64) else {
                return;
            };
            if event_navigation != navigation {
                return;
            }
            Some(sequence)
        } else {
            None
        };
        if !tab.observations.page(navigation, sequence) {
            return;
        }
        if sequence == Some(1) {
            // A new guest's blank bootstrap must leave the GPUI loading state visible.
            tab.has_page = false;
        }
        let page = event.get("page").unwrap_or(event);
        tab.loading = page
            .get("loading")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        tab.can_back = page
            .get("canBack")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        tab.can_forward = page
            .get("canForward")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        tab.load_error = page.get("error").and_then(Value::as_str).map(str::to_owned);
        if let Some(address) = page.get("address").and_then(Value::as_str)
            && (address.starts_with("https://") || address.starts_with("http://"))
        {
            tab.has_page = true;
            load.observed = true;
            let checkpoint = sidebar.surface.browsers.entry(id).or_default();
            let address_changed = checkpoint.address != address;
            if address_changed {
                checkpoint.address = address.to_owned();
                replace(&tab.input, address, w, cx);
                tab.address_error = None;
            }
            let mut title_changed = false;
            if let Some(title) = page.get("title").and_then(Value::as_str)
                && checkpoint.title != title
            {
                checkpoint.title = title.to_owned();
                title_changed = true;
            }
            if address_changed || title_changed {
                sidebar.persist(&session);
            }
        }
        cx.notify();
    }

    fn place_browser(
        &self,
        tab: RightPanelTab,
        panel: FloatRect,
        covers: impl Iterator<Item = BrowserClip>,
        floating: bool,
        w: &Window,
        cx: &App,
    ) -> Option<BrowserPlacement> {
        let RightPanelTab::Browser(id) = tab else {
            return None;
        };
        let tab = self.sidebar.browsers.get(&id)?;
        let load = tab.load.as_ref()?;
        let request = &load.request;
        if load.attachment == BrowserAttachment::Failed
            || (tab.observations.terminated
                && tab.observations.navigation == Some(request.navigation))
        {
            return None;
        }
        let errors = [tab.load_error.as_ref(), tab.address_error.as_ref()];
        let toolbar = RIGHT_DOCK_HEADER_HEIGHT
            + BAR_HEIGHT
            + errors
                .into_iter()
                .flatten()
                .map(|message| browser_error_height(message, panel.width, w, cx))
                .sum::<f32>();
        let bounds = FloatRect {
            x: panel.x,
            y: panel.y + toolbar,
            width: panel.width,
            height: panel.height - toolbar,
        };
        if bounds.height <= 0. || bounds.width <= 0. {
            return None;
        }
        Some(BrowserPlacement {
            key: tab.key,
            request: request.clone(),
            bounds: bounds.into(),
            frame_clip: floating
                .then(|| BrowserClip::new(panel, super::floating_panes::FLOAT_RADIUS)),
            covers: covers
                .filter(|cover| cover.covers(bounds))
                // The blank bootstrap surface would otherwise cover the GPUI opening state.
                .chain((!tab.has_page).then(|| BrowserClip::new(bounds, 0.)))
                .collect(),
        })
    }

    pub(super) fn sync_embedded_browser(&mut self, w: &Window, cx: &mut Context<Self>) {
        let Some(shell) = self.browser_shell else {
            return;
        };
        let mut placements = Vec::new();
        let menus: Vec<_> = self
            .header_menu_covers()
            .chain(self.dock_tab_menus.covers(w))
            .chain(self.stats_dialog_cover())
            .map(BrowserClip::from)
            .collect();
        let shell_menu = self
            .terminal_shell_menu_rect(w, cx)
            .map(|rect| BrowserClip::new(rect, super::terminal_shell_menu::MENU_RADIUS));
        let dock_hints: Vec<_> = self
            .dock_controls
            .hints(&self.shortcuts, w, cx)
            .into_iter()
            .filter(|hint| hint.opacity > 0.)
            .map(|hint| BrowserClip {
                visual_only: !hint.pinned,
                ..BrowserClip::new(hint.bounds, super::dock_controls::HINT_RADIUS)
            })
            .collect();
        // A child WKWebView draws above the GPUI canvas, including its drawer mask.
        if !self.sidebar_drawer_covers_window()
            && !self.session_downloads.visible()
            && !self.session_feedback.visible()
            && self.floats_on_screen(cx)
        {
            let floats = self.float_layers(w);
            if self.right_dock_frame.visible {
                for (index, pane) in self.sidebar.surface.layout.panes().iter().enumerate() {
                    let (left, width) = self
                        .sidebar
                        .surface
                        .layout
                        .pane_bounds(index, self.right_dock_frame.width);
                    let panel = FloatRect {
                        x: self.right_dock_frame.left + left,
                        y: 0.,
                        width,
                        height: w.viewport_size().height.into(),
                    };
                    let hint = (self.dock_drop_hint_pane() == Some(pane.id))
                        .then(|| BrowserClip::new(panel, 0.));
                    placements.extend(
                        self.place_browser(
                            pane.active(),
                            panel,
                            floats
                                .iter()
                                .map(|(_, rect)| {
                                    BrowserClip::new(*rect, super::floating_panes::FLOAT_RADIUS)
                                })
                                .chain(menus.iter().copied())
                                .chain(hint)
                                .chain(shell_menu)
                                .chain(dock_hints.iter().copied()),
                            false,
                            w,
                            cx,
                        ),
                    );
                }
            }
            for (index, (float, rect)) in floats.iter().enumerate() {
                let covers = floats[index + 1..]
                    .iter()
                    .map(|(_, rect)| BrowserClip::new(*rect, super::floating_panes::FLOAT_RADIUS))
                    .chain(menus.iter().copied())
                    .chain(shell_menu)
                    .chain(dock_hints.iter().copied());
                placements.extend(self.place_browser(
                    float.pane.active(),
                    *rect,
                    covers,
                    true,
                    w,
                    cx,
                ));
            }
        }
        if self.browser_placements == placements {
            return;
        }
        self.browser_revision += 1;
        let revision = self.browser_revision;
        for tab in self.sidebar.browsers.values_mut() {
            if placements.iter().any(|placement| placement.key == tab.key) {
                tab.latest_revision = revision;
            }
        }
        let owner = self
            .active_session
            .as_ref()
            .map(|session| json!({"session": session, "cwd": self.sidebar.workspace}));
        let args = json!({"shell": shell, "revision": revision, "presentations": placements, "owner": owner});
        self.browser_placements = placements;
        cx.spawn_in(w, async move |this, cx| {
            let reply = bridge::invoke("present_embedded_browser", args)
                .await
                .and_then(|value| {
                    serde_json::from_value::<Vec<BrowserPresentationResult>>(value)
                        .map_err(|error| error.to_string())
                });
            let _ = this.update_in(cx, |this, w, cx| {
                let replies = match reply {
                    Ok(replies) => replies,
                    Err(error) if this.browser_revision == revision => {
                        let mut failed_load = false;
                        for tab in this.sidebar.browsers.values_mut() {
                            if tab.latest_revision == revision
                                && tab.load.as_ref().is_some_and(|load| {
                                    load.attachment == BrowserAttachment::Pending
                                })
                            {
                                tab.fail_browser_attachment();
                                failed_load = true;
                            }
                        }
                        if !failed_load {
                            this.error = Some(error);
                        }
                        cx.notify();
                        return;
                    }
                    Err(_) => return,
                };
                for reply in replies {
                    let Some((session, id, sidebar)) = this.browser_owner(reply.key) else {
                        continue;
                    };
                    let tab = sidebar
                        .browsers
                        .get_mut(&id)
                        .expect("browser owner has the tab");
                    let Some(load) = tab.load.as_mut().filter(|load| {
                        load.attachment == BrowserAttachment::Pending
                            && load.request.navigation == reply.navigation
                            && tab.latest_revision == revision
                    }) else {
                        continue;
                    };
                    let accepted = reply.target.is_some();
                    let checkpoint_changed = if let Some(target) = reply.target
                        && !load.observed
                        && target.address != load.requested_address
                    {
                        load.requested_address.clone_from(&target.address);
                        replace(&tab.input, target.address.clone(), w, cx);
                        sidebar.surface.browsers.insert(id, target);
                        true
                    } else {
                        false
                    };
                    if let Some(error) = reply.error {
                        tab.fail_browser_attachment();
                        if !accepted {
                            tab.load_error = Some(error);
                        }
                    } else {
                        // A page observation may already have settled this navigation.
                        load.attachment = BrowserAttachment::Attached;
                    }
                    if checkpoint_changed {
                        sidebar.persist(&session);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn render_right_browser(
        &self,
        id: u64,
        floating: bool,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let Some(tab) = self.sidebar.browsers.get(&id) else {
            return div();
        };
        let p = dsh(cx);
        let checkpoint = self
            .sidebar
            .surface
            .browsers
            .get(&id)
            .filter(|saved| !saved.address.is_empty());
        let input_focused = tab.input.read(cx).focus_handle(cx).is_focused(w);
        let address_focused = tab.address_focus.contains_focused(w, cx);
        let restore = tab.load.is_none() && checkpoint.is_some();
        let external = checkpoint.map(|saved| saved.address.clone());
        column()
            .size_full()
            .gap_0()
            .bg(p.bg_base)
            .when(floating, |body| {
                let radius = px(super::floating_panes::FLOAT_RADIUS);
                body.rounded_bl(radius).rounded_br(radius)
            })
            .child(
                row()
                    .relative()
                    .w_full()
                    .h(px(BAR_HEIGHT))
                    .flex_none()
                    .px(px(BAR_PADDING))
                    .gap(px(BAR_GAP))
                    .items_center()
                    // Paint the hairline separately to keep the controls on the
                    // centered device-pixel row used by the installed toolbar.
                    .child(
                        gpui::canvas(
                            |_, _, _| {},
                            move |bounds, _, w, _| {
                                let scale = w.scale_factor();
                                let width = px((0.5 * scale).floor().max(1.) / scale);
                                let divider = gpui::Bounds::from_corners(
                                    gpui::point(bounds.left(), bounds.bottom() - width),
                                    bounds.bottom_right(),
                                );
                                w.paint_quad(gpui::fill(divider, p.border_l3));
                            },
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(
                        browser_tool(
                            "browser-back",
                            "后退",
                            BACK_ICON,
                            14.,
                            tab.can_back,
                            Some(&tab.back_focus),
                            cx,
                        )
                        .when(tab.can_back, |button| {
                            super::dock_controls::dock_action_button(
                                button,
                                cx.listener(move |this, _: &(), w, cx| {
                                    this.browser_history(id, false, w, cx);
                                }),
                            )
                        }),
                    )
                    .child(
                        browser_tool(
                            "browser-forward",
                            "前进",
                            FORWARD_ICON,
                            14.,
                            tab.can_forward,
                            Some(&tab.forward_focus),
                            cx,
                        )
                        .when(tab.can_forward, |button| {
                            super::dock_controls::dock_action_button(
                                button,
                                cx.listener(move |this, _: &(), w, cx| {
                                    this.browser_history(id, true, w, cx);
                                }),
                            )
                        }),
                    )
                    .child(if checkpoint.is_some() {
                        self.dock_button(DockControl::BrowserRefresh(id), "刷新", false, cx)
                            .group("browser-refresh")
                            .child(browser_tool_glyph(
                                "browser-refresh",
                                REFRESH_ICON,
                                16.,
                                true,
                                cx,
                            ))
                    } else {
                        browser_tool(
                            "browser-refresh",
                            "刷新",
                            REFRESH_ICON,
                            16.,
                            false,
                            None,
                            cx,
                        )
                    })
                    .child(
                        row()
                            .id("browser-address")
                            .track_focus(&tab.address_focus)
                            .tab_stop(false)
                            .h(px(28.))
                            .flex_1()
                            .min_w_0()
                            .relative()
                            .items_center()
                            .rounded(px(8.))
                            .bg(p.bg_layer_1)
                            .pl(px(9.5))
                            .pr(px(34.5))
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .rounded(px(8.))
                                    .border(px(if input_focused { 1. } else { 0.5 }))
                                    .border_color(if input_focused {
                                        p.state_business_primary
                                    } else {
                                        p.border_l2
                                    }),
                            )
                            .on_mouse_down(gpui::MouseButton::Left, {
                                let focus = tab.input.read(cx).focus_handle(cx);
                                let group = tab.address_focus.clone();
                                move |_, w, cx| {
                                    if !group.contains_focused(w, cx) {
                                        focus.focus(w, cx);
                                    }
                                }
                            })
                            .child(div().flex_1().h_full().min_w_0().child(tab.input.clone()))
                            .when(address_focused, |box_| {
                                box_.child(super::dock_controls::dock_action_button(
                                    browser_tool(
                                        "browser-go",
                                        "前往",
                                        GO_ICON,
                                        16.,
                                        true,
                                        Some(&tab.go_focus),
                                        cx,
                                    )
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .track_focus(&tab.go_focus),
                                    cx.listener(move |this, _: &(), w, cx| {
                                        this.submit_browser_address(id, w, cx);
                                    }),
                                ))
                            }),
                    )
                    .child(
                        browser_tool(
                            "browser-external",
                            "在系统浏览器中打开",
                            EXTERNAL_ICON,
                            14.,
                            external.is_some(),
                            Some(&tab.external_focus),
                            cx,
                        )
                        .when_some(external, |button, address| {
                            super::dock_controls::dock_action_button(button, move |_, _, _| {
                                invoke("open_browser_url", json!({"url": address}));
                            })
                        }),
                    ),
            )
            .children(
                [tab.load_error.as_ref(), tab.address_error.as_ref()]
                    .into_iter()
                    .flatten()
                    .map(|error| {
                        div()
                            .flex_none()
                            .w_full()
                            .min_w_0()
                            .px(px(ERROR_PAD_X))
                            .py(px(ERROR_PAD_Y))
                            .text_size(px(ERROR_FONT))
                            .line_height(px(ERROR_LINE))
                            .text_color(p.state_error_primary)
                            .bg(p.state_error_primary.opacity(0.08))
                            .child(error.clone())
                    }),
            )
            .child(
                div()
                    .id("browser-content")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .relative()
                    // Keep the sequential focus position when the restore card disappears.
                    .when(!restore, |content| {
                        content.track_focus(&tab.restore_focus.clone().tab_stop(false))
                    })
                    .children(checkpoint.filter(|_| restore).map(|saved| {
                        column()
                            .id("browser-restore-scroll")
                            .absolute()
                            .inset_0()
                            .overflow_y_scroll()
                            .items_center()
                            .p(px(24.))
                            .gap(px(10.))
                            .text_align(TextAlign::Center)
                            .child(
                                div()
                                    .mt_auto()
                                    .flex_none()
                                    .text_size(px(12.))
                                    .line_height(px(18.))
                                    .text_color(p.label_tertiary)
                                    .child("上次打开"),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(13.))
                                    .line_height(px(20.))
                                    .text_color(p.label_primary)
                                    .font_weight(FontWeight::MEDIUM)
                                    .max_w_full()
                                    .child(if saved.title.is_empty() {
                                        saved.address.clone()
                                    } else {
                                        saved.title.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(12.))
                                    .line_height(px(18.))
                                    .text_color(p.label_secondary)
                                    .max_w_full()
                                    .child(saved.address.clone()),
                            )
                            .child(super::dock_controls::dock_action_button(
                                row()
                                    .id("browser-restore")
                                    .role(gpui::Role::Button)
                                    .aria_label("恢复页面")
                                    .track_focus(&tab.restore_focus.clone().tab_stop(true))
                                    .h(px(28.))
                                    .flex_none()
                                    .items_center()
                                    .justify_center()
                                    .mb_auto()
                                    .px(px(10.))
                                    .rounded(px(8.))
                                    .cursor_pointer()
                                    .bg(p.button_primary_fill)
                                    .hover(|style| style.bg(p.button_primary_hover))
                                    .relative()
                                    .child(browser_focus_ring(
                                        tab.restore_focus.clone(),
                                        p.state_business_primary,
                                    ))
                                    .text_color(p.label_primary_foreground)
                                    .text_size(px(12.))
                                    .line_height(px(18.))
                                    .child("恢复页面"),
                                cx.listener(move |this, _: &(), w, cx| {
                                    this.browser_reload(id, w, cx);
                                }),
                            ))
                    }))
                    .when(
                        !restore
                            && tab.load_error.is_none()
                            && (checkpoint.is_none() || tab.loading),
                        |body| {
                            body.child(
                                row()
                                    .absolute()
                                    .inset_0()
                                    .justify_center()
                                    .items_center()
                                    .p(px(24.))
                                    .text_size(px(13.))
                                    .line_height(px(20.))
                                    .text_align(TextAlign::Center)
                                    .text_color(p.label_tertiary)
                                    .child(if tab.loading {
                                        "正在打开…"
                                    } else {
                                        "输入 HTTP(S) 地址开始浏览"
                                    }),
                            )
                        },
                    ),
            )
    }
}

fn browser_error_height(message: &str, width: f32, w: &Window, cx: &App) -> f32 {
    let run = gpui::TextRun {
        len: message.len(),
        font: super::document_fonts::interface_style(w, cx).font(),
        ..Default::default()
    };
    let wrap_width = px((width - 2. * ERROR_PAD_X).max(1.));
    let rows = w
        .text_system()
        .shape_text(
            message.to_owned().into(),
            px(ERROR_FONT),
            &[run],
            Some(wrap_width),
            None,
        )
        .map(|lines| {
            lines
                .iter()
                .map(|line| line.wrap_boundaries().len() + 1)
                .sum::<usize>()
        })
        .unwrap_or(1)
        .max(1);
    rows as f32 * ERROR_LINE + 2. * ERROR_PAD_Y
}

fn browser_tab_id(sidebar: &SessionSidebar, key: u64) -> Option<u64> {
    sidebar
        .browsers
        .iter()
        .find_map(|(id, tab)| (tab.key == key).then_some(*id))
}

pub(super) fn browser_focus_ring(focus: FocusHandle, color: gpui::Hsla) -> gpui::Canvas<()> {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, w, _| {
            if !focus.is_focused(w) || !bridge::keyboard_focus_visible() {
                return;
            }
            let scale = w.scale_factor();
            let align = |value: gpui::Pixels| px((f32::from(value) * scale).round() / scale);
            let bounds = gpui::Bounds::from_corners(
                gpui::point(align(bounds.left()), align(bounds.top())),
                gpui::point(align(bounds.right()), align(bounds.bottom())),
            );
            // Chromium's automatic outline has a contrast line around the themed stroke.
            for (offset, width, ink) in [(2., 1., gpui::white()), (1., 2., color)] {
                let mut ring = gpui::outline(bounds.dilate(px(offset)), ink, Default::default());
                ring.corner_radii = px(8. + offset).into();
                ring.border_widths = px(width).into();
                w.paint_quad(ring);
            }
        },
    )
    .absolute()
    .inset_0()
}

fn browser_tool(
    id: &'static str,
    label: &'static str,
    data: &'static [u8],
    size: f32,
    enabled: bool,
    focus: Option<&FocusHandle>,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let p = dsh(cx);
    row()
        .id(id)
        .group(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .size(px(TOOL_SIZE))
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .relative()
        .when_some(focus, |button, focus| {
            button.track_focus(&focus.clone().tab_stop(enabled))
        })
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(p.hover))
                .children(
                    focus.map(|focus| browser_focus_ring(focus.clone(), p.state_business_primary)),
                )
        })
        .when(!enabled, |button| {
            button.capture_any_mouse_down(|_, w, _| w.prevent_default())
        })
        .child(browser_tool_glyph(id, data, size, enabled, cx))
}

fn browser_tool_glyph(
    id: &'static str,
    data: &'static [u8],
    size: f32,
    enabled: bool,
    cx: &App,
) -> gpui::Svg {
    let p = dsh(cx);
    svg()
        .data(data)
        .size(px(size))
        // The disabled color token is unset in the installed theme.
        .text_color(if enabled {
            p.label_secondary
        } else {
            p.label_primary
        })
        .when(enabled, |glyph| {
            glyph.group_hover(id, |style| style.text_color(p.label_primary))
        })
}

const BACK_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke-width="1"><path d="M10 4L6.70711 7.29289C6.31658 7.68342 6.31658 8.31658 6.70711 8.70711L10 12" stroke="currentColor"></path></svg>"#;
const FORWARD_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke-width="1"><path d="M6 12L9.29289 8.70711C9.68342 8.31658 9.68342 7.68342 9.29289 7.29289L6 4" stroke="currentColor"></path></svg>"#;
pub(super) const REFRESH_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke-width="1"><path d="M14.5001 8C14.5 9.28552 14.1188 10.5422 13.4045 11.611C12.6903 12.6799 11.6752 13.5129 10.4875 14.0049C9.29982 14.4968 7.99295 14.6255 6.73212 14.3747C5.4713 14.124 4.31314 13.505 3.4041 12.596C2.49514 11.687 1.87614 10.5288 1.62537 9.26798C1.37459 8.00716 1.50331 6.70028 1.99525 5.51261C2.48719 4.32494 3.32025 3.30981 4.3891 2.59557C5.45795 1.88134 6.71458 1.50008 8.0001 1.5C9.9001 1.5 11.7001 2.3 13.0001 3.6L14.5001 5.1" stroke="currentColor"></path><path d="M14.4999 1.5V5.1H10.8999" stroke="currentColor"></path></svg>"#;
const EXTERNAL_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke-width="1"><path d="M11.7256 2.77441C12.5538 2.77469 13.2256 3.44616 13.2256 4.27441V10.1416H12.2256V4.27441C12.2256 3.99844 12.0015 3.77469 11.7256 3.77441H5.7207V2.77441H11.7256Z" fill="currentColor"></path><path d="M2.77441 13.2255L12.3756 3.62427" stroke="currentColor"></path></svg>"#;
const GO_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke-width="1"><path d="M6.59961 9.40051C6.82779 9.6334 7.10015 9.81842 7.40074 9.94472C7.70132 10.071 8.02409 10.1361 8.35013 10.1361C8.67618 10.1361 8.99894 10.071 9.29953 9.94472C9.60011 9.81842 9.87247 9.6334 10.1007 9.40051L12.9015 6.59967C13.3658 6.13541 13.6266 5.50572 13.6266 4.84915C13.6266 4.19258 13.3658 3.56289 12.9015 3.09863C12.4372 2.63436 11.8075 2.37354 11.151 2.37354C10.4944 2.37354 9.86472 2.63436 9.40045 3.09863L9.05034 3.44873" stroke="currentColor"></path><path d="M9.40051 6.59959C9.17233 6.3667 8.89997 6.18169 8.59939 6.05538C8.2988 5.92907 7.97603 5.86401 7.64999 5.86401C7.32395 5.86401 7.00118 5.92907 6.70059 6.05538C6.40001 6.18169 6.12765 6.3667 5.89946 6.59959L3.09863 9.40043C2.63436 9.8647 2.37354 10.4944 2.37354 11.151C2.37354 11.8075 2.63436 12.4372 3.09863 12.9015C3.56289 13.3657 4.19258 13.6266 4.84915 13.6266C5.50572 13.6266 6.13541 13.3657 6.59967 12.9015L6.94978 12.5514" stroke="currentColor"></path></svg>"#;
