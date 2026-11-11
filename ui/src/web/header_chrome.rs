//! Native titlebar clearance and the conversation's trailing controls.

use super::menu_surface::{MenuCover, menu_elevation, menu_material};
use super::session_context_menu::{
    SessionContext, SessionContextAction, SessionMenuAnchor, SessionMenuOrigin, mac_control_click,
};
use super::{dock_controls::dock_action_button, *};
use crate::anim::{
    CONTEXT_MENU_PREPARE_MS, CONTEXT_MENU_REPLACE_MS, MENU_ENTER, MENU_EXIT,
    context_menu_enter_opacity, context_menu_exit_opacity,
};
use gpui::{
    AvailableSpace, Bounds, DispatchPhase, FocusHandle, KeyDownEvent, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, Pixels, Point, ScrollHandle, Size, Task, canvas, point, size,
    svg,
};
use std::{cell::Cell, rc::Rc, sync::Arc, time::Duration};

const CHEVRON: &[u8] = include_bytes!("../../assets/header-chevron.svg");
const MORE: &[u8] = include_bytes!("../../assets/workspace-ellipsis.svg");
const CHEVRON_RIGHT: &[u8] = include_bytes!("../../assets/header-chevron-right.svg");
const CHEVRON_LEFT: &[u8] = include_bytes!("../../assets/question-chevron-left.svg");
const OPEN: &[u8] = include_bytes!("../../assets/open-in-application.svg");
const COPY: &[u8] = include_bytes!("../../assets/session-markdown-copy.svg");
const DOWNLOAD: &[u8] = include_bytes!("../../assets/session-log-download.svg");
const FEEDBACK: &[u8] = include_bytes!("../../assets/session-feedback.svg");
const APPLICATION_MENU_WIDTH: f32 = 160.;
const SESSION_MENU_WIDTH: f32 = 190.;
const MENU_MIN_WIDTH: f32 = 144.;
pub(super) const MENU_MARGIN: f32 = 12.;
pub(super) const MENU_GAP: f32 = 4.;
const SUBMENU_MIN_WIDTH: f32 = 163.;
const MENU_MAX_WIDTH: f32 = 360.;
const SESSION_ROW_HEIGHT: f32 = 34.;
const MENU_PADDING: f32 = 4.;
const CONTEXT_RADIUS: f32 = 16.;
const APPLICATION_RADIUS: f32 = 12.;
const SECTION_SEPARATOR_HEIGHT: f32 = 6.5;
const SUBMENU_POINTER_GRACE: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, PartialEq, Eq)]
enum SubmenuOpen {
    Preview,
    Entered,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SubmenuPlacement {
    Right,
    Left,
    Page,
}

#[derive(Clone, Copy)]
struct HeaderMenuBounds {
    anchor: Option<Bounds<Pixels>>,
    parent: Option<Bounds<Pixels>>,
    trigger: Option<Bounds<Pixels>>,
    child: Option<Bounds<Pixels>>,
}

impl HeaderMenuBounds {
    fn covers(self, contextual: bool) -> impl Iterator<Item = MenuCover> {
        let radius = if contextual {
            CONTEXT_RADIUS
        } else {
            APPLICATION_RADIUS
        };
        [self.parent, self.child]
            .into_iter()
            .flatten()
            .map(move |bounds| MenuCover::measured(bounds, radius, false))
    }

    fn contains(self, position: &Point<Pixels>) -> bool {
        self.anchor.is_some_and(|bounds| bounds.contains(position))
            || self.parent.is_some_and(|bounds| bounds.contains(position))
            || self.child.is_some_and(|bounds| bounds.contains(position))
    }
}

fn publish_header_bounds(
    shared: &Cell<Option<HeaderMenuBounds>>,
    bounds: Option<HeaderMenuBounds>,
    contextual: bool,
) -> Option<HeaderMenuBounds> {
    shared.set(bounds);
    for cover in bounds
        .into_iter()
        .flat_map(|bounds| bounds.covers(contextual))
    {
        super::document_html::cover_all(cover);
    }
    bounds
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderMenu {
    Applications(Option<DockPaneId>),
    Session,
    SessionContext,
}

impl HeaderMenu {
    fn contextual(self) -> bool {
        !matches!(self, Self::Applications(_))
    }
}

pub(super) struct OpenHeaderMenu {
    kind: HeaderMenu,
    requested_at: f64,
    entered_at: Option<f64>,
    last_anchor: Rc<Cell<Option<Bounds<Pixels>>>>,
    pointer_bounds: Rc<Cell<Option<HeaderMenuBounds>>>,
    context: Option<SessionContext>,
    trigger: Option<FocusHandle>,
    app_rows: Vec<FocusHandle>,
    app_scroll: ScrollHandle,
    copy: Option<FocusHandle>,
    download: Option<FocusHandle>,
    open_in: Option<FocusHandle>,
    feedback: Option<FocusHandle>,
    submenu_back: Option<FocusHandle>,
    submenu: Option<SubmenuOpen>,
    submenu_placement: Rc<Cell<Option<SubmenuPlacement>>>,
    submenu_hovered: bool,
    submenu_grace: Option<Task<()>>,
}

#[derive(Clone, Copy)]
pub(super) struct HeaderMenuAppearance {
    pub(super) app_enabled: bool,
    pub(super) pinned: bool,
    pub(super) archived: bool,
    copy_enabled: bool,
    download_enabled: bool,
}

pub(super) struct ExitingHeaderMenu {
    menu: OpenHeaderMenu,
    appearance: HeaderMenuAppearance,
    anchor: Bounds<Pixels>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    started_at: f64,
    duration_ms: f64,
}

impl OpenHeaderMenu {
    fn add_application_rows(&mut self, count: usize, cx: &mut App) {
        self.app_rows = (0..count)
            .map(|_| cx.focus_handle().tab_stop(true))
            .collect();
        if self.kind.contextual() {
            let open_in = cx.focus_handle().tab_stop(true);
            if let Some(context) = &mut self.context {
                context.rows.insert(
                    context.rows.len() - 1,
                    (SessionContextAction::OpenIn, open_in.clone()),
                );
            }
            self.open_in = Some(open_in);
            self.submenu_back = Some(cx.focus_handle().tab_stop(true));
        }
    }

    fn visible_submenu(&self) -> bool {
        match self.submenu_placement.get() {
            Some(SubmenuPlacement::Page) => self.submenu == Some(SubmenuOpen::Entered),
            Some(_) => self.submenu.is_some(),
            None => false,
        }
    }

    fn in_submenu(&self, w: &Window) -> bool {
        self.app_rows.iter().any(|row| row.is_focused(w))
            || self
                .submenu_back
                .as_ref()
                .is_some_and(|row| row.is_focused(w))
    }
    fn owns_focus(&self, w: &Window) -> bool {
        self.in_submenu(w)
            || self
                .context
                .as_ref()
                .is_some_and(|context| context.rows.iter().any(|(_, row)| row.is_focused(w)))
            || [&self.copy, &self.download, &self.open_in, &self.feedback]
                .into_iter()
                .flatten()
                .any(|row| row.is_focused(w))
    }
}

pub(super) struct WorkspaceApplication {
    id: String,
    name: String,
    icon: Option<Arc<gpui::Image>>,
}

impl DshApp {
    pub(super) fn render_leading_controls(&self, w: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        row()
            .absolute()
            .left(px(if bridge::is_mac() && !self.shell_fullscreen(w) {
                88.
            } else {
                12.
            }))
            .top(px(11.))
            .gap(px(8.))
            .child(self.sidebar_toggle_button(false, false, cx))
            .child(dock_action_button(
                row()
                    .id("titlebar-new-session")
                    .role(gpui::Role::Button)
                    .aria_label("新建会话")
                    .when_some(
                        self.shortcuts.aria(crate::shortcuts::Command::NewSession),
                        |button, aria| button.aria_keyshortcuts(aria),
                    )
                    .tab_stop(true)
                    .size(px(28.))
                    .rounded(px(8.))
                    .justify_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(p.hover))
                    .child(
                        svg()
                            .data(include_bytes!("../../assets/header-new-session.svg"))
                            .size(px(16.))
                            .text_color(p.label_secondary),
                    ),
                cx.listener(|app, _, w, cx| app.new_session(w, cx)),
            ))
    }

    fn workspace_for_session(&self, id: &str) -> Option<&str> {
        self.sessions
            .iter()
            .find(|session| session.id == id)
            .map(|session| session.cwd.as_str())
            .filter(|path| !path.is_empty())
    }

    fn session_workspace(&self) -> Option<&str> {
        self.active_session
            .as_deref()
            .and_then(|id| self.workspace_for_session(id))
    }

    fn conversation_workspace(&self) -> Option<&str> {
        self.session_workspace()
            .or_else(|| (!self.workspace.is_empty()).then_some(self.workspace.as_str()))
    }

    pub(super) fn apply_workspace_applications(
        &mut self,
        reply: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        let Ok(Value::Array(rows)) = reply else {
            return;
        };
        self.workspace_applications = rows
            .into_iter()
            .filter_map(|mut row| {
                let id = row["id"].as_str()?.to_owned();
                let name = row["name"].as_str()?.to_owned();
                let icon = serde_json::from_value::<Vec<u8>>(row["icon"].take())
                    .ok()
                    .map(|bytes| Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes)));
                Some(WorkspaceApplication { id, name, icon })
            })
            .collect();
        // The catalogue is fetched at startup and can arrive after a local
        // session's context card has opened. Retain that card and its focus.
        let count = self.workspace_applications.len();
        let add_apps = count > 0
            && self.header_menu.as_ref().is_some_and(|menu| {
                menu.kind.contextual()
                    && menu.open_in.is_none()
                    && menu.context.as_ref().map_or_else(
                        || self.session_workspace().is_some(),
                        |context| self.workspace_for_session(&context.session_id).is_some(),
                    )
            });
        if add_apps && let Some(menu) = self.header_menu.as_mut() {
            menu.add_application_rows(count, cx);
            cx.notify();
        }
    }

    fn preferred_workspace_application(&self) -> Option<&WorkspaceApplication> {
        self.workspace_applications
            .iter()
            .find(|app| Some(app.id.as_str()) == self.workspace_open_with.as_deref())
            .or_else(|| self.workspace_applications.first())
    }

    pub(super) fn open_workspace_in(&mut self, cx: &mut Context<Self>) {
        if let Some(workspace) = self.conversation_workspace().map(str::to_owned) {
            self.launch_workspace_app(workspace, None, cx);
        }
    }

    fn launch_workspace_app(
        &mut self,
        workspace: String,
        choice: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_open_pending {
            return;
        }
        let Some(application) = choice.or_else(|| {
            self.preferred_workspace_application()
                .map(|app| app.id.as_str())
        }) else {
            return;
        };
        let args =
            json!({"workspace":workspace,"application":application,"remember":choice.is_some()});
        self.workspace_open_pending = true;
        invoke("open_workspace_application", args);
        cx.notify();
    }

    pub(super) fn finish_workspace_open(
        &mut self,
        reply: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        self.workspace_open_pending = false;
        match reply {
            Ok(value) => {
                if let Some(id) = value.as_str() {
                    self.workspace_open_with = Some(id.into());
                    self.save_workspace_preferences(cx);
                }
                cx.notify();
            }
            Err(_) => self.push_toast("打开失败，请重试", cx),
        }
    }

    pub(super) fn update_header_menu(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let now = bridge::monotonic_ms();
        if self
            .exiting_header_menu
            .as_ref()
            .is_some_and(|exit| cx.reduce_motion() || now - exit.started_at >= exit.duration_ms)
        {
            self.exiting_header_menu = None;
        }
        if self.exiting_header_menu.is_some() {
            w.request_animation_frame();
        }
        let Some(menu) = &self.header_menu else {
            return;
        };
        let conversation = self.page == WorkspacePage::Conversation;
        let visible = match menu.kind {
            HeaderMenu::Applications(Some(pane)) => {
                conversation
                    && self.dock_pane_on_screen(pane, cx)
                    && self
                        .sidebar
                        .surface
                        .layout
                        .pane(pane)
                        .is_some_and(|pane| pane.active() == RightPanelTab::Files)
                    && !self.sidebar.workspace.is_empty()
            }
            HeaderMenu::Applications(None) => {
                conversation && self.conversation_workspace().is_some()
            }
            HeaderMenu::Session => {
                conversation
                    && self.active_session.is_some()
                    && !self.conversation_header_is_blank()
            }
            HeaderMenu::SessionContext => menu.context.as_ref().is_some_and(|context| {
                self.sessions
                    .iter()
                    .any(|session| session.id == context.session_id && !session.blank)
                    && match context.origin {
                        SessionMenuOrigin::Row => !self.rail_active(w),
                        SessionMenuOrigin::Title => {
                            conversation
                                && self.active_session.as_deref()
                                    == Some(context.session_id.as_str())
                        }
                    }
            }),
        };
        if !visible {
            self.header_menu = None;
        } else if menu.entered_at.is_none() {
            if now - menu.requested_at < CONTEXT_MENU_PREPARE_MS {
                w.request_animation_frame();
                return;
            }
            let first = self.header_menu_first_focus(menu).cloned();
            if let Some(menu) = self.header_menu.as_mut() {
                menu.entered_at = Some(now);
            }
            if let Some(first) = first {
                first.focus(w, cx);
            }
        }
    }

    pub(super) fn session_context_for(&self, id: &str) -> Option<&SessionContext> {
        self.header_menu
            .as_ref()
            .and_then(|menu| menu.context.as_ref())
            .filter(|context| context.session_id == id)
    }

    pub(super) fn open_session_context(
        &mut self,
        session_id: &str,
        origin: SessionMenuOrigin,
        anchor: SessionMenuAnchor,
        trigger: Option<FocusHandle>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .sessions
            .iter()
            .any(|session| session.id == session_id && !session.blank)
        {
            return;
        }
        cx.stop_propagation();
        w.prevent_default();
        if self
            .header_menu
            .as_ref()
            .and_then(|menu| menu.context.as_ref())
            .is_some_and(|context| {
                context.session_id == session_id
                    && context.origin == origin
                    && anchor.repeats(context.anchor)
            })
        {
            self.dismiss_header_menu(true, w, cx);
            return;
        }

        self.workspace_context_menu = None;
        self.show_workspace_view_options = false;
        self.show_workspace_menu = false;
        self.show_more_menu = false;
        self.composer_menu = None;

        let has_apps = self.workspace_for_session(session_id).is_some()
            && !self.workspace_applications.is_empty();
        let mut menu = self.new_action_menu(
            HeaderMenu::SessionContext,
            trigger.or_else(|| w.focused(cx)),
            has_apps,
            cx,
        );
        let archived = self.archived_sessions.contains(session_id);
        let rows = (!archived)
            .then_some(SessionContextAction::Pin)
            .into_iter()
            .chain([
                SessionContextAction::Rename,
                SessionContextAction::Fork,
                SessionContextAction::CopyId,
            ])
            .map(|action| (action, cx.focus_handle().tab_stop(true)))
            .chain(
                menu.open_in
                    .iter()
                    .cloned()
                    .map(|focus| (SessionContextAction::OpenIn, focus)),
            )
            .chain(std::iter::once((
                SessionContextAction::Archive,
                cx.focus_handle().tab_stop(true),
            )))
            .collect::<Vec<_>>();
        menu.context = Some(SessionContext {
            session_id: session_id.to_owned(),
            origin,
            anchor,
            rows,
        });
        self.begin_header_menu(menu, w, cx);
    }

    pub(super) fn render_open_session_context(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let menu = self.header_menu.as_ref()?;
        let context = menu.context.as_ref()?;
        Some(self.render_header_popup(menu, self.workspace_for_session(&context.session_id), cx))
    }

    fn toggle_header_menu(&mut self, kind: HeaderMenu, w: &mut Window, cx: &mut Context<Self>) {
        if self
            .header_menu
            .as_ref()
            .is_some_and(|menu| menu.kind == kind)
        {
            self.dismiss_header_menu(true, w, cx);
            return;
        }
        let session = kind == HeaderMenu::Session;
        if !session && self.workspace_open_pending {
            return;
        }
        let has_apps = !self.workspace_applications.is_empty()
            && (!session || self.session_workspace().is_some());
        let menu = self.new_action_menu(kind, w.focused(cx), has_apps, cx);
        self.begin_header_menu(menu, w, cx);
    }

    fn begin_header_menu(&mut self, menu: OpenHeaderMenu, w: &mut Window, cx: &mut Context<Self>) {
        self.replace_header_menu(cx);
        if let Some(focus) = if menu.entered_at.is_some() {
            self.header_menu_first_focus(&menu)
        } else {
            menu.trigger.as_ref()
        } {
            focus.focus(w, cx);
        }
        self.header_menu = Some(menu);
        cx.notify();
    }

    fn header_menu_first_focus<'a>(&self, menu: &'a OpenHeaderMenu) -> Option<&'a FocusHandle> {
        if let Some(context) = &menu.context {
            return context.rows.first().map(|(_, focus)| focus);
        }
        if !menu.kind.contextual() {
            return menu.app_rows.first();
        }
        menu.copy
            .as_ref()
            .filter(|_| !self.session_markdown.pending)
            .or(menu
                .download
                .as_ref()
                .filter(|_| !self.active_download_pending()))
            .or(menu
                .open_in
                .as_ref()
                .filter(|_| !self.workspace_open_pending))
            .or(menu.feedback.as_ref())
    }

    fn header_menu_appearance(&self, menu: &OpenHeaderMenu) -> HeaderMenuAppearance {
        let target = menu.context.as_ref().map(|context| &context.session_id);
        HeaderMenuAppearance {
            pinned: target.is_some_and(|id| self.pinned_sessions.contains(id)),
            archived: target.is_some_and(|id| self.archived_sessions.contains(id)),
            copy_enabled: !self.session_markdown.pending,
            download_enabled: !self.active_download_pending(),
            app_enabled: !self.workspace_open_pending,
        }
    }

    pub(super) fn replace_header_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.header_menu.take() {
            self.start_header_menu_exit(menu, CONTEXT_MENU_REPLACE_MS, cx);
        } else if let Some(exit) = self.exiting_header_menu.as_mut() {
            let now = bridge::monotonic_ms();
            if cx.reduce_motion() || now - exit.started_at >= exit.duration_ms {
                self.exiting_header_menu = None;
            } else {
                exit.duration_ms = CONTEXT_MENU_REPLACE_MS;
                exit.started_at = now;
            }
            cx.notify();
        }
    }

    fn start_header_menu_exit(
        &mut self,
        mut menu: OpenHeaderMenu,
        duration_ms: f64,
        cx: &mut Context<Self>,
    ) {
        if !menu.kind.contextual() || menu.entered_at.is_none() || cx.reduce_motion() {
            return;
        }
        let Some(anchor) = menu.last_anchor.get() else {
            return;
        };
        let appearance = self.header_menu_appearance(&menu);
        menu.submenu = None;
        menu.submenu_grace = None;
        self.exiting_header_menu = Some(ExitingHeaderMenu {
            menu,
            appearance,
            anchor,
            bounds: Rc::new(Cell::new(None)),
            started_at: bridge::monotonic_ms(),
            duration_ms,
        });
        cx.notify();
    }

    fn new_action_menu(
        &self,
        kind: HeaderMenu,
        trigger: Option<FocusHandle>,
        has_apps: bool,
        cx: &mut Context<Self>,
    ) -> OpenHeaderMenu {
        let header = kind == HeaderMenu::Session;
        let mut focus = || cx.focus_handle().tab_stop(true);
        let now = bridge::monotonic_ms();
        let mut menu = OpenHeaderMenu {
            kind,
            requested_at: now,
            entered_at: (!kind.contextual()).then_some(now),
            last_anchor: Rc::new(Cell::new(None)),
            pointer_bounds: Rc::new(Cell::new(None)),
            context: None,
            trigger,
            app_rows: Vec::new(),
            app_scroll: ScrollHandle::new(),
            copy: header.then(&mut focus),
            download: header.then(&mut focus),
            open_in: None,
            feedback: header.then(&mut focus),
            submenu_back: None,
            submenu: None,
            submenu_placement: Rc::new(Cell::new(None)),
            submenu_hovered: false,
            submenu_grace: None,
        };
        if has_apps {
            menu.add_application_rows(self.workspace_applications.len(), cx);
        }
        menu
    }

    pub(super) fn dismiss_header_menu(
        &mut self,
        restore_focus: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.header_menu.take() else {
            return false;
        };
        if let Some(trigger) = &menu.trigger {
            if restore_focus {
                trigger.focus(w, cx);
            } else if menu.kind.contextual()
                && menu.owns_focus(w)
                && let Some(previous) = w.focused(cx)
            {
                // An outside mouse down may focus a different control. Wait
                // until its dispatch finishes before giving focus back.
                let trigger = trigger.clone();
                let owner = cx.weak_entity();
                w.defer(cx, move |w, cx| {
                    if previous.is_focused(w)
                        && let Some(owner) = owner.upgrade()
                    {
                        owner.update(cx, |app, cx| {
                            if app.header_menu.is_none() {
                                trigger.focus(w, cx);
                            }
                        });
                    }
                });
            }
        }
        self.finish_header_menu_dismissal(menu, cx);
        true
    }

    pub(super) fn dismiss_header_menu_for_guest(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.header_menu.take() {
            self.finish_header_menu_dismissal(menu, cx);
        }
    }

    fn finish_header_menu_dismissal(&mut self, menu: OpenHeaderMenu, cx: &mut Context<Self>) {
        self.start_header_menu_exit(menu, MENU_EXIT.as_millis() as f64, cx);
        cx.notify();
    }

    pub(super) fn escape_header_menu(&mut self, w: &mut Window, cx: &mut Context<Self>) -> bool {
        if self
            .header_menu
            .as_ref()
            .is_some_and(OpenHeaderMenu::visible_submenu)
        {
            self.close_header_submenu(true, w, cx);
            return true;
        }
        self.dismiss_header_menu(true, w, cx)
    }

    fn open_header_submenu(&mut self, enter: bool, w: &mut Window, cx: &mut Context<Self>) {
        if self.workspace_open_pending {
            return;
        }
        let Some(menu) = self
            .header_menu
            .as_mut()
            .filter(|menu| menu.open_in.is_some())
        else {
            return;
        };
        menu.submenu_grace = None;
        if enter {
            menu.submenu = Some(SubmenuOpen::Entered);
            menu.app_scroll.scroll_to_item(0);
            if let Some(first) = menu.app_rows.first() {
                first.focus(w, cx);
            }
            cx.notify();
        } else if menu.submenu.is_none() {
            menu.submenu = Some(SubmenuOpen::Preview);
            menu.app_scroll.scroll_to_item(0);
            cx.notify();
        }
    }

    fn close_header_submenu(
        &mut self,
        restore_focus: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.header_menu.as_mut() else {
            return;
        };
        menu.submenu_grace = None;
        menu.submenu_hovered = false;
        if menu.submenu.take().is_none() {
            return;
        }
        if (restore_focus || menu.in_submenu(w))
            && let Some(row) = &menu.open_in
        {
            row.focus(w, cx);
        }
        cx.notify();
    }

    fn header_submenu_mouse_move(
        &mut self,
        bounds: HeaderMenuBounds,
        position: Option<Point<Pixels>>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !bridge::has_any_pointer_hover() {
            return;
        }
        let Some(menu) = self
            .header_menu
            .as_mut()
            .filter(|menu| menu.open_in.is_some())
        else {
            return;
        };
        let over_trigger =
            position.is_some_and(|point| bounds.trigger.is_some_and(|rect| rect.contains(&point)));
        let over_child =
            position.is_some_and(|point| bounds.child.is_some_and(|rect| rect.contains(&point)));
        let hovered = over_trigger || over_child;
        let was_hovered = std::mem::replace(&mut menu.submenu_hovered, hovered);
        if over_trigger {
            self.open_header_submenu(false, w, cx);
        } else if over_child {
            menu.submenu_grace = None;
        } else if was_hovered
            && menu.submenu.is_some()
            && !(menu.visible_submenu()
                && menu.submenu_placement.get() == Some(SubmenuPlacement::Page))
        {
            menu.submenu_grace = Some(cx.spawn_in(w, async move |this, cx| {
                cx.background_executor().timer(SUBMENU_POINTER_GRACE).await;
                let _ = this.update_in(cx, |app, w, cx| {
                    if app.header_menu.as_ref().is_some_and(|menu| {
                        !menu.submenu_hovered
                            && !(menu.visible_submenu()
                                && menu.submenu_placement.get() == Some(SubmenuPlacement::Page))
                    }) {
                        app.close_header_submenu(false, w, cx);
                    }
                });
            }));
        }
    }

    fn header_menu_key(&mut self, event: &KeyDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        let stroke = &event.keystroke;
        let mods = stroke.modifiers;
        if mods.control || mods.alt || mods.platform {
            return;
        }
        if stroke.key == "tab" && mods.shift {
            self.dismiss_header_menu(true, w, cx);
            w.prevent_default();
            cx.stop_propagation();
            return;
        }
        let download_pending = self.active_download_pending();
        let Some(menu) = self.header_menu.as_ref() else {
            return;
        };
        let page =
            menu.visible_submenu() && menu.submenu_placement.get() == Some(SubmenuPlacement::Page);
        let parent = menu.open_in.as_ref();
        let in_apps =
            !menu.kind.contextual() || page || (menu.visible_submenu() && menu.in_submenu(w));
        let rows: Vec<_> = if in_apps {
            menu.submenu_back
                .iter()
                .filter(|_| page)
                .chain(
                    menu.app_rows
                        .iter()
                        .filter(|_| !self.workspace_open_pending),
                )
                .collect()
        } else if let Some(context) = &menu.context {
            context
                .rows
                .iter()
                .filter(|(action, _)| {
                    *action != SessionContextAction::OpenIn || !self.workspace_open_pending
                })
                .map(|(_, focus)| focus)
                .collect()
        } else {
            menu.copy
                .as_ref()
                .filter(|_| !self.session_markdown.pending)
                .into_iter()
                .chain(menu.download.as_ref().filter(|_| !download_pending))
                .chain(parent.filter(|_| !self.workspace_open_pending))
                .chain(menu.feedback.as_ref())
                .collect()
        };
        let selected = rows.iter().position(|row| row.is_focused(w));
        match stroke.key.as_str() {
            "right" if parent.is_some_and(|row| row.is_focused(w)) => {
                self.open_header_submenu(true, w, cx)
            }
            "left" if menu.visible_submenu() => self.close_header_submenu(true, w, cx),
            key if !rows.is_empty() => {
                let index = match key {
                    "down" => selected.map_or(0, |row| (row + 1) % rows.len()),
                    "up" => selected
                        .filter(|&row| row > 0)
                        .map_or(rows.len() - 1, |row| row - 1),
                    "home" => 0,
                    "end" => rows.len() - 1,
                    _ => return,
                };
                let row = rows[index];
                if in_apps {
                    if let Some(item) = index.checked_sub(usize::from(page)) {
                        menu.app_scroll.scroll_to_item(item);
                    }
                    row.focus(w, cx);
                } else {
                    let opens_submenu = parent.is_some_and(|parent| parent == row);
                    row.focus(w, cx);
                    if opens_submenu {
                        self.open_header_submenu(false, w, cx);
                    } else {
                        self.close_header_submenu(false, w, cx);
                    }
                }
            }
            _ => return,
        }
        w.prevent_default();
        cx.stop_propagation();
    }

    pub(super) fn render_workspace_open_split(
        &self,
        pane: Option<DockPaneId>,
        workspace: &str,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        let preferred = self.preferred_workspace_application()?;
        let p = dsh(cx);
        let busy = self.workspace_open_pending;
        let kind = HeaderMenu::Applications(pane);
        let menu = self.header_menu.as_ref().filter(|menu| menu.kind == kind);
        let target = workspace.to_owned();
        let can_choose = self.workspace_applications.len() > 1;

        Some(
            row()
                .id("workspace-open-anchor")
                .relative()
                .flex_none()
                .child(
                    row()
                        .id("workspace-open-split")
                        .h(px(24.))
                        .gap_0()
                        .rounded(px(7.))
                        .border(px(0.5))
                        .border_color(p.border_l4)
                        .overflow_hidden()
                        .child(dock_action_button(
                            row()
                                .id("workspace-open-primary")
                                .role(gpui::Role::Button)
                                .aria_label(format!("用 {} 打开", preferred.name))
                                .when_some(
                                    self.shortcuts
                                        .aria(crate::shortcuts::Command::OpenWorkspace),
                                    |button, aria| button.aria_keyshortcuts(aria),
                                )
                                .tab_index(0)
                                .tab_stop(!busy)
                                .h_full()
                                .w(px(23.))
                                .justify_center()
                                .when(!busy, |button| {
                                    button.cursor_pointer().hover(|style| style.bg(p.hover))
                                })
                                .child(app_icon(preferred, 13., p.label_secondary)),
                            cx.listener(move |app, _, w, cx| {
                                app.dismiss_header_menu(false, w, cx);
                                app.launch_workspace_app(target.clone(), None, cx);
                            }),
                        ))
                        .when(can_choose, |split| {
                            split.child(dock_action_button(
                                row()
                                    .id("workspace-open-more")
                                    .role(gpui::Role::Button)
                                    .aria_label("更多打开方式")
                                    .aria_expanded(menu.is_some())
                                    .tab_index(0)
                                    .tab_stop(!busy)
                                    .h_full()
                                    .w(px(17.))
                                    .border_l(px(0.5))
                                    .border_color(p.border_l4)
                                    .justify_center()
                                    .when(!busy, |button| {
                                        button.cursor_pointer().hover(|style| style.bg(p.hover))
                                    })
                                    .child(
                                        svg()
                                            .data(CHEVRON)
                                            .size(px(10.))
                                            .text_color(p.label_secondary),
                                    ),
                                cx.listener(move |app, _, w, cx| {
                                    app.toggle_header_menu(kind, w, cx)
                                }),
                            ))
                        }),
                )
                .when_some(menu, |anchor, menu| {
                    anchor.child(self.render_header_popup(menu, Some(workspace), cx))
                }),
        )
    }

    pub(super) fn render_conversation_header_actions(
        &self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let workspace = self.conversation_workspace();
        let menu = self
            .header_menu
            .as_ref()
            .filter(|menu| menu.kind == HeaderMenu::Session);
        row()
            .id("header-utilities")
            .flex_none()
            .ml(px(20.))
            .gap(px(8.))
            .items_center()
            .children(
                workspace
                    .and_then(|workspace| self.render_workspace_open_split(None, workspace, cx)),
            )
            .child(
                row()
                    .id("header-more-anchor")
                    .relative()
                    .child(dock_action_button(
                        row()
                            .id("header-more")
                            .relative()
                            .child(self.track_header_menu_button(None))
                            .role(gpui::Role::Button)
                            .aria_label("更多操作")
                            .aria_expanded(menu.is_some())
                            .tab_index(0)
                            .size(px(28.))
                            .justify_center()
                            .rounded(px(8.))
                            .cursor_pointer()
                            .hover(|style| style.bg(p.hover))
                            .child(svg().data(MORE).size(px(15.)).text_color(p.label_secondary)),
                        cx.listener(|app, _, w, cx| {
                            app.toggle_header_menu(HeaderMenu::Session, w, cx)
                        }),
                    ))
                    .when_some(menu, |anchor, menu| {
                        anchor.child(self.render_header_popup(menu, self.session_workspace(), cx))
                    }),
            )
    }

    fn active_download_pending(&self) -> bool {
        self.active_session
            .as_deref()
            .is_some_and(|id| self.session_downloads.pending(id))
    }

    pub(super) fn leave_header_submenu(
        &mut self,
        inside: &bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if *inside && bridge::has_any_pointer_hover() {
            self.close_header_submenu(false, w, cx);
        }
    }

    pub(super) fn render_open_in_row(
        &self,
        menu: &OpenHeaderMenu,
        focus: &FocusHandle,
        enabled: bool,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        header_menu_action(
            header_menu_row(
                p,
                "open-in-application",
                interactive.then_some(focus),
                true,
                enabled,
            )
            .when(interactive, |row| {
                row.aria_expanded(menu.visible_submenu())
                    .aria_label("在应用中打开")
            })
            .child(
                svg()
                    .data(OPEN)
                    .size(px(14.))
                    .flex_none()
                    .text_color(p.label_secondary),
            )
            .child(div().flex_1().child("在应用中打开"))
            .child(
                svg()
                    .data(CHEVRON_RIGHT)
                    .size(px(14.))
                    .flex_none()
                    .text_color(p.label_secondary),
            ),
            interactive,
            cx.listener(|app, _, w, cx| app.open_header_submenu(true, w, cx)),
        )
    }

    fn render_header_parent(
        &self,
        menu: &OpenHeaderMenu,
        appearance: HeaderMenuAppearance,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        if let Some(context) = &menu.context {
            return Some(self.render_session_context_parent(
                context,
                menu,
                appearance,
                interactive,
                cx,
            ));
        }
        let p = dsh(cx);
        let copy = menu.copy.as_ref()?;
        let mut parent = menu_card(p, "session-header-commands", CONTEXT_RADIUS, interactive);
        parent = parent.child(header_menu_action(
            header_menu_row(
                p,
                "copy-session-markdown",
                interactive.then_some(copy),
                true,
                appearance.copy_enabled,
            )
            .child(svg().data(COPY).size(px(14.)).text_color(p.label_secondary))
            .child("复制会话为 Markdown")
            .when(interactive, |row| {
                row.on_hover(cx.listener(Self::leave_header_submenu))
            }),
            interactive,
            cx.listener(|app, _, w, cx| {
                if !app.session_markdown.pending {
                    app.dismiss_header_menu(true, w, cx);
                    app.copy_session_markdown(cx);
                }
            }),
        ));
        if let Some(download) = &menu.download {
            parent = parent.child(header_menu_action(
                header_menu_row(
                    p,
                    "download-session-log",
                    interactive.then_some(download),
                    true,
                    appearance.download_enabled,
                )
                .child(
                    svg()
                        .data(DOWNLOAD)
                        .size(px(14.))
                        .text_color(p.label_secondary),
                )
                .child("下载 Session 日志")
                .when(interactive, |row| {
                    row.on_hover(cx.listener(Self::leave_header_submenu))
                }),
                interactive,
                cx.listener(|app, _, w, cx| {
                    if !app.active_download_pending() {
                        app.dismiss_header_menu(true, w, cx);
                        app.start_session_download(w, cx);
                    }
                }),
            ));
        }
        if let Some(open_in) = &menu.open_in {
            parent = parent
                .child(menu_separator(p))
                .child(self.render_open_in_row(
                    menu,
                    open_in,
                    appearance.app_enabled,
                    interactive,
                    cx,
                ));
        }
        if let Some(feedback) = &menu.feedback {
            parent = parent.child(menu_separator(p)).child(header_menu_action(
                header_menu_row(
                    p,
                    "session-feedback",
                    interactive.then_some(feedback),
                    true,
                    true,
                )
                .child(
                    svg()
                        .data(FEEDBACK)
                        .size(px(14.))
                        .text_color(p.label_secondary),
                )
                .child("反馈")
                .when(interactive, |row| {
                    row.on_hover(cx.listener(Self::leave_header_submenu))
                }),
                interactive,
                cx.listener(|app, _, w, cx| {
                    app.dismiss_header_menu(true, w, cx);
                    app.open_session_feedback(w, cx);
                }),
            ));
        }
        Some(parent)
    }

    pub(super) fn render_exiting_header_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let exit = self.exiting_header_menu.as_ref()?;
        let parent = self.render_header_parent(&exit.menu, exit.appearance, false, cx)?;
        let context = exit.menu.context.is_some();
        let anchor = exit.anchor;
        let bounds = exit.bounds.clone();
        let opacity =
            context_menu_exit_opacity(bridge::monotonic_ms() - exit.started_at, exit.duration_ms);
        Some(
            canvas(
                move |_, w, cx| {
                    let measured = if let Some((card, measured)) =
                        layout_context_card(parent, context, opacity, w, cx)
                    {
                        let origin = menu_origin(anchor, measured, w.viewport_size(), true);
                        w.defer_draw(card, origin, 3, None);
                        Some(Bounds::new(origin, measured))
                    } else {
                        None
                    };
                    bounds.set(measured);
                    if let Some(bounds) = measured {
                        super::document_html::cover_all(MenuCover::measured(
                            bounds,
                            CONTEXT_RADIUS,
                            true,
                        ));
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
    }

    pub(super) fn header_menu_covers(&self) -> impl Iterator<Item = MenuCover> {
        self.header_menu
            .iter()
            .filter_map(|menu| Some((menu.pointer_bounds.get()?, menu.kind.contextual())))
            .flat_map(|(bounds, contextual)| bounds.covers(contextual))
            .chain(
                self.exiting_header_menu
                    .iter()
                    .filter_map(|exit| exit.bounds.get())
                    .map(|bounds| MenuCover::measured(bounds, CONTEXT_RADIUS, true)),
            )
    }

    pub(super) fn header_menu_pointer_watch(&self, cx: &Context<Self>) -> impl IntoElement {
        let owner = cx.weak_entity();
        canvas(
            |_, _, _| (),
            move |_, (), w, _| {
                w.on_mouse_event(move |event: &MouseDownEvent, phase, w, cx| {
                    if phase == DispatchPhase::Capture
                        && let Some(owner) = owner.upgrade()
                    {
                        owner.update(cx, |app, cx| app.header_menu_pointer_down(event, w, cx));
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }

    fn header_menu_pointer_down(
        &mut self,
        event: &MouseDownEvent,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.header_menu.as_ref() else {
            return;
        };
        let contextual = menu.kind.contextual();
        let outside = !menu
            .pointer_bounds
            .get()
            .is_some_and(|bounds| bounds.contains(&event.position));
        let secondary = event.button == gpui::MouseButton::Right
            || event.button == gpui::MouseButton::Left && mac_control_click(event.modifiers);
        if contextual && secondary {
            // Let the new gesture replace or toggle this occurrence before
            // dismissing on an otherwise unhandled secondary click.
            let owner = cx.weak_entity();
            let occurrence = menu.submenu_placement.clone();
            w.defer(cx, move |w, cx| {
                if let Some(owner) = owner.upgrade() {
                    owner.update(cx, |app, cx| {
                        if app
                            .header_menu
                            .as_ref()
                            .is_some_and(|menu| Rc::ptr_eq(&menu.submenu_placement, &occurrence))
                        {
                            app.dismiss_header_menu(!outside, w, cx);
                        }
                    });
                }
            });
        } else if outside
            && !(contextual
                && event.button == gpui::MouseButton::Left
                && self
                    .header_menu_buttons
                    .borrow()
                    .iter()
                    .any(|button| button.contains(&event.position)))
        {
            self.dismiss_header_menu(false, w, cx);
        }
    }

    pub(super) fn track_header_menu_button(
        &self,
        anchor: Option<Rc<Cell<Option<Bounds<Pixels>>>>>,
    ) -> impl IntoElement {
        let buttons = self.header_menu_buttons.clone();
        canvas(
            move |bounds, _, _| {
                buttons.borrow_mut().push(bounds);
                if let Some(anchor) = anchor {
                    anchor.set(Some(bounds));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    fn render_header_popup(
        &self,
        menu: &OpenHeaderMenu,
        workspace: Option<&str>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = dsh(cx);
        let session = menu.kind.contextual();
        let context_anchor = menu.context.as_ref().map(|context| context.anchor);
        let parent_trigger_top = menu
            .context
            .as_ref()
            .and_then(|context| {
                context
                    .rows
                    .iter()
                    .position(|(action, _)| *action == SessionContextAction::OpenIn)
            })
            .map_or(
                MENU_PADDING + 2. * SESSION_ROW_HEIGHT + SECTION_SEPARATOR_HEIGHT,
                |index| {
                    // The context card's Copy ID separator precedes the application row.
                    MENU_PADDING + index as f32 * SESSION_ROW_HEIGHT + SECTION_SEPARATOR_HEIGHT
                },
            );
        let preferred = self
            .preferred_workspace_application()
            .map(|app| app.id.as_str());
        let applications = workspace
            .filter(|_| !session || menu.submenu.is_some())
            .map(|workspace| {
                let mut rows = column()
                    .id("workspace-application-menu-rows")
                    .gap_0()
                    .w_full()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&menu.app_scroll);
                for (app, focus) in self.workspace_applications.iter().zip(&menu.app_rows) {
                    let target = workspace.to_owned();
                    let id = app.id.clone();
                    let label = if !session && preferred == Some(app.id.as_str()) {
                        format!("{}（默认）", app.name)
                    } else {
                        app.name.clone()
                    };
                    rows = rows.child(header_menu_action(
                        header_menu_row(
                            p,
                            id.clone(),
                            Some(focus),
                            session,
                            !self.workspace_open_pending,
                        )
                        .child(app_icon(app, 14., p.label_secondary))
                        .child(div().flex_1().min_w_0().truncate().child(label)),
                        true,
                        cx.listener(move |app, _, w, cx| {
                            if !app.workspace_open_pending {
                                app.dismiss_header_menu(true, w, cx);
                                app.launch_workspace_app(target.clone(), Some(&id), cx);
                            }
                        }),
                    ));
                }
                rows
            });
        let parent = self
            .render_header_parent(menu, self.header_menu_appearance(menu), true, cx)
            .map(|parent| {
                parent
                    .on_key_down(cx.listener(|app, event, w, cx| app.header_menu_key(event, w, cx)))
            });
        let app_names: Vec<_> = self
            .workspace_applications
            .iter()
            .map(|app| app.name.clone())
            .collect();
        let back = menu.submenu_back.as_ref().map(|focus| {
            header_menu_action(
                header_menu_row(p, "header-submenu-back", Some(focus), true, true)
                    .child(
                        svg()
                            .data(CHEVRON_LEFT)
                            .size(px(14.))
                            .text_color(p.label_secondary),
                    )
                    .child("返回"),
                true,
                cx.listener(|app, _, w, cx| app.close_header_submenu(true, w, cx)),
            )
        });
        let back_focus = menu.submenu_back.clone();
        let first_app = menu.app_rows.first().cloned();
        let app_scroll = menu.app_scroll.clone();
        let app_key = cx.listener(|app, event, w, cx| app.header_menu_key(event, w, cx));
        let view = cx.weak_entity();
        let focus_owner = view.clone();
        let placement = menu.submenu_placement.clone();
        let pointer_bounds = menu.pointer_bounds.clone();
        let submenu = menu.submenu;
        let entered_at = menu.entered_at;
        let last_anchor = menu.last_anchor.clone();
        canvas(
            move |anchor, w, cx| {
                let trigger_bounds = context_anchor
                    .and_then(SessionMenuAnchor::trigger_bounds)
                    .or_else(|| context_anchor.is_none().then_some(anchor));
                let anchor = context_anchor.map_or(anchor, |context| {
                    Bounds::new(context.position(), size(px(0.), px(0.)))
                });
                last_anchor.set(Some(anchor));
                let viewport = w.viewport_size();
                let margin = px(MENU_MARGIN);
                let max_width = viewport.width - margin * 2.;
                let max_height = viewport.height - margin * 2.;
                if max_width <= px(0.) || max_height <= px(0.) {
                    return publish_header_bounds(&pointer_bounds, None, session);
                }

                let mut bounds = HeaderMenuBounds {
                    anchor: trigger_bounds,
                    parent: None,
                    trigger: None,
                    child: None,
                };
                let Some(entered_at) = entered_at else {
                    return publish_header_bounds(&pointer_bounds, Some(bounds), session);
                };
                let natural = size(AvailableSpace::MaxContent, AvailableSpace::MaxContent);
                if !session {
                    if let Some(applications) = applications {
                        let mut card =
                            menu_card(p, "workspace-applications", APPLICATION_RADIUS, true)
                                .w(px(APPLICATION_MENU_WIDTH).min(max_width))
                                .max_h(max_height)
                                .child(applications)
                                .on_key_down(app_key)
                                .into_any_element();
                        let measured = card.layout_as_root(natural, w, cx);
                        let position = menu_origin(anchor, measured, viewport, false);
                        bounds.parent = Some(Bounds::new(position, measured));
                        w.defer_draw(card, position, 4, None);
                    }
                    return publish_header_bounds(&pointer_bounds, Some(bounds), session);
                }

                let elapsed = bridge::monotonic_ms() - entered_at;
                let opacity = if !cx.reduce_motion() && elapsed < MENU_ENTER.as_millis() as f64 {
                    w.request_animation_frame();
                    context_menu_enter_opacity(elapsed)
                } else {
                    1.
                };
                let Some((card, measured)) = parent.and_then(|parent| {
                    layout_context_card(parent, context_anchor.is_some(), opacity, w, cx)
                }) else {
                    return publish_header_bounds(&pointer_bounds, None, session);
                };
                let width = measured.width;
                let origin = menu_origin(anchor, measured, viewport, true);

                let child_content_width =
                    submenu.map(|_| application_menu_content_width(&app_names, w, cx));
                let child_width = child_content_width.map(|width| {
                    width
                        .clamp(px(SUBMENU_MIN_WIDTH), px(MENU_MAX_WIDTH))
                        .min(max_width)
                });
                let next = child_width.map(|child_width| {
                    submenu_placement(origin.x, width, child_width, viewport.width)
                });
                let previous = placement.replace(next);
                if previous != next {
                    w.request_animation_frame();
                }
                let was_page = previous == Some(SubmenuPlacement::Page);
                let page =
                    next == Some(SubmenuPlacement::Page) && submenu == Some(SubmenuOpen::Entered);
                if was_page
                    && !page
                    && back_focus.as_ref().is_some_and(|back| back.is_focused(w))
                    && let Some(first) = first_app
                {
                    let occurrence = placement.clone();
                    w.defer(cx, move |w, cx| {
                        if let Some(owner) = focus_owner.upgrade() {
                            owner.update(cx, |app, cx| {
                                if app.header_menu.as_ref().is_some_and(|menu| {
                                    Rc::ptr_eq(&menu.submenu_placement, &occurrence)
                                }) {
                                    app_scroll.scroll_to_item(0);
                                    first.focus(w, cx);
                                }
                            });
                        }
                    });
                }

                if !page {
                    bounds.parent = Some(Bounds::new(origin, measured));
                    if back.is_some() {
                        bounds.trigger = Some(Bounds::new(
                            point(
                                origin.x + px(MENU_PADDING),
                                origin.y + px(parent_trigger_top),
                            ),
                            size(width - px(2. * MENU_PADDING), px(SESSION_ROW_HEIGHT)),
                        ));
                    }
                }
                if !page {
                    w.defer_draw(card, origin, 4, None);
                }

                if let (Some(side), Some(rows), Some(child_width), Some(content_width)) =
                    (next, applications, child_width, child_content_width)
                    && (side != SubmenuPlacement::Page || page)
                {
                    let child_width = if page {
                        content_width.max(width).min(max_width)
                    } else {
                        child_width
                    };
                    let mut child = menu_card(p, "workspace-applications", CONTEXT_RADIUS, true)
                        .w(child_width)
                        .max_h(if page {
                            viewport.height - margin - origin.y
                        } else {
                            max_height
                        })
                        .when(page, |child| child.children(back).child(menu_separator(p)))
                        .child(rows)
                        .on_key_down(app_key)
                        .into_any_element();
                    let measured = child.layout_as_root(natural, w, cx);
                    let x = match side {
                        SubmenuPlacement::Right => origin.x + width + px(MENU_GAP),
                        SubmenuPlacement::Left => origin.x - child_width - px(MENU_GAP),
                        // Chromium shifts a max-content page left when keeping
                        // the parent anchor would put its far edge off-screen.
                        SubmenuPlacement::Page => origin.x.min(viewport.width - measured.width),
                    };
                    let y = bounds.trigger.map_or(origin.y, |trigger| {
                        trigger
                            .top()
                            .clamp(margin, viewport.height - margin - measured.height)
                    });
                    let position = point(x, y);
                    bounds.child = Some(Bounds::new(position, measured));
                    w.defer_draw(child, position, 5, None);
                }
                publish_header_bounds(&pointer_bounds, Some(bounds), session)
            },
            move |_, bounds, w, _| {
                let Some(bounds) = bounds else { return };
                if session {
                    let owner = view.clone();
                    w.on_mouse_event(move |event: &MouseMoveEvent, phase, w, cx| {
                        if phase == DispatchPhase::Capture
                            && event.pressed_button.is_none()
                            && let Some(owner) = owner.upgrade()
                        {
                            owner.update(cx, |app, cx| {
                                app.header_submenu_mouse_move(bounds, Some(event.position), w, cx);
                            });
                        }
                    });
                    w.on_mouse_event(move |_: &MouseExitEvent, phase, w, cx| {
                        if phase == DispatchPhase::Capture
                            && let Some(owner) = view.upgrade()
                        {
                            owner.update(cx, |app, cx| {
                                app.header_submenu_mouse_move(bounds, None, w, cx)
                            });
                        }
                    });
                }
            },
        )
        .absolute()
        .inset_0()
    }
}

fn layout_context_card(
    card: gpui::Stateful<gpui::Div>,
    natural_width: bool,
    opacity: f32,
    w: &mut Window,
    cx: &mut App,
) -> Option<(gpui::AnyElement, Size<Pixels>)> {
    let viewport = w.viewport_size();
    let max_width = (viewport.width - px(2. * MENU_MARGIN)).min(px(MENU_MAX_WIDTH));
    let max_height = viewport.height - px(2. * MENU_MARGIN);
    if max_width <= px(0.) || max_height <= px(0.) {
        return None;
    }
    let card = card.max_h(max_height).opacity(opacity);
    let mut card = if natural_width {
        card.min_w(px(MENU_MIN_WIDTH).min(max_width))
            .max_w(max_width)
    } else {
        card.w(px(SESSION_MENU_WIDTH).min(max_width))
    }
    .into_any_element();
    let measured = card.layout_as_root(
        size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
        w,
        cx,
    );
    Some((card, measured))
}

fn submenu_placement(
    left: Pixels,
    width: Pixels,
    child: Pixels,
    viewport: Pixels,
) -> SubmenuPlacement {
    let needed = child + px(MENU_GAP);
    if left + width + needed <= viewport - px(MENU_MARGIN) {
        SubmenuPlacement::Right
    } else if left - needed >= px(MENU_MARGIN) {
        SubmenuPlacement::Left
    } else {
        SubmenuPlacement::Page
    }
}

fn menu_x(anchor: Bounds<Pixels>, width: Pixels, viewport: Pixels, align_start: bool) -> Pixels {
    let rightmost = (viewport - px(MENU_MARGIN) - width).max(px(MENU_MARGIN));
    if align_start {
        anchor.left().clamp(px(MENU_MARGIN), rightmost)
    } else {
        (anchor.right() - width).clamp(px(MENU_MARGIN), rightmost)
    }
}

pub(super) fn menu_origin(
    anchor: Bounds<Pixels>,
    menu: Size<Pixels>,
    viewport: Size<Pixels>,
    align_start: bool,
) -> Point<Pixels> {
    let margin = px(MENU_MARGIN);
    let below = anchor.bottom() + px(MENU_GAP);
    let above = anchor.top() - px(MENU_GAP) - menu.height;
    let bottom_limit = (viewport.height - margin - menu.height).max(margin);
    let y = if below <= bottom_limit {
        below.max(margin)
    } else if above >= margin {
        above.min(bottom_limit)
    } else {
        margin
    };
    point(menu_x(anchor, menu.width, viewport.width, align_start), y)
}

fn application_menu_content_width(names: &[String], w: &Window, cx: &App) -> Pixels {
    let style = super::document_fonts::interface_style(w, cx);
    let text_width = names
        .iter()
        .map(|name| {
            w.text_system()
                .shape_line(
                    name.clone().into(),
                    px(13.),
                    &[style.to_run(name.len())],
                    None,
                )
                .width()
        })
        .fold(px(0.), Pixels::max);
    // Card padding, row padding, the 14px application icon, and its 6px gap.
    text_width.ceil() + px(44.)
}

pub(super) fn menu_separator(p: crate::tokens::Tokens) -> gpui::Div {
    div()
        .h(px(0.5))
        .flex_none()
        .mx(px(8.))
        .my(px(3.))
        .bg(p.border_l2)
}

pub(super) fn header_menu_action(
    row: gpui::Stateful<gpui::Div>,
    interactive: bool,
    action: impl Fn(&(), &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    if !interactive {
        return row;
    }
    let action = Rc::new(action);
    let tab = action.clone();
    dock_action_button(row, move |event, w, cx| action(event, w, cx)).capture_key_down(
        move |event, w, cx| {
            if event.keystroke.key == "tab" && !event.keystroke.modifiers.modified() {
                tab(&(), w, cx);
                w.prevent_default();
                cx.stop_propagation();
            }
        },
    )
}

fn app_icon(app: &WorkspaceApplication, size: f32, color: gpui::Hsla) -> gpui::AnyElement {
    app.icon.as_ref().map_or_else(
        || {
            svg()
                .data(OPEN)
                .size(px(size))
                .flex_none()
                .text_color(color)
                .into_any_element()
        },
        |data| {
            gpui::img(data.clone())
                .size(px(size))
                .flex_none()
                .into_any_element()
        },
    )
}

pub(super) fn menu_card(
    p: crate::tokens::Tokens,
    id: &'static str,
    radius: f32,
    interactive: bool,
) -> gpui::Stateful<gpui::Div> {
    column()
        .id(id)
        .when(interactive, |card| card.role(gpui::Role::Menu).occlude())
        .p(px(MENU_PADDING))
        .gap_0()
        .rounded(px(radius))
        .child(menu_material(p, radius))
        .shadow(menu_elevation(p))
}

pub(super) fn header_menu_row(
    p: crate::tokens::Tokens,
    id: impl Into<gpui::ElementId>,
    focus: Option<&FocusHandle>,
    session: bool,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    row()
        .id(id)
        .when_some(focus, |row, focus| {
            row.role(gpui::Role::MenuItem).track_focus(focus)
        })
        .w_full()
        .h(px(if session { SESSION_ROW_HEIGHT } else { 30. }))
        .flex_none()
        .px(px(8.))
        .gap(px(6.))
        .rounded(px(12.))
        .when(focus.is_some() && enabled, |row| {
            row.cursor_pointer()
                .hover(|style| style.bg(p.hover))
                .focus_visible(|style| style.bg(p.hover))
        })
        .when(!enabled, |row| {
            row.opacity(0.4).when(focus.is_some(), |row| {
                row.tab_stop(false).cursor_not_allowed()
            })
        })
        .text_size(px(13.))
        .line_height(px(20.))
        .text_color(p.label_primary)
}
