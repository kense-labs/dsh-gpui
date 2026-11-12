//! Right-hand workspace dock: Guide / Files / Terminal / Browser（对齐官方 sidebar-right）。

use super::dock_controls::{DockControl, traverse_dock_focus};
use super::dock_drag::{DockGeometry, dock_tab_separator};
use super::dock_strip::{dock_chip, dock_chip_title};
use super::workspace::{begin_window_drag, conversation_bg, dark};
use super::*;
use crate::right_dock::{
    DockPane, auto_fullscreen, clamp_dock_preference, default_dock_width, dock_width,
};
use gpui::svg;

pub(super) const RIGHT_DOCK_HEADER_HEIGHT: f32 = 38.;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DockResize {
    Outer,
    Split,
}

pub(super) struct DockDrag {
    kind: DockResize,
    origin: f32,
    width: f32,
    extent: f32,
    moved: bool,
}

struct DockResizeGhost;

impl Render for DockResizeGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

impl DshApp {
    pub(super) fn right_panel_visible(&self) -> bool {
        self.active_session.is_some()
            && self.sidebar.surface.open
            && self.page == WorkspacePage::Conversation
    }

    pub(super) fn update_right_panel_frame(&mut self, w: &Window, cx: &Context<Self>) {
        let viewport = f32::from(w.viewport_size().width);
        let sidebar = self.sidebar_track_width(w);
        let preference = self
            .right_dock_preferred_width
            .unwrap_or_else(|| default_dock_width(viewport));
        let width = dock_width(viewport, sidebar, preference);
        let auto_fullscreen = auto_fullscreen(viewport);
        let fullscreen = self.sidebar.surface.fullscreen || auto_fullscreen;
        let on_screen = self.page == WorkspacePage::Conversation
            && self.active_session.is_some()
            && !self.show_welcome;
        let open = on_screen && self.right_panel_visible() && (fullscreen || width > 0.);
        if open && !auto_fullscreen {
            self.right_dock_preferred_width.get_or_insert(preference);
        }
        if !open {
            self.right_dock_drag = None;
        }
        if !open || self.show_settings || self.shortcuts.open || !w.is_window_active() {
            self.dock_tab_gesture = None;
        }
        let animate =
            on_screen && !cx.reduce_motion() && self.right_dock_viewport == Some(viewport);
        self.right_dock_viewport = on_screen.then_some(viewport);
        let (progress, running) =
            self.right_dock_slide
                .update(open, bridge::monotonic_ms(), animate);
        let track_width = if auto_fullscreen { 0. } else { width };
        self.right_dock_frame = DockFrame::new(viewport, track_width, fullscreen, progress);
        if running {
            w.request_animation_frame();
        }
    }

    pub(super) fn toggle_right_panel(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.toggle_right_panel_from(self.focused_right_pane(w, cx), w, cx);
    }

    pub(super) fn toggle_right_panel_from(
        &mut self,
        focused: Option<DockPaneId>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_session.is_none() {
            return;
        }
        self.sidebar.surface.open = !self.sidebar.surface.open;
        if self.sidebar.surface.open {
            self.narrow_sidebar_expanded = false;
            if !focused.is_some_and(DockPaneId::is_floating) {
                self.focus_dock_pane(self.sidebar.surface.layout.active_dock_pane(), w, cx);
                if let RightPanelTab::Terminal(key) = self.sidebar.surface.layout.active() {
                    self.focus_terminal(key, w, cx);
                }
            }
        } else if focused.is_some_and(|pane| !pane.is_floating()) {
            self.focus_composer(w, cx);
        }
        cx.notify();
    }

    pub(super) fn focused_right_pane(&self, w: &Window, cx: &App) -> Option<DockPaneId> {
        self.sidebar.surface.layout.all_panes().find_map(|pane| {
            if !self.dock_pane_on_screen(pane.id, cx) {
                return None;
            }
            let handle = self.right_dock_focus.get(&pane.id)?;
            (handle.is_focused(w) || handle.contains_focused(w, cx)).then_some(pane.id)
        })
    }

    pub(super) fn focus_dock_pane(
        &mut self,
        pane: DockPaneId,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        super::document_html::restore_shell_focus();
        self.sidebar.surface.layout.focus(pane);
        let handle = self
            .right_dock_focus
            .entry(pane)
            .or_insert_with(|| cx.focus_handle());
        if !handle.contains_focused(w, cx) {
            handle.focus(w, cx);
        }
    }

    fn dock_start_inset(&self, w: &Window) -> f32 {
        if self.right_dock_frame.fullscreen && bridge::is_mac() && !self.shell_fullscreen(w) {
            88.
        } else {
            10.
        }
    }

    fn split_block(&self, pane: &DockPane, start_inset: f32) -> Option<&'static str> {
        if self.sidebar.surface.layout.panes().len() == 2 {
            return Some("已达两格上限");
        }
        let add_tab_space = if pane.tabs().contains(&RightPanelTab::Guide) {
            0.
        } else {
            32.
        };
        // Each half needs the strip's fixed controls and one 100px tab box.
        let minimum = start_inset + 114. + add_tab_space + 100.;
        (self.right_dock_frame.width * 0.5 < minimum).then_some("栏宽不足，拖宽侧边栏后再分栏")
    }

    pub(super) fn split_right_panel(
        &mut self,
        pane: DockPaneId,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if pane.is_floating() {
            return;
        }
        let Some(target) = self.sidebar.surface.layout.pane(pane) else {
            return;
        };
        if self.split_block(target, self.dock_start_inset(w)).is_some() {
            return;
        }
        if let Some(created) = self.sidebar.surface.layout.split(pane) {
            self.focus_dock_pane(created, w, cx);
            cx.notify();
        }
    }

    pub(super) fn toggle_right_fullscreen(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if auto_fullscreen(f32::from(w.viewport_size().width)) {
            self.sidebar.surface.fullscreen = false;
            self.toggle_right_panel(w, cx);
        } else {
            self.sidebar.surface.fullscreen = !self.sidebar.surface.fullscreen;
            cx.notify();
        }
    }

    pub(super) fn render_dock_resize_handle(
        &self,
        kind: DockResize,
        edge: f32,
        width: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        div()
            .id(match kind {
                DockResize::Outer => "right-dock-resize-handle",
                DockResize::Split => "right-dock-divider",
            })
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(edge - 4.))
            .w(px(8.))
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .when(kind == DockResize::Split, |handle| {
                handle
                    .flex()
                    .justify_center()
                    .child(div().h_full().w(px(0.5)).bg(p.border_l4))
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, _| {
                    this.right_dock_drag = Some(DockDrag {
                        kind,
                        origin: f32::from(event.position.x),
                        width,
                        extent: this.right_dock_frame.width,
                        moved: false,
                    });
                }),
            )
            .on_drag(kind, |_, _, _, cx| cx.new(|_| DockResizeGhost))
            .on_drag_move(cx.listener(
                move |this, event: &gpui::DragMoveEvent<DockResize>, w, cx| {
                    this.resize_dock(kind, event.event.position.x, w, cx);
                },
            ))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseUpEvent, w, cx| {
                    this.finish_dock_resize(kind, event.position.x, w, cx);
                }),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseUpEvent, w, cx| {
                    this.finish_dock_resize(kind, event.position.x, w, cx);
                }),
            )
    }

    fn resize_dock(
        &mut self,
        kind: DockResize,
        position: gpui::Pixels,
        w: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self
            .right_dock_drag
            .as_mut()
            .filter(|drag| drag.kind == kind)
        else {
            return;
        };
        let delta = f32::from(position) - drag.origin;
        if !drag.moved && delta.abs() < 1. {
            return;
        }
        drag.moved = true;
        match kind {
            DockResize::Outer => {
                self.right_dock_preferred_width = Some(clamp_dock_preference(
                    f32::from(w.viewport_size().width),
                    drag.width - delta,
                ));
            }
            DockResize::Split => {
                self.sidebar
                    .surface
                    .layout
                    .set_split_fraction((drag.width + delta) / drag.extent);
            }
        }
        cx.notify();
    }

    fn finish_dock_resize(
        &mut self,
        kind: DockResize,
        position: gpui::Pixels,
        w: &Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .right_dock_drag
            .as_ref()
            .is_some_and(|drag| drag.kind == kind)
        {
            // The release can be newer than the last dispatched move.
            self.resize_dock(kind, position, w, cx);
            self.right_dock_drag = None;
        }
    }

    pub(super) fn render_start_panel(
        &self,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let frame = self.right_dock_frame;
        let panes = self.sidebar.surface.layout.panes();
        let geometry = DockGeometry::new(panes);
        let viewport_height = f32::from(w.viewport_size().height);
        row()
            .id(gpui::ElementId::Name(
                format!(
                    "right-dock-{}",
                    self.active_session.as_deref().unwrap_or_default()
                )
                .into(),
            ))
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(frame.left))
            .w(px(frame.width))
            .gap_0()
            .border_l(px(0.5))
            .border_color(p.border_l3)
            .bg(conversation_bg(cx))
            .occlude()
            .children(panes.iter().enumerate().map(|(index, pane)| {
                let id = pane.id;
                let (_, width) = self.sidebar.surface.layout.pane_bounds(index, frame.width);
                let start_inset = if index == 0 {
                    self.dock_start_inset(w)
                } else {
                    10.
                };
                column()
                    .id(gpui::ElementId::Name(
                        format!("right-dock-pane-{id:?}").into(),
                    ))
                    .relative()
                    .track_focus(&self.right_dock_focus[&id])
                    .tab_stop(false)
                    .when(
                        matches!(
                            pane.active(),
                            RightPanelTab::Browser(_)
                                | RightPanelTab::Files
                                | RightPanelTab::Document(_)
                        ),
                        |pane| {
                            pane.on_key_down(|event, w, cx| {
                                traverse_dock_focus(event, w, cx);
                            })
                        },
                    )
                    .w(px(width))
                    .h_full()
                    .min_w_0()
                    .gap_0()
                    .flex_none()
                    .overflow_hidden()
                    .child(geometry.measure_pane(id, self.split_block(pane, start_inset).is_none()))
                    .child(self.render_right_tab_strip(
                        pane,
                        start_inset,
                        index + 1 == panes.len(),
                        &geometry,
                        w,
                        cx,
                    ))
                    .child(
                        div()
                            .id("dock-body")
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .capture_any_mouse_down(cx.listener(move |this, _, w, cx| {
                                this.focus_dock_pane(id, w, cx);
                            }))
                            .child(self.render_dock_body(
                                id,
                                pane.active(),
                                width,
                                viewport_height,
                                w,
                                cx,
                            ))
                            .children(self.dock_drop_hint(id, cx)),
                    )
            }))
            .when(panes.len() == 2, |panel| {
                let (_, left_width) = self.sidebar.surface.layout.pane_bounds(0, frame.width);
                panel.child(self.render_dock_resize_handle(
                    DockResize::Split,
                    left_width,
                    left_width,
                    cx,
                ))
            })
            .child(geometry.pointer_capture(cx))
    }

    fn render_right_tab_strip(
        &self,
        pane: &DockPane,
        start_inset: f32,
        trailing: bool,
        geometry: &DockGeometry,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        let id = pane.id;
        let tabs = pane.tabs();
        let expanded = self.right_dock_frame.fullscreen;
        let split_block = self.split_block(pane, start_inset);
        let strip = &self.sidebar.tab_strips[&id];
        let mut chips = strip.scroller();
        let caret = self.dock_tab_caret(id);
        let pointer_hover = bridge::has_pointer_hover();
        let font_size = self.dock_title_font_size();
        for (index, &tab) in tabs.iter().enumerate() {
            let active = pane.active() == tab;
            if index > 0 || caret == Some(0) {
                let show_divider = index > 0 && !active && tabs[index - 1] != pane.active();
                chips = chips.child(dock_tab_separator(caret == Some(index), show_divider, cx));
            }
            let quiet = !self.sidebar.surface.layout.can_close(id, tab);
            let label = self.dock_tab_label(tab);
            let title_editor = self.render_terminal_title_editor(id, tab, cx);
            let title_color = if active || quiet { p.ink } else { p.secondary };
            let chip_id = format!("right-tab-{id:?}-{tab:?}").into();
            let key_geometry = geometry.clone();
            let focus = strip.chip_focus(tab, active);
            let key_focus = focus.clone();
            let hovered = pointer_hover && strip.is_hovered(tab);
            let close_visible = !quiet && (active || hovered || focus.contains_focused(w, cx));
            let chip = dock_chip()
                .id(gpui::ElementId::Name(chip_id))
                .track_focus(&focus)
                .role(gpui::Role::Tab)
                .aria_label(label.clone())
                .aria_selected(active)
                .rounded(px(8.))
                .text_size(px(font_size))
                .line_height(px(font_size * 1.4))
                .text_color(title_color)
                .focus_visible(move |style| {
                    style.shadow(vec![
                        gpui::BoxShadow::new(px(0.), px(0.), p.brand_accent)
                            .spread_radius(px(1.))
                            .inset(),
                    ])
                })
                .child(geometry.measure_chip(id, index))
                .when(self.is_dragged_dock_tab(id, tab), |chip| chip.opacity(0.5))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, w, cx| {
                        if bridge::is_mac() && event.modifiers.control {
                            this.open_dock_tab_menu(id, tab, event.position, w, cx);
                        } else {
                            w.prevent_default();
                            this.press_dock_tab(id, tab, event.position);
                        }
                    }),
                )
                .on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, w, cx| {
                        this.open_dock_tab_menu(id, tab, event.position, w, cx);
                    }),
                )
                .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, w, cx| {
                    let stroke = &event.keystroke;
                    let mods = stroke.modifiers;
                    let menu_key =
                        stroke.key == "contextmenu" || (stroke.key == "f10" && mods.shift);
                    if menu_key
                        && key_focus.is_focused(w)
                        && !mods.control
                        && !mods.alt
                        && !mods.platform
                    {
                        if !event.is_held
                            && let Some(bounds) = key_geometry.chip_bounds(id, index)
                        {
                            this.open_dock_tab_menu(id, tab, bounds.bottom_left(), w, cx);
                        }
                        w.prevent_default();
                        cx.stop_propagation();
                    } else {
                        this.handle_dock_chip_key(id, tab, event, w, cx);
                    }
                }))
                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, w, cx| {
                    if !(bridge::is_mac() && event.modifiers().control) {
                        if event.click_count() == 2 && this.begin_terminal_title(id, tab, w, cx) {
                            cx.stop_propagation();
                            return;
                        }
                        this.activate_dock_chip(id, tab, w, cx);
                    }
                }))
                .when(active && !quiet, |chip| chip.bg(p.md_tag))
                .when(!quiet, |chip| {
                    chip.cursor_pointer()
                        .when(pointer_hover, |chip| {
                            chip.hover_listener_mode(
                                gpui::HoverListenerMode::InputModalityIndependent,
                            )
                            .on_hover(strip.track_hover(tab))
                            .when(hovered, |chip| chip.bg(p.hover))
                        })
                        .on_aux_click(cx.listener(move |this, event: &gpui::ClickEvent, w, cx| {
                            if event.is_middle_click() {
                                this.close_right_tab(id, tab, w, cx);
                            }
                        }))
                })
                .child(dock_tab_icon(tab, title_color, cx))
                .child(if let Some(editor) = title_editor {
                    div()
                        .flex_auto()
                        .min_w_0()
                        .when(!quiet, |title| title.pr(px(14.)))
                        .child(editor)
                } else {
                    dock_chip_title(label, font_size, title_color, close_visible, w, cx)
                })
                .when(!quiet, |chip| {
                    chip.child(
                        self.dock_button(DockControl::Close(id, tab), "关闭", false, cx)
                            .group("right-tab-close")
                            .absolute()
                            .top(px(4.))
                            .right(px(4.))
                            // A hidden close button remains reachable by Tab without
                            // taking pointer presses from the chip.
                            .size(px(if close_visible { 20. } else { 0. }))
                            .when(close_visible, |button| {
                                button.child(
                                    svg()
                                        .data(include_bytes!("../../assets/dock-tab-close.svg"))
                                        .size(px(14.))
                                        .text_color(p.label_tertiary)
                                        .when(pointer_hover, |icon| {
                                            icon.group_hover("right-tab-close", |style| {
                                                style.text_color(p.label_primary)
                                            })
                                        }),
                                )
                            })
                            .on_mouse_down(gpui::MouseButton::Left, |event, _, cx| {
                                if !(bridge::is_mac() && event.modifiers.control) {
                                    cx.stop_propagation();
                                }
                            }),
                    )
                });
            chips = chips.child(chip);
        }
        if caret == Some(tabs.len()) {
            chips = chips.child(dock_tab_separator(true, false, cx));
        }

        row()
            .w_full()
            .h(px(RIGHT_DOCK_HEADER_HEIGHT))
            .pt(px(10.))
            .pl(px(start_inset))
            .pr(px(6.))
            .gap(px(4.))
            .flex_none()
            .child(strip.frame(pane, caret == Some(0), chips, conversation_bg(cx)))
            .when(!tabs.contains(&RightPanelTab::Guide), |strip| {
                strip.child(
                    self.dock_button(DockControl::Add(id), "新标签页", false, cx)
                        .child(
                            svg()
                                .data(include_bytes!("../../assets/dock-add.svg"))
                                .size(px(14.))
                                .text_color(p.secondary),
                        ),
                )
            })
            .child(
                div().flex_1().h_full().when(
                    self.sidebar.surface.layout.floats().is_empty(),
                    |fill| {
                        fill.on_mouse_down(gpui::MouseButton::Left, |_, _, _| begin_window_drag())
                    },
                ),
            )
            .child(
                self.dock_button(
                    DockControl::Split(id),
                    split_block.unwrap_or("分栏"),
                    split_block.is_some(),
                    cx,
                )
                .child(
                    svg()
                        .data(include_bytes!("../../assets/dock-split.svg"))
                        .size(px(15.))
                        .text_color(p.secondary.opacity(if split_block.is_some() {
                            0.4
                        } else {
                            1.
                        })),
                ),
            )
            .when(trailing, |strip| {
                strip
                    .child(
                        self.dock_button(
                            DockControl::Fullscreen,
                            if expanded { "退出全屏" } else { "全屏" },
                            false,
                            cx,
                        )
                        .ml(px(4.))
                        .child(
                            svg()
                                .data(if expanded {
                                    include_bytes!("../../assets/dock-exit-fullscreen.svg")
                                        .as_slice()
                                } else {
                                    include_bytes!("../../assets/dock-fullscreen.svg").as_slice()
                                })
                                .size(px(15.))
                                .text_color(p.secondary),
                        ),
                    )
                    .child(
                        div()
                            .ml(px(4.))
                            .child(self.render_start_panel_toggle(true, cx)),
                    )
            })
    }

    pub(super) fn activate_right_tab(
        &mut self,
        pane: DockPaneId,
        tab: RightPanelTab,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar.surface.layout.select(pane, tab);
        self.focus_dock_pane(pane, w, cx);
        match tab {
            RightPanelTab::Terminal(key) => self.focus_terminal(key, w, cx),
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn open_right_tab(
        &mut self,
        pane: DockPaneId,
        tab: RightPanelTab,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_session.is_none() {
            return;
        }
        self.sidebar.surface.layout.open(pane, tab);
        let focused = self.sidebar.surface.layout.focused();
        if !focused.is_floating() {
            self.sidebar.surface.open = true;
        }
        self.focus_dock_pane(focused, w, cx);
        cx.notify();
    }

    pub(super) fn close_right_tab(
        &mut self,
        pane: DockPaneId,
        tab: RightPanelTab,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .sidebar
            .surface
            .layout
            .pane(pane)
            .is_some_and(|pane| pane.tabs().contains(&tab))
        {
            return;
        }
        if !self.sidebar.surface.layout.can_close(pane, tab) {
            self.toggle_right_panel(w, cx);
            return;
        }
        if self.remove_right_tab(pane, tab, cx) {
            if !self.sidebar.surface.layout.focused().is_floating() {
                self.focus_composer(w, cx);
            }
        } else {
            let focused = self.sidebar.surface.layout.focused();
            if self.dock_pane_on_screen(focused, cx) {
                self.focus_dock_pane(focused, w, cx);
                if let RightPanelTab::Terminal(key) = self.sidebar.surface.layout.active() {
                    self.focus_terminal(key, w, cx);
                }
            } else {
                self.focus_composer(w, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn remove_right_tab(
        &mut self,
        pane: DockPaneId,
        tab: RightPanelTab,
        cx: &mut Context<Self>,
    ) -> bool {
        let layout = &self.sidebar.surface.layout;
        if !layout
            .pane(pane)
            .is_some_and(|pane| pane.tabs().contains(&tab))
            || !layout.can_close(pane, tab)
        {
            return false;
        }
        match tab {
            RightPanelTab::Terminal(key) => self.close_terminal(key, cx),
            RightPanelTab::Browser(key) => self.close_browser_tab(key),
            RightPanelTab::Document(key) => self.close_document(key),
            RightPanelTab::Plan(key) => {
                self.sidebar.surface.plans.remove(&key);
                self.sidebar.plans.remove(&key);
            }
            _ => {}
        }
        let collapsed = self.sidebar.surface.layout.close(pane, tab);
        if collapsed {
            self.sidebar.surface.open = false;
            self.sidebar.surface.fullscreen = false;
        }
        collapsed
    }

    pub(super) fn dock_tab_label(&self, tab: RightPanelTab) -> gpui::SharedString {
        let label = match tab {
            RightPanelTab::Guide => "开始",
            RightPanelTab::Files => "文件",
            RightPanelTab::Terminal(key) => self
                .sidebar
                .terminals
                .get(&key)
                .and_then(|terminal| terminal.title.as_deref())
                .or_else(|| self.sidebar.surface.terminals.get(&key)?.title.as_deref())
                .unwrap_or("终端"),
            RightPanelTab::Document(key) => self
                .sidebar
                .surface
                .documents
                .get(&key)
                .and_then(|path| path.rsplit('/').next())
                .unwrap_or("文件"),
            RightPanelTab::Plan(key) => self
                .sidebar
                .plans
                .get(&key)
                .and_then(|plan| plan.title())
                .unwrap_or("计划"),
            RightPanelTab::Browser(key) => self
                .sidebar
                .surface
                .browsers
                .get(&key)
                .map(|checkpoint| checkpoint.title.as_str())
                .filter(|title| !title.is_empty())
                .unwrap_or("浏览器"),
        };
        label.to_owned().into()
    }

    pub(super) fn render_dock_body(
        &self,
        pane: DockPaneId,
        tab: RightPanelTab,
        width: f32,
        height: f32,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match tab {
            RightPanelTab::Guide => div()
                .id("right-guide-scroll")
                .size_full()
                .overflow_y_scroll()
                .child(self.render_right_guide(pane, height, cx))
                .into_any_element(),
            RightPanelTab::Files => self.render_right_files(pane, cx).into_any_element(),
            RightPanelTab::Terminal(key) => self
                .render_right_terminal(key, pane.is_floating(), cx)
                .into_any_element(),
            RightPanelTab::Document(key) => self
                .render_document(pane, key, width, w, cx)
                .into_any_element(),
            RightPanelTab::Plan(key) => self.render_plan_document(key, w, cx).into_any_element(),
            RightPanelTab::Browser(key) => self
                .render_right_browser(key, pane.is_floating(), w, cx)
                .into_any_element(),
        }
    }

    fn render_right_guide(
        &self,
        pane: DockPaneId,
        viewport_height: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        let cards = column()
            .w_full()
            .max_w(px(380.))
            .gap(px(14.))
            .child(
                self.start_card(
                    pane,
                    "files",
                    "工作区文件",
                    Some("浏览会话工作区的文件"),
                    svg()
                        .data(include_bytes!("../../assets/start-files.svg"))
                        .text_color(gpui::Hsla::from(gpui::rgb(0xffbf58)))
                        .size(px(22.))
                        .flex_none(),
                    Some("⌘ P"),
                    cx,
                ),
            )
            .child(
                self.start_card(
                    pane,
                    "terminal",
                    "新建终端",
                    Some("在会话工作区运行命令"),
                    svg()
                        .data(include_bytes!("../../assets/start-terminal-guide.svg"))
                        .text_color(gpui::rgb(0x679efe))
                        .size(px(26.))
                        .flex_none(),
                    Some("⌃ `"),
                    cx,
                ),
            )
            .child(self.start_card(
                pane,
                "browser",
                "浏览器",
                Some("浏览网页"),
                icon(Glyph::Globe, 20., p.state_business_primary),
                Some("⌘ T"),
                cx,
            ))
            .child(
                self.start_card(
                    pane,
                    "tools",
                    "更多工具",
                    None,
                    svg()
                        .data(include_bytes!("../../assets/start-tools.svg"))
                        .size(px(24.))
                        .flex_none()
                        .text_color(p.secondary),
                    None,
                    cx,
                ),
            );

        div()
            .min_h(px((viewport_height - RIGHT_DOCK_HEADER_HEIGHT).max(0.)))
            .flex_none()
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .px(px(24.))
            .pb(px(((viewport_height - RIGHT_DOCK_HEADER_HEIGHT) * 0.10
                + 16.)
                .min((viewport_height - 425.).max(0.))))
            .child(
                column()
                    .w_full()
                    .max_w(px(380.))
                    .gap(px(30.))
                    .child(row().w_full().justify_center().child(start_compass(
                        56.,
                        gpui::rgb(if dark(cx) { 0x3c3c3d } else { 0xe5e5e5 }).into(),
                    )))
                    .child(cards)
                    .when_some(self.error.clone(), |element, error| {
                        element.child(
                            div()
                                .w_full()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(p.danger)
                                .child(error),
                        )
                    }),
            )
    }

    fn render_right_terminal(&self, key: u64, floating: bool, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let new_terminal = |compact: bool, cx: &Context<Self>| {
            row()
                .id("terminal-recreate")
                .role(gpui::Role::Button)
                .aria_label("新建终端")
                .tab_stop(true)
                .h(px(if compact { 28. } else { 36. }))
                .px(px(if compact { 10. } else { 14. }))
                .gap(px(4.))
                .rounded(px(if compact { 8. } else { 10. }))
                .bg(p.button_primary_fill)
                .hover(|style| style.bg(p.button_primary_hover))
                .cursor_pointer()
                .text_color(p.label_primary_foreground)
                .text_size(px(if compact { 12. } else { 14. }))
                .child(icon(Glyph::Plus, 16., p.label_primary_foreground))
                .child("新建终端")
                .on_click(cx.listener(move |this, _, w, cx| this.replace_terminal(key, w, cx)))
        };
        let Some(tab) = self
            .sidebar
            .terminals
            .get(&key)
            .filter(|tab| !matches!(tab.connection, TerminalConnection::Missing))
        else {
            return column()
                .size_full()
                .pt(px(8.))
                .items_center()
                .justify_center()
                .px(px(24.))
                .gap(px(16.))
                .child(
                    div().relative().size(px(36.))
                        .child(div().absolute().left(px(3. * 36. / 28.)).top(px(5. * 36. / 28.))
                            .w(px(22. * 36. / 28.)).h(px(19. * 36. / 28.))
                            .rounded(px(3. * 36. / 28.)).bg(gpui::rgb(0x17191d)))
                        .child(svg().data(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 28 28"><path d="m8 10 4 4-4 4M15 18h5" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" fill="none"/></svg>"#)
                            .absolute().inset_0().size(px(36.)).text_color(gpui::white())),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .child("此终端已不存在，请新建终端。"),
                )
                .child(new_terminal(false, cx));
        };
        let status = match tab.connection {
            TerminalConnection::Opening {
                reading_shells: true,
                ..
            } => Some("正在读取终端环境…".into()),
            TerminalConnection::Opening { create, .. } => Some(
                if create {
                    "正在启动…"
                } else {
                    "正在连接…"
                }
                .to_owned(),
            ),
            TerminalConnection::Exited { code } => {
                let code = code.map_or_else(|| "—".into(), |code| code.to_string());
                Some(format!("进程已退出（{code}）"))
            }
            _ => None,
        };
        let failure = match &tab.connection {
            TerminalConnection::Failed {
                error, has_screen, ..
            } => Some((
                error,
                if *has_screen {
                    "重新连接"
                } else {
                    "重试"
                },
            )),
            _ => None,
        };
        let has_screen = matches!(
            tab.connection,
            TerminalConnection::Connected
                | TerminalConnection::Exited { .. }
                | TerminalConnection::Failed {
                    has_screen: true,
                    ..
                }
        );
        column()
            .h_full()
            .w_full()
            .pt(px(8.))
            .gap_0()
            .when(status.is_some() || failure.is_some(), |panel| {
                panel.child(
                    row()
                        .w_full()
                        .flex_none()
                        .flex_wrap()
                        .gap(px(8.))
                        .px(px(12.))
                        .py(px(5.))
                        .text_size(px(13.))
                        .text_color(p.ink)
                        .bg(p.bg_layer_2)
                        .children(status)
                        .when(
                            matches!(tab.connection, TerminalConnection::Exited { .. }),
                            |row| row.child(new_terminal(true, cx)),
                        )
                        .when_some(failure, |status_bar, (_, label)| {
                            status_bar.child(
                                row()
                                    .id("terminal-retry")
                                    .role(gpui::Role::Button)
                                    .aria_label(label)
                                    .tab_stop(true)
                                    .h(px(28.))
                                    .px(px(10.))
                                    .rounded(px(8.))
                                    .border_1()
                                    .border_color(p.border_l3)
                                    .hover(|style| style.bg(p.hover))
                                    .cursor_pointer()
                                    .text_size(px(12.))
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, w, cx| {
                                        this.retry_terminal(key, w, cx);
                                    })),
                            )
                        }),
                )
            })
            .when(has_screen, |panel| {
                panel.child(
                    column()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .gap_0()
                        .bg(p.bg_base)
                        .when(floating && failure.is_none(), |body| {
                            let radius = px(super::floating_panes::FLOAT_RADIUS);
                            body.rounded_bl(radius).rounded_br(radius)
                        })
                        .child(tab.screen.clone()),
                )
            })
            .when_some(failure, |panel, (error, _)| {
                panel.child(
                    div()
                        .w_full()
                        .flex_none()
                        .px(px(12.))
                        .py(px(10.))
                        .text_size(px(13.))
                        .text_color(p.ink)
                        .when(!floating, |band| band.bg(p.bg_layer_2))
                        .child(format!("终端错误：{error}")),
                )
            })
    }

    pub(super) fn render_start_panel_toggle(
        &self,
        in_strip: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let open = self.sidebar.surface.open;
        self.dock_button(
            DockControl::Toggle { in_strip },
            if open {
                "收起侧边栏"
            } else {
                "打开侧边栏"
            },
            false,
            cx,
        )
        .aria_label(if open {
            "收起右侧边栏"
        } else {
            "打开右侧边栏"
        })
        .child(dock_sidebar_glyph().text_color(p.secondary))
    }

    fn start_card(
        &self,
        pane: DockPaneId,
        id: &'static str,
        title: &'static str,
        description: Option<&'static str>,
        artwork: impl IntoElement,
        shortcut: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let terminal = id == "terminal";
        let label = description.map_or_else(
            || title.to_string(),
            |description| format!("{title} {description}"),
        );
        row()
            .id(gpui::ElementId::Name(format!("start-card-{id}").into()))
            .relative()
            .w_full()
            .h(px(if description.is_some() { 67. } else { 56. }))
            .flex_none()
            .px(px(20.))
            .gap(px(14.))
            .rounded(px(20.))
            .border(px(0.5))
            .border_color(p.border_l3)
            .bg(p.bg_layer_1)
            .when(!terminal, |card| {
                card.role(gpui::Role::Button)
                    .aria_label(label.clone())
                    .tab_index(0)
                    .cursor_pointer()
                    .hover(|style| style.bg(p.hover))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.error = None;
                        match id {
                            "tools" => this.open_page(WorkspacePage::More, cx),
                            "files" => this.open_right_tab(pane, RightPanelTab::Files, w, cx),
                            "browser" => this.new_browser_tab(pane, None, w, cx),
                            _ => {}
                        }
                    }))
            })
            .when(terminal, |card| {
                card.overflow_hidden().child(
                    div()
                        .id("terminal-guide-launch")
                        .role(gpui::Role::Button)
                        .aria_label(label)
                        .aria_keyshortcuts("Control+`")
                        .tab_index(0)
                        .absolute()
                        .inset_0()
                        .cursor_pointer()
                        .hover(|style| style.bg(p.hover))
                        .focus_visible(|style| {
                            style.border_2().border_color(p.state_business_primary)
                        })
                        .on_click(cx.listener(move |this, event: &gpui::ClickEvent, w, cx| {
                            if !(bridge::is_mac() && event.modifiers().control) {
                                this.error = None;
                                this.new_terminal(pane, None, w, cx);
                            }
                        })),
                )
            })
            .child(
                row()
                    .size(px(26.))
                    .flex_none()
                    .justify_center()
                    .child(artwork),
            )
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        row()
                            .w_full()
                            .gap(px(if terminal { 4. } else { 5. }))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .line_height(px(20.))
                                    .truncate()
                                    .child(title),
                            )
                            .when(terminal, |element| {
                                element.child(self.terminal_shell_trigger(pane, cx))
                            }),
                    )
                    .children(description.map(|description| {
                        div()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(p.faint)
                            .truncate()
                            .child(description)
                    })),
            )
            .children(shortcut.map(|shortcut| {
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .font_family(if bridge::is_windows() {
                        "Segoe UI"
                    } else {
                        ".SystemUIFont"
                    })
                    .text_color(p.faint)
                    .child(shortcut)
            }))
    }
}

pub(super) fn dock_tab_icon(
    tab: RightPanelTab,
    title_color: gpui::Hsla,
    cx: &App,
) -> gpui::AnyElement {
    let color = dsh(cx).faint;
    match tab {
        RightPanelTab::Guide => start_compass(16., color).into_any_element(),
        RightPanelTab::Files => icon(Glyph::Folder, 16., color).into_any_element(),
        RightPanelTab::Terminal(_) => svg()
            .data(include_bytes!("../../assets/start-terminal.svg"))
            .size(px(16.))
            .flex_none()
            .text_color(title_color)
            .into_any_element(),
        RightPanelTab::Document(_) | RightPanelTab::Plan(_) => {
            super::dock_files::file_sheet_icon(16.).into_any_element()
        }
        RightPanelTab::Browser(_) => icon(Glyph::Globe, 16., color).into_any_element(),
    }
}

pub(super) fn dock_sidebar_glyph() -> gpui::Svg {
    svg()
        .data(include_bytes!("../../assets/sidebar-right.svg"))
        .size(px(15.))
}

fn start_compass(size: f32, color: gpui::Hsla) -> gpui::Svg {
    svg()
        .data(include_bytes!("../../assets/start-compass.svg"))
        .size(px(size))
        .flex_none()
        .text_color(color)
}
